//! The other side of the door: what the `annulus-agent` CLI and its MCP server talk through. Plain
//! blocking I/O, since a CLI does one thing at a time.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;

use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use crate::door::DoorFile;
use crate::protocol::{
    self, Call, Hello, HostInfo, Outcome, Reply, Request, RunOutput, Status, TabInfo,
};

/// Why a call through the door failed.
#[derive(Debug)]
pub enum ClientError {
    /// No door is open: Annulus is not running, automation is off, or the vault is locked.
    Closed,
    /// The door was there but talking through it failed.
    Io(std::io::Error),
    /// Annulus answered with an error.
    Refused(String),
    /// The answer was not what was asked for.
    Garbled(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(
                f,
                "Annulus's door is closed: start Annulus, unlock the vault and switch on \
                 Settings > Agents"
            ),
            Self::Io(error) => write!(f, "talking to Annulus failed: {error}"),
            Self::Refused(why) => write!(f, "{why}"),
            Self::Garbled(why) => write!(f, "Annulus's answer made no sense: {why}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(windows)]
type Stream = std::fs::File;
#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

/// A connection through the door.
pub struct Client {
    read: BufReader<Stream>,
    write: Stream,
    next: u64,
}

impl Client {
    /// Connects through the door of the data folder `dir`.
    ///
    /// # Errors
    /// [`ClientError::Closed`] when no door is open there.
    pub fn connect(dir: &Path) -> Result<Self, ClientError> {
        let text = Zeroizing::new(
            std::fs::read_to_string(crate::door_file(dir)).map_err(|_| ClientError::Closed)?,
        );
        let door: DoorFile = protocol::decode(&text).map_err(|_| ClientError::Closed)?;
        let token = Zeroizing::new(door.token);
        let stream = open(&door.pipe)?;
        let mut client = Self {
            read: BufReader::new(stream.try_clone()?),
            write: stream,
            next: 1,
        };
        let mut hello = Zeroizing::new(protocol::encode(&Hello {
            token: token.to_string(),
        }));
        hello.push('\n');
        client.write.write_all(hello.as_bytes())?;
        Ok(client)
    }

    /// Asks for `request` and reads the answer as a `T`.
    ///
    /// # Errors
    /// When the door fails, Annulus refuses, or the answer is not a `T`.
    pub fn call<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, ClientError> {
        let id = self.next;
        self.next += 1;
        let mut line = protocol::encode(&Call { id, request });
        line.push('\n');
        self.write.write_all(line.as_bytes())?;
        self.write.flush()?;
        let mut answer = String::new();
        if self.read.read_line(&mut answer)? == 0 {
            // The door shut the connection: a wrong token, or it closed meanwhile.
            return Err(ClientError::Closed);
        }
        let reply: Reply = protocol::decode(answer.trim_end()).map_err(ClientError::Garbled)?;
        if reply.id != id {
            return Err(ClientError::Garbled(format!(
                "the answer to {} came for {id}",
                reply.id
            )));
        }
        match reply.result {
            Outcome::Ok(value) => {
                serde_json::from_value(value).map_err(|e| ClientError::Garbled(e.to_string()))
            }
            Outcome::Error(why) => Err(ClientError::Refused(why)),
        }
    }

    /// The app's state.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn status(&mut self) -> Result<Status, ClientError> {
        self.call(Request::Status)
    }

    /// The saved hosts.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn hosts(&mut self) -> Result<Vec<HostInfo>, ClientError> {
        self.call(Request::Hosts)
    }

    /// Replaces the notes of saved host `host`; the host as it is now.
    ///
    /// # Errors
    /// As [`Self::call`]; notes over 2,000 characters are refused.
    pub fn set_notes(&mut self, host: u64, notes: &str) -> Result<HostInfo, ClientError> {
        self.call(Request::SetNotes {
            host,
            notes: notes.to_owned(),
        })
    }

    /// The open tabs.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn tabs(&mut self) -> Result<Vec<TabInfo>, ClientError> {
        self.call(Request::Tabs)
    }

    /// The text of terminal tab `tab`: its last `lines` lines, or all of it.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn read(&mut self, tab: u64, lines: Option<usize>) -> Result<String, ClientError> {
        self.call(Request::Read { tab, lines })
    }

    /// Opens a terminal tab of the agent's own: a session to saved host `host`, or a local
    /// shell.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn open(&mut self, host: Option<u64>) -> Result<TabInfo, ClientError> {
        self.call(Request::Open { host })
    }

    /// Runs `command` in agent tab `tab` and returns its output and how far it got, waiting
    /// `wait` seconds at most.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn run(
        &mut self,
        tab: u64,
        command: &str,
        wait: Option<u64>,
    ) -> Result<RunOutput, ClientError> {
        self.call(Request::Run {
            tab,
            command: command.to_owned(),
            wait,
        })
    }

    /// Waits up to `wait` seconds for the last command in agent tab `tab` to finish.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn wait(&mut self, tab: u64, wait: Option<u64>) -> Result<RunOutput, ClientError> {
        self.call(Request::Wait { tab, wait })
    }

    /// Presses Ctrl+C in agent tab `tab`.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn interrupt(&mut self, tab: u64) -> Result<(), ClientError> {
        self.call(Request::Interrupt { tab })
    }

    /// Closes agent tab `tab`.
    ///
    /// # Errors
    /// As [`Self::call`].
    pub fn close(&mut self, tab: u64) -> Result<(), ClientError> {
        self.call(Request::Close { tab })
    }
}

