//! An SFTP tab (F-SFTP-002…005): browse a host's files, download, upload by dropping
//! files on the window, make folders, rename and delete. The connection runs on its own
//! thread; the tab sends it commands and draws what comes back. After a failure the thread
//! checks the session before the next command and reconnects if it is gone, so Retry on a
//! failed transfer continues it (F-SFTP-004).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use annulus_session::{Answers, Hop, SessionEvent, SshFiles, Target};
use annulus_sftp::{Conflict, Entry, Kind, Outcome, Sftp, join};

use crate::local_pane::LocalPane;

/// What the tab asks the connection thread to do.
pub(crate) enum Command {
    /// List a folder; replies carry the request number so stale ones can be dropped.
    List(String, u64),
    Download {
        id: u64,
        remote: String,
        to: PathBuf,
        conflict: Conflict,
        cancel: Arc<AtomicBool>,
    },
    Upload {
        id: u64,
        local: PathBuf,
        dir: String,
        conflict: Conflict,
        cancel: Arc<AtomicBool>,
    },
    /// A folder with everything in it.
    UploadTree {
        id: u64,
        local: PathBuf,
        dir: String,
        conflict: Conflict,
        cancel: Arc<AtomicBool>,
    },
    CreateDir(String),
    Rename(String, String),
    Remove(String),
    /// Set the permission bits of a path.
    Chmod(String, u32),
}

/// What the connection thread reports.
enum Reply {
    /// A host-key or password question, or the connection failed.
    Session(SessionEvent),
    Ready {
        home: String,
    },
    Listed {
        dir: String,
        request: u64,
        entries: Vec<Entry>,
    },
    ListFailed {
        dir: String,
        request: u64,
        message: String,
    },
    Progress {
        id: u64,
        done: u64,
        total: u64,
    },
    Finished {
        id: u64,
        result: Result<Outcome, String>,
    },
    /// A folder was made, renamed or removed; `error` says why not.
    Changed {
        error: Option<String>,
    },
}

type Send = Arc<dyn Fn(Reply) + std::marker::Send + Sync>;

/// The running connection.
pub(crate) struct Worker {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    replies: Receiver<Reply>,
    answers: Answers,
}

impl Worker {
    fn start(
        login: Arc<Mutex<Login>>,
        wake: impl Fn() + std::marker::Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (commands, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let (reply_tx, replies) = channel();
        let reply_tx = Mutex::new(reply_tx);
        let send: Send = Arc::new(move |reply| {
            if let Ok(tx) = reply_tx.lock() {
                let _ = tx.send(reply);
            }
            wake();
        });
        let answers = Answers::default();
        let prompt_answers = answers.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("sftp session".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let open = Opener {
                        login,
                        answers: prompt_answers,
                        send: send.clone(),
                    };
                    let mut session = match open.open().await {
                        Ok(session) => session,
                        Err(message) => {
                            return send(Reply::Session(SessionEvent::Failed(message)));
                        }
                    };
                    let home = session.1.home().await.unwrap_or_else(|_| "/".to_owned());
                    send(Reply::Ready { home });
                    let suspect = Arc::new(AtomicBool::new(false));
                    let slots = Arc::new(tokio::sync::Semaphore::new(TRANSFER_SLOTS));
                    // Each command runs as its own task, so a long transfer does not hold up
                    // browsing. The loop ends when the tab closes.
                    while let Some(command) = incoming.recv().await {
                        let alive = !session.0.is_closed()
                            && (!suspect.swap(false, Ordering::Relaxed)
                                || session.1.home().await.is_ok());
                        if !alive {
                            match open.open().await {
                                Ok(fresh) => session = fresh,
                                Err(message) => {
                                    refuse(
                                        command,
                                        &format!("Reconnecting failed: {message}"),
                                        &send,
                                    );
                                    continue;
                                }
                            }
                        }
                        tokio::spawn(run(
                            session.1.clone(),
                            command,
                            send.clone(),
                            suspect.clone(),
                            slots.clone(),
                        ));
                    }
                });
            })?;
        Ok(Self {
            commands,
            replies,
            answers,
        })
    }
}

