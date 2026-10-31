//! The door the app opens while the owner allows automation and the vault is unlocked
//! (ADR-0020 §2–4): a named pipe on Windows, a Unix socket in an owner-only folder elsewhere.
//!
//! A client's first line must carry the door's token; until it does, the door writes nothing
//! back. Requests then go to the app's thread through [`Door::take_calls`], and the window is
//! woken for each. Dropping the door closes it: the listener and every connection stop, the
//! door file goes and the token is wiped.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use zeroize::Zeroizing;

use crate::protocol::{self, Call, Hello, Outcome, Reply, Request, Response};

/// Wakes the app's window from another thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The longest first line taken before a client has shown its token.
const HELLO_MAX: u64 = 4 * 1024;
/// The longest request line.
const CALL_MAX: u64 = 1024 * 1024;

/// The open door.
pub struct Door {
    calls: mpsc::Receiver<Pending>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    file: PathBuf,
    /// The Unix socket's own folder, removed as the door closes.
    #[cfg(unix)]
    folder: PathBuf,
}

/// A request waiting for the app's answer.
pub struct Pending {
    /// What was asked.
    pub request: Request,
    reply: tokio::sync::oneshot::Sender<Outcome>,
}

impl Pending {
    /// Sends the app's answer back to the client that asked.
    pub fn answer(self, answer: Result<Response, String>) {
        // The client may have gone in the meantime; nothing waits for this answer then.
        let _ = self.reply.send(Outcome::of(answer));
    }
}

/// What the door file says: where the door is and its token.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct DoorFile {
    pub(crate) pipe: String,
    pub(crate) token: String,
}

impl Door {
    /// Opens the door for the data folder `dir`; `wake` is called for each request.
    ///
    /// # Errors
    /// When the pipe or socket cannot be made, or the door file cannot be written.
    pub fn open(dir: &Path, wake: Wake) -> io::Result<Self> {
        let token = hex(&*random::<32>()?);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (sender, calls) = mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let file = crate::door_file(dir);
        let listener;
        let pipe;
        #[cfg(unix)]
        let folder;
        {
            let _entered = runtime.enter();
            #[cfg(windows)]
            {
                pipe = format!(r"\\.\pipe\annulus-{}", hex(&*random::<12>()?).as_str());
                listener = windows::listen(&pipe)?;
            }
            #[cfg(unix)]
            {
                let made = unix::folder()?;
                pipe = made.join("door").to_string_lossy().into_owned();
                listener = unix::listen(&made, &pipe)?;
                folder = made;
            }
        }
        write_door_file(
            &file,
            &DoorFile {
                pipe: pipe.clone(),
                token: token.to_string(),
            },
        )
        .inspect_err(|_| {
            // The socket's folder goes with the door that could not be opened.
            #[cfg(unix)]
            let _ = std::fs::remove_dir_all(&folder);
        })?;
        let token = Arc::new(token);
        let thread = std::thread::Builder::new()
            .name("annulus-door".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let accepting = accept(listener, &pipe, token, sender, wake);
                    tokio::select! {
                        _ = stopped => {}
                        () = accepting => {}
                    }
                });
            })?;
        Ok(Self {
            calls,
            stop: Some(stop),
            thread: Some(thread),
            file,
            #[cfg(unix)]
            folder,
        })
    }

    /// The requests that came in since the last call, oldest first.
    #[must_use]
    pub fn take_calls(&self) -> Vec<Pending> {
        self.calls.try_iter().collect()
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.file);
        #[cfg(unix)]
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Accepts connections until the door closes; each is served on its own task, at most
/// [`CLIENTS_MAX`] at once. A failed accept (out of file handles) waits and tries again
/// rather than leaving a door file that nothing answers.
async fn accept(
    listener: Listener,
    pipe: &str,
    token: Arc<Zeroizing<String>>,
    calls: mpsc::Sender<Pending>,
    wake: Wake,
) {
    let room = Arc::new(tokio::sync::Semaphore::new(CLIENTS_MAX));
    #[cfg(windows)]
    let mut listener = listener;
    loop {
        #[cfg(windows)]
        let connection = {
            let connected = listener.connect().await;
            // The next client gets a fresh instance; this one is the connected client's.
            let Some(next) = windows::next_retrying(pipe).await else {
                return;
            };
            let connection = std::mem::replace(&mut listener, next);
            if connected.is_err() {
                continue;
            }
            connection
        };
        #[cfg(unix)]
        let connection = match listener.accept().await {
            Ok((stream, _)) if unix::same_user(&stream, pipe) => stream,
            Ok(_) => continue,
            Err(_) => {
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        // Full: the newest is turned away, so a pile of idle connections cannot crowd out
        // the ones already working.
        let Ok(seat) = Arc::clone(&room).try_acquire_owned() else {
            continue;
        };
        let (token, calls, wake) = (Arc::clone(&token), calls.clone(), Arc::clone(&wake));
        tokio::spawn(async move {
            serve(connection, token, calls, wake).await;
            drop(seat);
        });
    }
}

/// How many clients may be connected at once.
const CLIENTS_MAX: usize = 8;
/// How long a client has to show its token.
const HELLO_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a client may sit without asking anything before it is let go.
const IDLE_MAX: std::time::Duration = std::time::Duration::from_mins(30);
/// The wait before trying again after a failed accept.
const RETRY: std::time::Duration = std::time::Duration::from_millis(200);

/// One client: its token first, then a reply for each request.
async fn serve<S: AsyncRead + AsyncWrite>(
    stream: S,
    token: Arc<Zeroizing<String>>,
    calls: mpsc::Sender<Pending>,
    wake: Wake,
) {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let Ok(Some(hello)) = tokio::time::timeout(HELLO_WITHIN, read_line(&mut read, HELLO_MAX)).await
    else {
        return;
    };
    let hello = Zeroizing::new(hello);
    let shown = protocol::decode::<Hello>(&hello).map(|hello| Zeroizing::new(hello.token));
    if !shown.is_ok_and(|shown| same(shown.as_bytes(), token.as_bytes())) {
        return;
    }
    while let Ok(Some(line)) = tokio::time::timeout(IDLE_MAX, read_line(&mut read, CALL_MAX)).await
    {
        let reply = match protocol::decode::<Call>(&line) {
            Ok(Call { id, request }) => {
                let (reply, answer) = tokio::sync::oneshot::channel();
                if calls.send(Pending { request, reply }).is_err() {
                    return;
                }
                wake();
                let result = answer
                    .await
                    .unwrap_or_else(|_| Outcome::Error("Annulus closed the door".into()));
                Reply { id, result }
            }
            Err(error) => Reply {
                id: 0,
                result: Outcome::Error(format!("not a request: {error}")),
            },
        };
        let mut out = protocol::encode(&reply);
        out.push('\n');
        if write.write_all(out.as_bytes()).await.is_err() || write.flush().await.is_err() {
            return;
        }
    }
}

/// The next line, without its line end; `None` at the end of the stream, on an error or when
/// the line runs past `max` bytes.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R, max: u64) -> Option<String> {
    use tokio::io::AsyncReadExt as _;
    let mut line = String::new();
    let read = read.take(max).read_line(&mut line).await.ok()?;
    if read == 0 || !line.ends_with('\n') {
        return None;
    }
    line.truncate(line.trim_end_matches(['\r', '\n']).len());
    Some(line)
}

/// Whether `a` and `b` are equal, in time that does not depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `N` random bytes, wiped when dropped.
fn random<const N: usize>() -> io::Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(&mut *bytes).map_err(|_| io::Error::other("no randomness"))?;
    Ok(bytes)
}

