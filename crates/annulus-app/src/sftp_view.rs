//! An SFTP tab (F-SFTP-002…005): browse a host's files, download, upload by dropping
//! files on the window, make folders, rename and delete. The connection runs on its own
//! thread; the tab sends it commands and draws what comes back. After a failure the thread
//! checks the session before the next command and reconnects if it is gone, so Retry on a
//! failed transfer continues it (F-SFTP-004).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, RichText, Ui};
use annulus_config::tokens::{Palette, space, text};
use annulus_session::{Answers, Hop, SessionEvent, SshFiles, Target};
use annulus_sftp::{Conflict, Entry, Kind, Outcome, Sftp, join, parent};
use annulus_ui::{icons, theme};

use crate::local_pane::{LocalAction, LocalPane};
use crate::ssh_dialogs::SshDialogs;

/// What the tab asks the connection thread to do.
enum Command {
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
    CreateDir(String),
    Rename(String, String),
    Remove(String),
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
struct Worker {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    replies: Receiver<Reply>,
    answers: Answers,
}

impl Worker {
    fn start(
        login: Login,
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
    login: Login,
    answers: Answers,
    send: Send,
}

impl Opener {
    /// A connection and SFTP on it, or why not.
    async fn open(&self) -> Result<(annulus_ssh::Connection, Arc<Sftp>), String> {
        let notify = self.send.clone();
        let notify: Arc<dyn Fn(SessionEvent) + std::marker::Send + Sync> =
            Arc::new(move |event| notify(Reply::Session(event)));
        let (via, last, files) = &self.login;
        let connection =
            annulus_session::connect_chain(via, last, files, &self.answers, notify).await?;
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
        Command::Download { id, .. } | Command::Upload { id, .. } => Reply::Finished {
            id,
            result: Err(message.to_owned()),
        },
        Command::CreateDir(_) | Command::Rename(..) | Command::Remove(_) => Reply::Changed {
            error: Some(message.to_owned()),
        },
    });
}

/// Runs `command`; a failure marks the session `suspect`, so it is checked before the next.
async fn run(sftp: Arc<Sftp>, command: Command, send: Send, suspect: Arc<AtomicBool>) {
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
        Command::CreateDir(path) => send(changed(sftp.create_dir(&path).await)),
        Command::Rename(from, to) => send(changed(sftp.rename(&from, &to).await)),
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
struct Transfer {
    id: u64,
    label: String,
    upload: bool,
    done: u64,
    total: u64,
    state: TransferState,
    cancel: Arc<AtomicBool>,
    /// How to start it again.
    recipe: Recipe,
    /// When this run's first progress came, and how much was done then (a resumed run
    /// starts above zero), for its speed.
    since: Option<(std::time::Instant, u64)>,
}

/// What a transfer copies, so Retry can send it again.
#[derive(Clone)]
enum Recipe {
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
        }
    }
}

enum TransferState {
    Running,
    Done(String),
    Skipped,
    Failed(String),
}

/// A dialog waiting for the user.
enum Dialog {
    NewFolder(String),
    Rename { from: String, to: String },
    Delete(Entry),
}

/// Where and how a tab logs in: the jump hosts, the destination, and the SSH files.
type Login = (Vec<Hop>, Hop, SshFiles);

/// One SFTP tab.
pub struct SftpTab {
    /// The tab's title.
    pub title: String,
    start: Option<Login>,
    worker: Option<Worker>,
    dialogs: SshDialogs,
    failed: Option<String>,
    /// The folder shown; `None` until connected.
    cwd: Option<String>,
    /// The login folder.
    home: String,
    /// The newest listing asked for; older replies are dropped.
    list_request: u64,
    path_field: String,
    entries: Vec<Entry>,
    selected: Option<String>,
    list_error: Option<String>,
    notice: Option<String>,
    show_hidden: bool,
    filter: String,
    conflict: Conflict,
    transfers: Vec<Transfer>,
    next_id: u64,
    dialog: Option<Dialog>,
    /// The local folder beside the remote one; downloads land in it.
    local: LocalPane,
}

impl SftpTab {
    /// A tab that connects to `last` (through `via`) when first shown.
    #[must_use]
    pub fn new(title: impl Into<String>, last: Hop, files: SshFiles, via: Vec<Hop>) -> Self {
        Self {
            title: title.into(),
            start: Some((via, last, files)),
            worker: None,
            dialogs: SshDialogs::default(),
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
            conflict: Conflict::KeepBoth,
            transfers: Vec::new(),
            next_id: 0,
            dialog: None,
            local: LocalPane::new(downloads_dir()),
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

    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.commands.send(command);
        }
    }