/// Logs in again whenever the session is gone.
struct Opener {
    /// Shared with the tab, which wipes its secrets when the vault locks.
    login: Arc<Mutex<Login>>,
    answers: Answers,
    send: Send,
}

impl Opener {
    /// A connection and SFTP on it, or why not.
    async fn open(&self) -> Result<(annulus_ssh::Connection, Arc<Sftp>), String> {
        let notify = self.send.clone();
        let notify: Arc<dyn Fn(SessionEvent) + std::marker::Send + Sync> =
            Arc::new(move |event| notify(Reply::Session(event)));
        let (via, last, files) = self
            .login
            .lock()
            .map_err(|_| "The connection's login is gone.".to_owned())?
            .clone();
        let connection =
            annulus_session::connect_chain(&via, &last, &files, &self.answers, notify).await?;
        let sftp = Sftp::open(&connection)
            .await
            .map_err(|error| format!("SFTP is not available: {error}"))?;
        Ok((connection, Arc::new(sftp)))
    }
}

/// Answers `command` with `message` as its failure, without running it.
fn refuse(command: Command, message: &str, send: &Send) {
    send(match command {
        Command::List(dir, request) => Reply::ListFailed {
            dir,
            request,
            message: message.to_owned(),
        },
        Command::Download { id, .. }
        | Command::Upload { id, .. }
        | Command::UploadTree { id, .. } => Reply::Finished {
            id,
            result: Err(message.to_owned()),
        },
        Command::CreateDir(_) | Command::Rename(..) | Command::Remove(_) | Command::Chmod(..) => {
            Reply::Changed {
                error: Some(message.to_owned()),
            }
        }
    });
}

/// Transfers that run at once per tab; the others wait their turn (browsing never waits).
const TRANSFER_SLOTS: usize = 3;

/// Runs `command`; a failure marks the session `suspect`, so it is checked before the next.
/// Transfers first take one of the `slots`.
async fn run(
    sftp: Arc<Sftp>,
    command: Command,
    send: Send,
    suspect: Arc<AtomicBool>,
    slots: Arc<tokio::sync::Semaphore>,
) {
    let _slot = match &command {
        Command::Download { .. } | Command::Upload { .. } | Command::UploadTree { .. } => {
            slots.acquire_owned().await.ok()
        }
        _ => None,
    };
    // Cancelled while it waited its turn: it never opens a file.
    if let Command::Download { id, cancel, .. }
    | Command::Upload { id, cancel, .. }
    | Command::UploadTree { id, cancel, .. } = &command
        && cancel.load(Ordering::Relaxed)
    {
        return send(Reply::Finished {
            id: *id,
            result: Err(annulus_sftp::SftpError::Cancelled.to_string()),
        });
    }
    let failed = move |reply: Reply| {
        let bad = matches!(
            &reply,
            Reply::ListFailed { .. }
                | Reply::Finished { result: Err(_), .. }
                | Reply::Changed { error: Some(_) }
        );
        if bad {
            suspect.store(true, Ordering::Relaxed);
        }
        send(reply);
    };
    let send: Send = Arc::new(failed);
    let changed = |result: Result<(), annulus_sftp::SftpError>| Reply::Changed {
        error: result.err().map(|error| error.to_string()),
    };
    match command {
        Command::List(dir, request) => send(match sftp.list(&dir).await {
            Ok(entries) => Reply::Listed {
                dir,
                request,
                entries,
            },
            Err(error) => Reply::ListFailed {
                dir,
                request,
                message: error.to_string(),
            },
        }),
        Command::Download {
            id,
            remote,
            to,
            conflict,
            cancel,
        } => {
            let mut progress = progress(id, cancel, send.clone());
            let result = sftp.download(&remote, &to, conflict, &mut progress).await;
            send(Reply::Finished {
                id,
                result: result.map_err(|error| error.to_string()),
            });
        }
        Command::Upload {
            id,
            local,
            dir,
            conflict,
            cancel,
        } => {
            let mut progress = progress(id, cancel, send.clone());
            let result = sftp.upload(&local, &dir, conflict, &mut progress).await;
            send(Reply::Finished {
                id,
                result: result.map_err(|error| error.to_string()),
            });
        }
        Command::UploadTree {
            id,
            local,
            dir,
            conflict,
            cancel,
        } => {
            let mut progress = progress(id, cancel, send.clone());
            let result = sftp
                .upload_tree(&local, &dir, conflict, &mut progress)
                .await;
            send(Reply::Finished {
                id,
                result: result.map_err(|error| error.to_string()),
            });
        }
        Command::CreateDir(path) => send(changed(sftp.create_dir(&path).await)),
        Command::Rename(from, to) => send(changed(sftp.rename(&from, &to).await)),
        Command::Chmod(path, mode) => send(changed(sftp.set_permissions(&path, mode).await)),
        Command::Remove(path) => send(changed(sftp.remove(&path).await)),
    }
}