/// `bytes` in hex, in a string sized for it at once (no copies left behind as it grows) and
/// wiped when dropped.
fn hex(bytes: &[u8]) -> Zeroizing<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    out
}

/// Writes the door file afresh: whatever was at `path` (a file left by a crash, a link) goes
/// first, and the new file is made new, owner-only on Unix.
fn write_door_file(path: &Path, door: &DoorFile) -> io::Result<()> {
    use std::io::Write as _;
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let text = Zeroizing::new(protocol::encode(door));
    options.open(path)?.write_all(text.as_bytes())
}

#[cfg(windows)]
type Listener = tokio::net::windows::named_pipe::NamedPipeServer;
#[cfg(unix)]
type Listener = tokio::net::UnixListener;

#[cfg(windows)]
mod windows {
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// The pipe's first instance: no other program may already own the name.
    pub(super) fn listen(name: &str) -> std::io::Result<NamedPipeServer> {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(name)
    }

    /// One more instance, for the next client; tried a few times when the system is short of
    /// handles, and `None` if it never comes (the door then stops answering).
    pub(super) async fn next_retrying(name: &str) -> Option<NamedPipeServer> {
        for _ in 0..50 {
            match ServerOptions::new()
                .reject_remote_clients(true)
                .create(name)
            {
                Ok(next) => return Some(next),
                Err(_) => tokio::time::sleep(super::RETRY).await,
            }
        }
        None
    }
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    use std::path::{Path, PathBuf};

    /// A new folder only this user may enter, in the runtime folder (or the temp folder).
    pub(super) fn folder() -> std::io::Result<PathBuf> {
        let base =
            std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let folder = base.join(format!(
            "annulus-{}",
            super::hex(&*super::random::<8>()?).as_str()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&folder)?;
        Ok(folder)
    }

    pub(super) fn listen(
        _folder: &Path,
        socket: &str,
    ) -> std::io::Result<tokio::net::UnixListener> {
        tokio::net::UnixListener::bind(socket)
    }

    /// Whether the peer of `stream` runs as the user that owns the door's folder (us).
    pub(super) fn same_user(stream: &tokio::net::UnixStream, socket: &str) -> bool {
        let ours = Path::new(socket)
            .parent()
            .and_then(|folder| std::fs::metadata(folder).ok())
            .map(|meta| meta.uid());
        let theirs = stream.peer_cred().ok().map(|cred| cred.uid());
        ours.is_some() && ours == theirs
    }
}