    fn navigate(&mut self, dir: String) {
        self.list_request += 1;
        self.send(Command::List(dir, self.list_request));
    }

    fn reload(&mut self) {
        if let Some(cwd) = self.cwd.clone() {
            self.navigate(cwd);
        }
    }

    /// Applies what the connection reported since the last frame.
    fn drain(&mut self) {
        let replies: Vec<Reply> = self
            .worker
            .as_ref()
            .map(|worker| worker.replies.try_iter().collect())
            .unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Session(SessionEvent::Failed(message)) => self.failed = Some(message),
                Reply::Session(event) => {
                    self.dialogs.take(event);
                }
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
            upload: matches!(recipe, Recipe::Upload { .. }),
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
    fn retry(&mut self, id: u64) {
        let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return;
        };
        transfer.cancel = Arc::new(AtomicBool::new(false));
        transfer.state = TransferState::Running;
        transfer.since = None;
        let command = transfer.recipe.command(id, transfer.cancel.clone());
        self.send(command);
    }

    fn download(&mut self, entry: &Entry) {
        let Some(cwd) = self.cwd.clone() else { return };
        let recipe = Recipe::Download {
            remote: join(&cwd, &entry.name),
            to: self.local.dir.clone(),
            conflict: self.conflict,
        };
        self.start_transfer(format!("\u{2193} {}", entry.name), recipe);
    }

    fn upload(&mut self, local: PathBuf) {
        let Some(dir) = self.cwd.clone() else { return };
        let name = local.file_name().map_or_else(
            || local.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        if local.is_dir() {
            self.notice = Some(format!(
                "{name} is a folder; uploading folders is not supported yet."
            ));
            return;
        }
        let recipe = Recipe::Upload {
            local,
            dir,
            conflict: self.conflict,
        };
        self.start_transfer(format!("\u{2191} {name}"), recipe);
    }

    /// Draws the tab.
    pub fn ui(&mut self, ui: &mut Ui, palette: &Palette) {
        if let Some(login) = self.start.take() {
            let ctx = ui.ctx().clone();
            match Worker::start(login, move || {
                ctx.request_repaint();
            }) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.failed = Some(format!("Could not start the connection: {error}"));
                }
            }
        }
        self.drain();
        if let Some(worker) = &self.worker {
            self.dialogs.show(ui.ctx(), &worker.answers);
        }
        if let Some(message) = &self.failed {
            ui.colored_label(theme::color(palette.status_error), message);
            return;
        }
        if self.cwd.is_none() {
            ui.label(format!("Connecting to {}\u{2026}", self.title));
            return;
        }
        self.uploads_from_drop(ui);
        egui::Panel::top("sftp-toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("sftp-transfers")
            .resizable(true)
            .default_size(140.0)
            .show(ui, |ui| self.transfers_ui(ui, palette));
        let hidden = self.show_hidden;
        let action = egui::Panel::left("sftp-local")
            .resizable(true)
            .default_size(ui.available_width() / 2.0)
            .show(ui, |ui| self.local.ui(ui, palette, hidden))
            .inner;
        if let Some(LocalAction::Upload(path)) = action {
            self.upload(path);
        }
        egui::CentralPanel::default().show(ui, |ui| self.listing(ui, palette));
        self.dialog_ui(ui.ctx());
    }