fn progress(
    id: u64,
    cancel: Arc<AtomicBool>,
    send: Send,
) -> impl FnMut(u64, u64) -> bool + std::marker::Send {
    move |done, total| {
        send(Reply::Progress { id, done, total });
        !cancel.load(Ordering::Relaxed)
    }
}

/// One transfer in the bottom panel.
pub(crate) struct Transfer {
    pub(crate) id: u64,
    pub(crate) label: String,
    pub(crate) upload: bool,
    pub(crate) done: u64,
    pub(crate) total: u64,
    pub(crate) state: TransferState,
    pub(crate) cancel: Arc<AtomicBool>,
    /// How to start it again.
    pub(crate) recipe: Recipe,
    /// When this run's first progress came, and how much was done then (a resumed run
    /// starts above zero), for its speed.
    pub(crate) since: Option<(std::time::Instant, u64)>,
}

/// What a transfer copies, so Retry can send it again.
#[derive(Clone)]
pub(crate) enum Recipe {
    Download {
        remote: String,
        to: PathBuf,
        conflict: Conflict,
    },
    Upload {
        local: PathBuf,
        dir: String,
        conflict: Conflict,
    },
    UploadTree {
        local: PathBuf,
        dir: String,
        conflict: Conflict,
    },
}

impl Recipe {
    /// The command for transfer `id`.
    fn command(&self, id: u64, cancel: Arc<AtomicBool>) -> Command {
        match self.clone() {
            Recipe::Download {
                remote,
                to,
                conflict,
            } => Command::Download {
                id,
                remote,
                to,
                conflict,
                cancel,
            },
            Recipe::Upload {
                local,
                dir,
                conflict,
            } => Command::Upload {
                id,
                local,
                dir,
                conflict,
                cancel,
            },
            Recipe::UploadTree {
                local,
                dir,
                conflict,
            } => Command::UploadTree {
                id,
                local,
                dir,
                conflict,
                cancel,
            },
        }
    }
}

pub(crate) enum TransferState {
    Running,
    Done(String),
    Skipped,
    Failed(String),
}

/// A card waiting for the user (new folders and renames are typed inline instead).
pub(crate) enum Dialog {
    /// Delete this entry from the server? It cannot be undone.
    Delete(Entry),
    /// Permission bits of `name`, typed in octal.
    Permissions { name: String, mode: String },
    /// What is known about an entry.
    Properties(Entry),
}

impl Dialog {
    /// The Permissions dialog for `entry`, filled with its current bits.
    pub(crate) fn permissions(entry: &Entry) -> Self {
        Dialog::Permissions {
            name: entry.name.clone(),
            mode: entry
                .permissions
                .map(|bits| format!("{:o}", bits & 0o7777))
                .unwrap_or_default(),
        }
    }
}

/// Where and how a tab logs in: the jump hosts, the destination, and the SSH files.
type Login = (Vec<Hop>, Hop, SshFiles);

/// One SFTP tab.
pub struct SftpTab {
    /// The tab's title.
    pub title: String,
    pub(crate) start: Option<Login>,
    /// The login the worker reconnects with, once started.
    login: Option<Arc<Mutex<Login>>>,
    pub(crate) worker: Option<Worker>,
    pub(crate) failed: Option<String>,
    /// The folder shown; `None` until connected.
    pub(crate) cwd: Option<String>,
    /// The login folder.
    pub(crate) home: String,
    /// The newest listing asked for; older replies are dropped.
    pub(crate) list_request: u64,
    pub(crate) path_field: String,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: Option<String>,
    pub(crate) list_error: Option<String>,
    pub(crate) notice: Option<String>,
    pub(crate) show_hidden: bool,
    pub(crate) filter: String,
    /// The column the listing is sorted by, and whether ascending.
    pub(crate) sort: (SortBy, bool),
    pub(crate) conflict: Conflict,
    pub(crate) transfers: Vec<Transfer>,
    pub(crate) next_id: u64,
    pub(crate) dialog: Option<Dialog>,
    /// The local folder beside the remote one; downloads land in it.
    pub(crate) local: LocalPane,
    /// Questions the connection asked (a new host key, a password), for the tab to show.
    pub(crate) questions: Vec<SessionEvent>,
}