#[cfg(windows)]
fn open(pipe: &str) -> Result<Stream, ClientError> {
    /// All of the pipe's instances are taken: the door makes the next one in a moment.
    const PIPE_BUSY: i32 = 231;
    // As long as the door gives a client to show its token (5 s): a busy machine can keep the
    // door from making the next instance for more than a second.
    for _ in 0..200 {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe)
        {
            Ok(file) => return Ok(file),
            Err(error) if error.raw_os_error() == Some(PIPE_BUSY) => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ClientError::Closed);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(ClientError::Closed)
}

#[cfg(unix)]
fn open(socket: &str) -> Result<Stream, ClientError> {
    std::os::unix::net::UnixStream::connect(socket).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => ClientError::Closed,
        _ => error.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::door::Door;
    use crate::protocol::Response;
    use std::sync::Arc;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("annulus-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Answers every call that reaches `door` until `until` calls are done, as an app would.
    fn answer(door: &Door, until: usize) {
        let mut done = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while done < until && std::time::Instant::now() < deadline {
            for call in door.take_calls() {
                let answer = match call.request {
                    Request::Status => Ok(Response::Status(Status {
                        version: "test".into(),
                        unlocked: true,
                        tabs: 2,
                    })),
                    Request::Read { tab, .. } => Err(format!("no tab {tab}")),
                    _ => Ok(Response::Tabs(Vec::new())),
                };
                call.answer(answer);
                done += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_client_with_the_token_is_answered_and_the_door_file_goes_when_it_closes() {
        let dir = temp_dir("open");
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let door = Door::open(
            &dir,
            Arc::new(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .unwrap();
        assert!(crate::door_file(&dir).exists());
        let asking = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let mut client = Client::connect(&dir).unwrap();
                let status = client.status().unwrap();
                let refused = client.read(9, None).unwrap_err().to_string();
                (status, refused)
            }
        });
        answer(&door, 2);
        let (status, refused) = asking.join().unwrap();
        assert_eq!(status.version, "test");
        assert_eq!(refused, "no tab 9");
        assert!(
            woken.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "the window was woken"
        );
        drop(door);
        assert!(
            !crate::door_file(&dir).exists(),
            "closed, the door file is gone"
        );
        assert!(matches!(Client::connect(&dir), Err(ClientError::Closed)));
    }

    #[test]
    fn past_eight_clients_the_newest_is_turned_away_until_one_leaves() {
        let dir = temp_dir("cap");
        let door = Door::open(&dir, Arc::new(|| {})).unwrap();
        // Eight clients, each seated: the door has answered a call from every one.
        let held: Vec<Client> = std::thread::scope(|scope| {
            let asking: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let mut client = Client::connect(&dir).unwrap();
                        client.status().unwrap();
                        client
                    })
                })
                .collect();
            answer(&door, 8);
            asking
                .into_iter()
                .map(|asking| asking.join().unwrap())
                .collect()
        });
        let mut ninth = Client::connect(&dir).unwrap();
        assert!(ninth.status().is_err(), "the ninth gets no answer");
        drop(held);
        // A seat is free once the door has seen a client go; until then a newcomer is turned
        // away too, so it knocks until it gets in.
        let asking = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    match Client::connect(&dir).and_then(|mut client| client.status()) {
                        Ok(status) => return Ok(status.tabs),
                        Err(error) if std::time::Instant::now() > deadline => return Err(error),
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
            }
        });
        answer(&door, 1);
        assert_eq!(
            asking.join().unwrap().unwrap(),
            2,
            "with room again, one gets in"
        );
    }

    #[test]
    fn a_wrong_token_gets_nothing_back_and_no_request_reaches_the_app() {
        let dir = temp_dir("token");
        let door = Door::open(&dir, Arc::new(|| {})).unwrap();
        let path = crate::door_file(&dir);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut file: DoorFile = protocol::decode(&text).unwrap();
        file.token = "0".repeat(file.token.len());
        // A copy of the door file with the wrong token, in another folder.
        let other = temp_dir("token-other");
        std::fs::write(crate::door_file(&other), protocol::encode(&file)).unwrap();
        let mut client = Client::connect(&other).unwrap();
        let answer = client.status();
        assert!(
            matches!(answer, Err(ClientError::Closed | ClientError::Io(_))),
            "{answer:?}"
        );
        assert!(door.take_calls().is_empty());
    }
}