    fn uploads_from_drop(&mut self, ui: &Ui) {
        let dropped: Vec<PathBuf> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            self.upload(path);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.add_space(space::XS);
        ui.horizontal(|ui| {
            let cwd = self.cwd.clone().unwrap_or_default();
            if ui.button("Up").on_hover_text("Parent folder").clicked() {
                self.navigate(parent(&cwd));
            }
            if ui.button("Home").clicked() {
                self.navigate(self.home.clone());
            }
            if ui.button("Refresh").clicked() {
                self.reload();
            }
            let path = ui.add(
                egui::TextEdit::singleline(&mut self.path_field)
                    .desired_width(320.0)
                    .hint_text("Path"),
            );
            if path.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                let target = self.path_field.trim().to_owned();
                if !target.is_empty() {
                    self.navigate(target);
                }
            }
            if ui.button("New folder").clicked() {
                self.dialog = Some(Dialog::NewFolder(String::new()));
            }
            ui.checkbox(&mut self.show_hidden, "Hidden files");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .desired_width(140.0)
                    .hint_text("Filter"),
            );
            egui::ComboBox::from_label("if a file exists")
                .selected_text(conflict_name(self.conflict))
                .show_ui(ui, |ui| {
                    for choice in [Conflict::KeepBoth, Conflict::Overwrite, Conflict::Skip] {
                        ui.selectable_value(&mut self.conflict, choice, conflict_name(choice));
                    }
                });
        });
        let selected = self
            .selected
            .as_ref()
            .and_then(|name| self.entries.iter().find(|entry| &entry.name == name))
            .cloned();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(selected.is_some(), |ui| {
                if ui.button("Download").clicked()
                    && let Some(entry) = &selected
                    && !entry.is_dir()
                {
                    self.download(entry);
                }
                if ui.button("Rename").clicked()
                    && let Some(entry) = &selected
                {
                    self.dialog = Some(Dialog::Rename {
                        from: entry.name.clone(),
                        to: entry.name.clone(),
                    });
                }
                if ui.button("Delete").clicked()
                    && let Some(entry) = &selected
                {
                    self.dialog = Some(Dialog::Delete(entry.clone()));
                }
            });
            ui.label(
                RichText::new("Drop files on the window to upload them here.")
                    .size(text::SECONDARY),
            );
        });
        ui.add_space(space::XS);
    }

    fn listing(&mut self, ui: &mut Ui, palette: &Palette) {
        if let Some(error) = &self.list_error {
            ui.colored_label(theme::color(palette.status_error), error);
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(theme::color(palette.status_error), notice);
        }
        let filter = self.filter.to_lowercase();
        let visible: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .filter(|entry| filter.is_empty() || entry.name.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        if visible.is_empty() {
            ui.label(
                RichText::new("This folder is empty.").color(theme::color(palette.text_secondary)),
            );
            return;
        }
        let mut open = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sftp-listing")
                .num_columns(3)
                .striped(true)
                .spacing([space::L, space::XS])
                .show(ui, |ui| {
                    for entry in &visible {
                        let selected = self.selected.as_deref() == Some(entry.name.as_str());
                        let row = ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            let icon = if entry.is_dir() {
                                icons::FOLDER
                            } else {
                                icons::FILE
                            };
                            icons::paint(
                                ui.painter(),
                                icon,
                                rect,
                                theme::color(palette.text_secondary),
                            );
                            let mut label = entry.name.clone();
                            if matches!(entry.kind, Kind::Link { .. }) {
                                label.push_str(" \u{2192}");
                            }
                            ui.selectable_label(selected, label)
                        });
                        let response = row.inner;
                        if response.clicked() {
                            self.selected = Some(entry.name.clone());
                        }
                        let activated = response.double_clicked()
                            || (response.has_focus()
                                && ui.input(|input| input.key_pressed(Key::Enter)));
                        if activated {
                            open = Some(entry.clone());
                        }
                        response.context_menu(|ui| {
                            if !entry.is_dir() && ui.button("Download").clicked() {
                                open = Some(entry.clone());
                            }
                            if ui.button("Rename").clicked() {
                                self.dialog = Some(Dialog::Rename {
                                    from: entry.name.clone(),
                                    to: entry.name.clone(),
                                });
                            }
                            if ui.button("Delete").clicked() {
                                self.dialog = Some(Dialog::Delete(entry.clone()));
                            }
                        });
                        ui.label(if entry.is_dir() {
                            String::new()
                        } else {
                            human_size(entry.size)
                        });
                        ui.label(entry.modified.map(format_time).unwrap_or_default());
                        ui.end_row();
                    }
                });
        });
        if let Some(entry) = open {
            if entry.is_dir() {
                if let Some(cwd) = &self.cwd {
                    let dir = join(cwd, &entry.name);
                    self.navigate(dir);
                }
            } else {
                self.download(&entry);
            }
        }
    }

    fn transfers_ui(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.add_space(space::XS);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Transfers").strong());
            ui.label(
                RichText::new(format!("Downloads go to {}", self.local.dir.display()))
                    .size(text::SECONDARY)
                    .color(theme::color(palette.text_secondary)),
            );
            if ui.button("Clear finished").clicked() {
                self.transfers
                    .retain(|transfer| matches!(transfer.state, TransferState::Running));
            }
        });
        let mut retry = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for transfer in self.transfers.iter().rev() {
                ui.horizontal(|ui| {
                    ui.label(&transfer.label);
                    match &transfer.state {
                        TransferState::Running => {
                            #[allow(clippy::cast_precision_loss)] // A ratio for a progress bar.
                            let fraction = if transfer.total == 0 {
                                0.0
                            } else {
                                transfer.done as f32 / transfer.total as f32
                            };
                            ui.add(egui::ProgressBar::new(fraction).desired_width(220.0).text(
                                format!(
                                    "{} of {}",
                                    human_size(transfer.done),
                                    human_size(transfer.total)
                                ),
                            ));
                            if let Some((at, from)) = transfer.since {
                                let rate = pace(
                                    transfer.done.saturating_sub(from),
                                    transfer.total.saturating_sub(transfer.done),
                                    at.elapsed().as_secs_f64(),
                                );
                                ui.label(RichText::new(rate).size(text::SECONDARY));
                            }
                            if ui.button("Cancel").clicked() {
                                transfer.cancel.store(true, Ordering::Relaxed);
                            }
                        }
                        TransferState::Done(to) => {
                            ui.label(format!("done \u{2192} {to}"));
                        }
                        TransferState::Skipped => {
                            ui.label("skipped: a file with that name exists");
                        }
                        TransferState::Failed(message) => {
                            ui.colored_label(theme::color(palette.status_error), message);
                            if ui
                                .button("Retry")
                                .on_hover_text("Continue from where it stopped")
                                .clicked()
                            {
                                retry = Some(transfer.id);
                            }
                        }
                    }
                });
            }
        });
        if let Some(id) = retry {
            self.retry(id);
        }
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let cwd = self.cwd.clone().unwrap_or_default();
        let mut close = false;
        let mut command = None;
        let window = |title: &str| {
            egui::Window::new(title.to_owned())
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        };
        match dialog {
            Dialog::NewFolder(name) => {
                window("New folder").show(ctx, |ui| {
                    let field = ui.add(egui::TextEdit::singleline(name).hint_text("Folder name"));
                    field.request_focus();
                    let enter =
                        field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if (ui.button("Create").clicked() || enter) && valid_name(name) {
                            command = Some(Command::CreateDir(join(&cwd, name.trim())));
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                });
            }
            Dialog::Rename { from, to } => {
                window("Rename").show(ctx, |ui| {
                    let field = ui.add(egui::TextEdit::singleline(to));
                    field.request_focus();
                    let enter =
                        field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if (ui.button("Rename").clicked() || enter)
                            && valid_name(to)
                            && to.trim() != from
                        {
                            command =
                                Some(Command::Rename(join(&cwd, from), join(&cwd, to.trim())));
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                });
            }
            Dialog::Delete(entry) => {
                window("Delete").show(ctx, |ui| {
                    ui.label(format!("Delete {} from the server?", entry.name));
                    if entry.kind == Kind::Dir {
                        ui.label("The folder is deleted with everything in it.");
                    }
                    ui.label("This cannot be undone.");
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            command = Some(Command::Remove(join(&cwd, &entry.name)));
                        }
                        close |= ui.button("Cancel").clicked();
                    });
                });
            }
        }
        close |= ctx.input(|input| input.key_pressed(Key::Escape));
        if let Some(command) = command {
            self.send(command);
            close = true;
        }
        if close {
            self.dialog = None;
        }
    }
}

fn conflict_name(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::KeepBoth => "Keep both",
        Conflict::Overwrite => "Overwrite",
        Conflict::Skip => "Skip",
    }
}

/// A name for a new or renamed entry: not empty, no `/`, not `.` or `..`.
fn valid_name(name: &str) -> bool {
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
fn pace(moved: u64, left: u64, seconds: f64) -> String {
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
fn format_time(seconds: u32) -> String {
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
        let worker = Worker::start((Vec::new(), hop, files), || {}).unwrap();
        let home = loop {
            match worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap()
            {
                Reply::Session(SessionEvent::HostKey(_)) => worker.answers.host_key(true),
                Reply::Ready { home } => break home,
                Reply::Session(SessionEvent::Failed(why)) => panic!("{why}"),
                _ => {}
            }
        };
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