impl SftpTab {
    /// A tab that connects to `last` (through `via`) when first shown.
    #[must_use]
    pub fn new(title: impl Into<String>, last: Hop, files: SshFiles, via: Vec<Hop>) -> Self {
        Self {
            title: title.into(),
            start: Some((via, last, files)),
            login: None,
            worker: None,
            failed: None,
            cwd: None,
            home: "/".to_owned(),
            list_request: 0,
            path_field: String::new(),
            entries: Vec::new(),
            selected: None,
            list_error: None,
            notice: None,
            show_hidden: false,
            filter: String::new(),
            sort: (SortBy::Name, true),
            conflict: Conflict::KeepBoth,
            transfers: Vec::new(),
            next_id: 0,
            dialog: None,
            local: LocalPane::new(downloads_dir()),
            questions: Vec::new(),
        }
    }

    /// A tab that only explains why SFTP cannot open.
    #[must_use]
    pub fn unavailable(title: impl Into<String>, why: &str) -> Self {
        let mut tab = Self::new(
            title,
            Hop {
                target: Target {
                    user: String::new(),
                    host: String::new(),
                    port: 22,
                },
                password: None,
                identity: None,
                proxy: None,
            },
            SshFiles {
                known_hosts: PathBuf::new(),
                key_dir: None,
            },
            Vec::new(),
        );
        tab.start = None;
        tab.failed = Some(why.to_owned());
        tab
    }

    pub(crate) fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.commands.send(command);
        }
    }

    pub(crate) fn navigate(&mut self, dir: String) {
        self.list_request += 1;
        self.send(Command::List(dir, self.list_request));
    }

    pub(crate) fn reload(&mut self) {
        if let Some(cwd) = self.cwd.clone() {
            self.navigate(cwd);
        }
    }

    /// Applies what the connection reported since the last frame.
    pub(crate) fn drain(&mut self) {
        let replies: Vec<Reply> = self
            .worker
            .as_ref()
            .map(|worker| worker.replies.try_iter().collect())
            .unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Session(SessionEvent::Failed(message)) => self.failed = Some(message),
                Reply::Session(event) => self.questions.push(event),
                Reply::Ready { home } => {
                    // Shown even if listing it fails, so the error has somewhere to appear.
                    self.cwd = Some(home.clone());
                    self.path_field.clone_from(&home);
                    self.home.clone_from(&home);
                    self.navigate(home);
                }
                Reply::Listed { request, .. } | Reply::ListFailed { request, .. }
                    if request != self.list_request => {}
                Reply::Listed { dir, entries, .. } => {
                    if self.cwd.as_deref() != Some(dir.as_str()) {
                        self.selected = None;
                    }
                    self.path_field.clone_from(&dir);
                    self.cwd = Some(dir);
                    self.entries = entries;
                    self.list_error = None;
                }
                Reply::ListFailed { dir, message, .. } => {
                    self.list_error = Some(format!("Could not open {dir}: {message}"));
                    if let Some(cwd) = &self.cwd {
                        self.path_field.clone_from(cwd);
                    }
                }
                Reply::Progress { id, done, total } => {
                    if let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) {
                        transfer
                            .since
                            .get_or_insert((std::time::Instant::now(), done));
                        transfer.done = done;
                        transfer.total = total;
                    }
                }
                Reply::Finished { id, result } => {
                    let done = matches!(result, Ok(Outcome::Done { .. }));
                    let mut upload = None;
                    if let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) {
                        transfer.state = match result {
                            Ok(Outcome::Done { to, .. }) => TransferState::Done(to),
                            Ok(Outcome::Skipped) => TransferState::Skipped,
                            Err(message) => TransferState::Failed(message),
                        };
                        upload = Some(transfer.upload);
                    }
                    // A finished transfer shows up in the listing on its receiving side.
                    match upload {
                        Some(true) if done => self.reload(),
                        Some(false) if done => self.local.refresh(self.show_hidden),
                        _ => {}
                    }
                }
                Reply::Changed { error } => {
                    self.notice = error;
                    self.reload();
                }
            }
        }
    }

    fn start_transfer(&mut self, label: String, recipe: Recipe) {
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let command = recipe.command(self.next_id, cancel.clone());
        self.transfers.push(Transfer {
            id: self.next_id,
            label,
            upload: matches!(recipe, Recipe::Upload { .. } | Recipe::UploadTree { .. }),
            done: 0,
            total: 0,
            state: TransferState::Running,
            cancel,
            recipe,
            since: None,
        });
        self.send(command);
    }

    /// Sends failed transfer `id` again; it continues from what arrived before.
    pub(crate) fn retry(&mut self, id: u64) {
        let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return;
        };
        transfer.cancel = Arc::new(AtomicBool::new(false));
        transfer.state = TransferState::Running;
        transfer.since = None;
        let command = transfer.recipe.command(id, transfer.cancel.clone());
        self.send(command);
    }

    pub(crate) fn download(&mut self, entry: &Entry) {
        let Some(cwd) = self.cwd.clone() else { return };
        let recipe = Recipe::Download {
            remote: join(&cwd, &entry.name),
            to: self.local.dir.clone(),
            conflict: self.conflict,
        };
        self.start_transfer(format!("\u{2193} {}", entry.name), recipe);
    }

    pub(crate) fn upload(&mut self, local: PathBuf) {
        let Some(dir) = self.cwd.clone() else { return };
        let name = local.file_name().map_or_else(
            || local.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        if local.is_dir() {
            let recipe = Recipe::UploadTree {
                local,
                dir,
                conflict: self.conflict,
            };
            self.start_transfer(format!("\u{2191} {name}/"), recipe);
            return;
        }
        let recipe = Recipe::Upload {
            local,
            dir,
            conflict: self.conflict,
        };
        self.start_transfer(format!("\u{2191} {name}"), recipe);
    }

    /// Starts the connection if it has not started; `wake` repaints the window.
    pub(crate) fn start_with(&mut self, wake: impl Fn() + std::marker::Send + Sync + 'static) {
        if let Some(login) = self.start.take() {
            let login = Arc::new(Mutex::new(login));
            self.login = Some(login.clone());
            match Worker::start(login, wake) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.failed = Some(format!("Could not start the connection: {error}"));
                }
            }
        }
    }

    /// Drops the login's secrets, as when the vault locks: a reconnect asks for a password.
    pub(crate) fn forget_secrets(&mut self) {
        let forget = |(via, last, _): &mut Login| {
            last.forget_secrets();
            via.iter_mut().for_each(Hop::forget_secrets);
        };
        if let Some(login) = &mut self.start {
            forget(login);
        }
        if let Some(mut login) = self.login.as_ref().and_then(|login| login.lock().ok()) {
            forget(&mut login);
        }
    }

    /// The connection's answers, for its questions.
    pub(crate) fn answers(&self) -> Option<&Answers> {
        self.worker.as_ref().map(|worker| &worker.answers)
    }
}

pub(crate) fn conflict_name(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::KeepBoth => "Existing files: keep both",
        Conflict::Overwrite => "Existing files: overwrite",
        Conflict::Skip => "Existing files: skip",
    }
}

/// A name for a new or renamed entry: not empty, no `/`, not `.` or `..`.
pub(crate) fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

/// The user's Downloads folder, else home, else the temp folder.
fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| {
            dirs.download_dir()
                .map(Path::to_path_buf)
                .or_else(|| Some(dirs.home_dir().to_path_buf()))
        })
        .unwrap_or_else(std::env::temp_dir)
}

/// "12.3 MiB/s · 0:42 left" from `moved` bytes in `seconds` with `left` bytes to go;
/// empty until there is a second's worth to measure.
pub(crate) fn pace(moved: u64, left: u64, seconds: f64) -> String {
    if seconds < 1.0 || moved == 0 {
        return String::new();
    }
    #[allow(clippy::cast_precision_loss)] // A rate for display.
    let per_second = moved as f64 / seconds;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Positive, display.
    let rate = per_second as u64;
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let eta = (left as f64 / per_second).ceil() as u64;
    let eta = if eta >= 3600 {
        format!("{}:{:02}:{:02}", eta / 3600, eta / 60 % 60, eta % 60)
    } else {
        format!("{}:{:02}", eta / 60, eta % 60)
    };
    format!("{}/s \u{b7} {eta} left", human_size(rate))
}

/// What the SFTP listing can be sorted by (F-SFTP-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortBy {
    Name,
    Size,
    Modified,
}

/// Sorts `entries` by `by`, folders always first; names break ties.
pub(crate) fn sort_entries(entries: &mut [Entry], by: SortBy, ascending: bool) {
    entries.sort_by(|a, b| {
        let order = match by {
            SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortBy::Size => a.size.cmp(&b.size),
            SortBy::Modified => a.modified.cmp(&b.modified),
        }
        .then_with(|| a.name.cmp(&b.name));
        let order = if ascending { order } else { order.reverse() };
        b.is_dir().cmp(&a.is_dir()).then(order)
    });
}

/// The Properties dialog's lines for `entry` in folder `cwd`.
pub(crate) fn properties(cwd: &str, entry: &Entry) -> Vec<(&'static str, String)> {
    let kind = match entry.kind {
        Kind::Dir => "Folder".to_owned(),
        Kind::File => "File".to_owned(),
        Kind::Link { to_dir: true } => "Link to a folder".to_owned(),
        Kind::Link { to_dir: false } => "Link".to_owned(),
        Kind::Other => "Other (device, socket or pipe)".to_owned(),
    };
    let mut lines = vec![
        ("Name", entry.name.clone()),
        ("Path", join(cwd, &entry.name)),
        ("Kind", kind),
    ];
    if !entry.is_dir() {
        lines.push((
            "Size",
            format!("{} ({} bytes)", human_size(entry.size), entry.size),
        ));
    }
    if let Some(modified) = entry.modified {
        lines.push(("Modified", format_time(modified)));
    }
    if let Some(bits) = entry.permissions {
        lines.push((
            "Permissions",
            format!("{:o} ({})", bits & 0o7777, mode_text(bits)),
        ));
    }
    lines
}

/// `0o754` as `rwxr-xr--`.
fn mode_text(bits: u32) -> String {
    (0..9)
        .map(|i| {
            let on = bits & (0o400 >> i) != 0;
            match (on, i % 3) {
                (false, _) => '-',
                (true, 0) => 'r',
                (true, 1) => 'w',
                (true, _) => 'x',
            }
        })
        .collect()
}

/// Permission bits typed in octal: three or four digits, `644` or `0755`.
pub(crate) fn parse_mode(text: &str) -> Option<u32> {
    let text = text.trim();
    if !(3..=4).contains(&text.len()) || !text.chars().all(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    u32::from_str_radix(text, 8).ok()
}

/// `1536` → `1.5 KiB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    let mut tenths = 0;
    while value >= 1024 && unit < UNITS.len() - 1 {
        tenths = (value % 1024) * 10 / 1024;
        value /= 1024;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{value}.{tenths} {}", UNITS[unit])
    }
}

/// Seconds since the Unix epoch as `YYYY-MM-DD HH:MM` (UTC).
pub(crate) fn format_time(seconds: u32) -> String {
    let days = i64::from(seconds / 86_400);
    let minutes_of_day = seconds % 86_400 / 60;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        minutes_of_day / 60,
        minutes_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Waits for the reply to transfer `id`, answering host-key questions; returns the
    /// first progress it reported and how it ended.
    fn finish(worker: &Worker, id: u64, on_progress: &mut dyn FnMut(u64)) -> (u64, Reply) {
        let mut first = None;
        loop {
            let reply = worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("a reply within a minute");
            match reply {
                Reply::Session(SessionEvent::HostKey(_)) => worker.answers.host_key(true),
                Reply::Progress { id: of, done, .. } if of == id => {
                    first.get_or_insert(done);
                    on_progress(done);
                }
                Reply::Finished { id: of, .. } if of == id => return (first.unwrap_or(0), reply),
                _ => {}
            }
        }
    }

    /// Waits until `worker` is connected; its home folder.
    fn ready(worker: Worker) -> (Worker, String) {
        loop {
            match worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap()
            {
                Reply::Session(SessionEvent::HostKey(_)) => worker.answers.host_key(true),
                Reply::Ready { home } => return (worker, home),
                Reply::Session(SessionEvent::Failed(why)) => panic!("{why}"),
                _ => {}
            }
        }
    }

    /// End to end on test VM 1: of five downloads started at once, at most
    /// [`TRANSFER_SLOTS`] run together, and all finish.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_transfers_beyond_the_slots_wait_their_turn() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let (vm1, user) = (var("ANNULUS_E2E_VM1"), var("ANNULUS_E2E_USER"));
        let ssh = |command: &str| {
            let status = std::process::Command::new("ssh")
                .args(["-o", "BatchMode=yes", &format!("{user}@{vm1}"), command])
                .status()
                .unwrap();
            assert!(status.success(), "{command}");
        };
        ssh("head -c 30000000 /dev/urandom > ~/annulus-slots.bin");
        let dir = std::env::temp_dir().join(format!("annulus-slots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hop = Hop {
            target: Target {
                host: vm1.clone(),
                port: 22,
                user: user.clone(),
            },
            password: None,
            identity: Some(annulus_session::Password::new(
                &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
            )),
            proxy: None,
        };
        let files = SshFiles {
            known_hosts: dir.join("known_hosts"),
            key_dir: None,
        };
        let login = Arc::new(Mutex::new((Vec::new(), hop, files)));
        let (worker, home) = ready(Worker::start(login, || {}).unwrap());
        let recipe = Recipe::Download {
            remote: join(&home, "annulus-slots.bin"),
            to: dir.clone(),
            conflict: Conflict::KeepBoth,
        };
        for id in 1..=5 {
            let command = recipe.command(id, Arc::new(AtomicBool::new(false)));
            worker.commands.send(command).unwrap();
        }
        let (mut running, mut most, mut finished) = (Vec::new(), 0, 0);
        while finished < 5 {
            match worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("a reply within a minute")
            {
                Reply::Progress { id, .. } if !running.contains(&id) => {
                    running.push(id);
                    most = most.max(running.len());
                }
                Reply::Finished { id, result } => {
                    assert!(result.is_ok(), "{result:?}");
                    running.retain(|other| *other != id);
                    finished += 1;
                }
                _ => {}
            }
        }
        assert!(most <= TRANSFER_SLOTS, "{most} ran at once");
        assert!(most > 1, "transfers still run side by side");
        ssh("rm -f ~/annulus-slots.bin");
    }

    /// End to end on test VM 1: the server cuts a download off; Retry through the same tab
    /// reconnects SFTP and continues where it stopped.
    #[test]
    #[ignore = "needs the test servers"]
    fn e2e_retry_after_the_server_cut_a_download_reconnects_and_resumes() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
        let (vm1, user) = (var("ANNULUS_E2E_VM1"), var("ANNULUS_E2E_USER"));
        let target = format!("{user}@{vm1}");
        let remote_shell = |command: &str| {
            let status = std::process::Command::new("ssh")
                .args(["-o", "BatchMode=yes", &target, command])
                .status()
                .unwrap();
            assert!(status.success(), "{command}");
        };
        remote_shell("head -c 150000000 /dev/urandom > ~/annulus-retry.bin");
        let key = std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap();
        let dir = std::env::temp_dir().join(format!("annulus-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hop = Hop {
            target: Target {
                host: vm1.clone(),
                port: 22,
                user: user.clone(),
            },
            password: None,
            identity: Some(annulus_session::Password::new(&key)),
            proxy: None,
        };
        let files = SshFiles {
            known_hosts: dir.join("known_hosts"),
            key_dir: None,
        };
        let login = Arc::new(Mutex::new((Vec::new(), hop, files)));
        let (worker, home) = ready(Worker::start(login, || {}).unwrap());
        let recipe = Recipe::Download {
            remote: join(&home, "annulus-retry.bin"),
            to: dir.clone(),
            conflict: Conflict::KeepBoth,
        };
        let send = |id| {
            let command = recipe.command(id, Arc::new(AtomicBool::new(false)));
            worker.commands.send(command).unwrap();
        };

        send(1);
        let mut cut = false;
        let (_, ended) = finish(&worker, 1, &mut |done| {
            if !cut && done > 1_000_000 {
                cut = true;
                remote_shell("pkill -9 -x sftp-server");
            }
        });
        assert!(
            matches!(ended, Reply::Finished { result: Err(_), .. }),
            "the cut fails it"
        );

        send(1);
        let (first, ended) = finish(&worker, 1, &mut |_| {});
        assert!(
            matches!(ended, Reply::Finished { result: Ok(_), .. }),
            "the retry works"
        );
        assert!(first > 1_000_000, "it continued instead of starting over");
        assert_eq!(
            std::fs::metadata(dir.join("annulus-retry.bin"))
                .unwrap()
                .len(),
            150_000_000
        );
        remote_shell("rm -f ~/annulus-retry.bin");
    }

    #[test]
    fn the_pace_shows_speed_and_time_left_once_measurable() {
        assert_eq!(pace(1024, 1024, 0.5), "", "under a second: no guess yet");
        assert_eq!(pace(0, 1024, 3.0), "");
        assert_eq!(
            pace(10 * 1024 * 1024, 30 * 1024 * 1024, 2.0),
            "5.0 MiB/s \u{b7} 0:06 left"
        );
        assert_eq!(
            pace(1024, 1024 * 4000, 1.0),
            "1.0 KiB/s \u{b7} 1:06:40 left"
        );
    }

    #[test]
    fn modes_are_three_or_four_octal_digits() {
        assert_eq!(parse_mode("644"), Some(0o644));
        assert_eq!(parse_mode(" 0755 "), Some(0o755));
        assert_eq!(parse_mode("4755"), Some(0o4755));
        assert_eq!(parse_mode("78"), None);
        assert_eq!(parse_mode("648"), None);
        assert_eq!(parse_mode("12345"), None);
        assert_eq!(parse_mode("rw-"), None);
    }

    #[test]
    fn listings_sort_by_name_size_or_date_with_folders_first() {
        let entry = |name: &str, dir: bool, size: u64, modified: u32| Entry {
            name: name.into(),
            kind: if dir { Kind::Dir } else { Kind::File },
            size,
            modified: Some(modified),
            permissions: None,
        };
        let mut entries = vec![
            entry("b.txt", false, 10, 3),
            entry("A.txt", false, 30, 1),
            entry("zdir", true, 0, 9),
            entry("c.txt", false, 20, 2),
        ];
        let names = |entries: &[Entry]| entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        sort_entries(&mut entries, SortBy::Name, true);
        assert_eq!(names(&entries), ["zdir", "A.txt", "b.txt", "c.txt"]);
        sort_entries(&mut entries, SortBy::Size, false);
        assert_eq!(names(&entries), ["zdir", "A.txt", "c.txt", "b.txt"]);
        sort_entries(&mut entries, SortBy::Modified, true);
        assert_eq!(names(&entries), ["zdir", "A.txt", "c.txt", "b.txt"]);
    }

    #[test]
    fn properties_spell_out_size_and_permissions() {
        let entry = Entry {
            name: "notes.txt".into(),
            kind: Kind::File,
            size: 2048,
            modified: Some(0),
            permissions: Some(0o754),
        };
        let lines = properties("/home/me", &entry);
        let get = |name: &str| {
            lines
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("Path").as_deref(), Some("/home/me/notes.txt"));
        assert_eq!(get("Size").as_deref(), Some("2.0 KiB (2048 bytes)"));
        assert_eq!(get("Permissions").as_deref(), Some("754 (rwxr-xr--)"));
        assert_eq!(mode_text(0o640), "rw-r-----");
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn times_are_shown_as_utc_dates() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_time(1_798_675_200 + 3_660), "2026-12-31 01:01");
    }

    #[test]
    fn new_names_cannot_leave_the_folder() {
        assert!(valid_name("logs"));
        assert!(!valid_name(" "));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
    }
}
