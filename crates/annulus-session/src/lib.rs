//! A session: a program feeding a terminal (ADR-0016). Local programs run in a PTY read by
//! `alacritty_terminal`'s event loop thread; SSH sessions run on a thread with a small tokio
//! runtime driving `annulus-ssh`. Either way the UI thread writes input, resizes, answers
//! prompts and takes snapshots under the terminal lock.

#![forbid(unsafe_code)]

pub mod profiles;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use annulus_ssh::{Output, Prompt};
use annulus_term::alacritty_terminal::event::{Event, EventListener, WindowSize};
use annulus_term::alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use annulus_term::alacritty_terminal::grid::Scroll;
use annulus_term::alacritty_terminal::sync::FairMutex;
use annulus_term::alacritty_terminal::term::{Term, TermMode};
use annulus_term::alacritty_terminal::tty;
use annulus_term::alacritty_terminal::vte::ansi::Processor;
use annulus_term::find::{Find, Highlight};
use annulus_term::links::{self, Link};
use annulus_term::select::{self, SelectionKind};
use annulus_term::{Palette, Screen, Size, config, snapshot};
use zeroize::Zeroizing;

pub use annulus_ssh::{HostKeyQuestion, Target};

/// What a session tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The program set the window title.
    Title(String),
    /// The program rang the bell.
    Bell,
    /// The program exited or the connection closed; the tab shows its last output.
    Exited,
    /// SSH: the connection dropped without the program exiting (F-SSH-009).
    Lost,
    /// SSH: the server's key is new. Answer with [`Answers::host_key`].
    HostKey(HostKeyQuestion),
    /// SSH: a password is needed for `user@host`. Answer with [`Answers::password`].
    Password {
        /// Login name.
        user: String,
        /// Host.
        host: String,
    },
    /// The session could not start or connect; the message says why.
    Failed(String),
}

/// Where SSH keeps its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshFiles {
    /// Annulus's known-hosts file.
    pub known_hosts: PathBuf,
    /// The folder with the user's default keys (`~/.ssh`), if any.
    pub key_dir: Option<PathBuf>,
}

/// The program a session runs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Made once per tab and moved, never stored in bulk.
pub enum Program {
    /// The user's default shell (`$SHELL`, or PowerShell/cmd on Windows).
    DefaultShell,
    /// A specific local program with arguments.
    Command {
        /// The program to run, found on `PATH`.
        program: String,
        /// Its arguments.
        args: Vec<String>,
    },
    /// Annulus's own SSH client.
    Ssh {
        /// Where to connect.
        target: Target,
        /// Known hosts and keys.
        files: SshFiles,
        /// A saved password, tried before asking the user.
        password: Option<Password>,
        /// A keychain private key (OpenSSH text), tried before the default key files.
        identity: Option<Password>,
        /// Jump hosts to go through, in order (empty: connect directly).
        via: Vec<Hop>,
        /// A proxy to reach the first hop through (F-SSH-015).
        proxy: Option<Password>,
        /// Reconnect on its own when the connection drops (F-SSH-009).
        reconnect: bool,
        /// Environment variables for the shell (F-SSH-012).
        env: Vec<(String, String)>,
        /// A command typed into the shell once it opens (F-SSH-013).
        startup: Option<String>,
        /// The terminal type (`TERM`); empty is `xterm-256color` (F-SSH-007).
        term: String,
    },
}

/// One hop of a jump chain: where, and what to log in with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// Where to connect.
    pub target: Target,
    /// A saved password, tried before asking.
    pub password: Option<Password>,
    /// A keychain private key.
    pub identity: Option<Password>,
    /// A proxy for the first hop: `socks5://…` or `http://…` (F-SSH-015).
    pub proxy: Option<Password>,
}

impl Hop {
    /// Drops this hop's secrets: its saved password, keychain key and proxy login.
    pub fn forget_secrets(&mut self) {
        self.password = None;
        self.identity = None;
        self.proxy = None;
    }
}

/// Connects through `via`, in order, to `last` (F-SSH-014). Each hop checks its own host
/// key and logs in with its own credentials; a failure names the hop it happened at.
///
/// # Errors
/// The first hop that failed, as text for the user.
pub async fn connect_chain(
    via: &[Hop],
    last: &Hop,
    files: &SshFiles,
    answers: &Answers,
    notify: Arc<dyn Fn(SessionEvent) + Send + Sync>,
) -> Result<annulus_ssh::Connection, String> {
    let mut jump: Option<Arc<annulus_ssh::Connection>> = None;
    let hops: Vec<&Hop> = via.iter().chain(std::iter::once(last)).collect();
    for (index, hop) in hops.iter().enumerate() {
        let notify = notify.clone();
        let prompt = Arc::new(AskUi::new(
            answers.clone(),
            hop.password.clone().map(Password::into_secret),
            move |event| notify(event),
        ));
        let identities: Vec<Zeroizing<String>> = hop
            .identity
            .clone()
            .map(Password::into_secret)
            .into_iter()
            .collect();
        let (known_hosts, key_dir) = (&files.known_hosts, files.key_dir.as_deref());
        let proxy = match hop.proxy.as_ref().filter(|_| index == 0) {
            Some(url) => Some(annulus_ssh::proxy::Proxy::parse(&url.0).ok_or_else(|| {
                format!(
                    "the proxy for {} is not a socks5:// or http:// address",
                    hop.target.host
                )
            })?),
            None => None,
        };
        let result = match (jump.take(), proxy) {
            (Some(through), _) => {
                annulus_ssh::connect_via(
                    through,
                    &hop.target,
                    known_hosts,
                    key_dir,
                    &identities,
                    prompt,
                )
                .await
            }
            (None, Some(proxy)) => {
                annulus_ssh::connect_proxied(
                    &proxy,
                    &hop.target,
                    known_hosts,
                    key_dir,
                    &identities,
                    prompt,
                )
                .await
            }
            (None, None) => {
                annulus_ssh::connect(&hop.target, known_hosts, key_dir, &identities, prompt).await
            }
        };
        let connection = result.map_err(|error| {
            if hops.len() == 1 {
                error.to_string()
            } else {
                format!(
                    "{} (hop {} of {}): {error}",
                    hop.target.host,
                    index + 1,
                    hops.len()
                )
            }
        })?;
        if index + 1 == hops.len() {
            return Ok(connection);
        }
        jump = Some(Arc::new(connection));
    }
    Err("nothing to connect to".into())
}

/// A password handed to a session: wiped on drop, never shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Zeroizing<String>);

impl Password {
    /// Wraps `value`.
    #[must_use]
    pub fn new(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }

    /// The password, still wiped on drop.
    #[must_use]
    pub fn into_secret(self) -> Zeroizing<String> {
        self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password([redacted])")
    }
}

impl Program {
    /// Drops the secrets this program carries (saved passwords, the private key, proxy logins,
    /// the jump hosts' too), as when the vault locks; a new connection needs them back.
    pub fn forget_secrets(&mut self) {
        if let Program::Ssh {
            password,
            identity,
            via,
            proxy,
            ..
        } = self
        {
            *password = None;
            *identity = None;
            *proxy = None;
            via.iter_mut().for_each(Hop::forget_secrets);
        }
    }

    /// The system OpenSSH client for `target` (`user@host`, `host`, `user@host:port`), for
    /// when Annulus's own client can't be used. The options come first and `--` ends them, so
    /// a target such as `-oProxyCommand=…` (typed, imported or synced) is only ever a
    /// destination, never an option that runs a command.
    #[must_use]
    pub fn system_ssh(target: &str) -> Self {
        let (destination, port) = split_port(target.trim());
        let mut args = Vec::new();
        if let Some(port) = port {
            args.push("-p".into());
            args.push(port.to_owned());
        }
        args.push("--".into());
        args.push(destination.to_owned());
        Program::Command {
            program: "ssh".into(),
            args,
        }
    }
}

/// The current OS user name, the default SSH login.
#[must_use]
pub fn local_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

/// `host:port` splits into host and port; anything else (including IPv6 literals without
/// brackets) is a destination with no port.
fn split_port(target: &str) -> (&str, Option<&str>) {
    if let Some(rest) = target.strip_prefix('[')
        && let Some((host, port)) = rest.split_once("]:")
    {
        return (
            host,
            Some(port).filter(|port| port.chars().all(|c| c.is_ascii_digit())),
        );
    }
    match target.rsplit_once(':') {
        Some((host, port))
            if !host.contains(':')
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            (host, Some(port))
        }
        _ => (target, None),
    }
}

type Reply = Box<dyn Fn(Vec<u8>) + Send>;

/// Routes the terminal's events: replies to the program, the rest to the UI, and a wake-up
/// so the UI repaints only when there is something new.
#[derive(Clone)]
struct Listener {
    events: Sender<SessionEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    reply: Arc<Mutex<Option<Reply>>>,
}

impl Listener {
    fn send(&self, event: SessionEvent) {
        let _ = self.events.send(event);
        (self.wake)();
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                let _ = self.events.send(SessionEvent::Title(title));
            }
            Event::Bell => {
                let _ = self.events.send(SessionEvent::Bell);
            }
            Event::Exit | Event::ChildExit(_) => {
                let _ = self.events.send(SessionEvent::Exited);
            }
            // Device status and color queries: the terminal's reply goes back to the program.
            Event::PtyWrite(reply) => {
                if let Ok(sink) = self.reply.lock()
                    && let Some(sink) = sink.as_ref()
                {
                    sink(reply.into_bytes());
                }
            }
            // Wake-ups only need the repaint below. OSC 52 clipboard and other requests are
            // ignored until F-TERM settings gate them.
            _ => {}
        }
        (self.wake)();
    }
}

/// Input for an SSH session's thread.
enum SshInput {
    Data(Vec<u8>),
    Resize(Size),
    /// The tab or pane was closed: end the session and its connection.
    Close,
}

/// How input reaches the program.
enum Backend {
    Pty(EventLoopSender),
    Ssh(tokio::sync::mpsc::UnboundedSender<SshInput>),
}

/// Questions waiting for the user.
#[derive(Default)]
struct Pending {
    host_key: Option<tokio::sync::oneshot::Sender<annulus_ssh::Trust>>,
    password: Option<tokio::sync::oneshot::Sender<Option<Zeroizing<String>>>>,
}

/// Where the UI answers the questions an [`AskUi`] asked.
#[derive(Clone, Default)]
pub struct Answers(Arc<Mutex<Pending>>);

impl Answers {
    /// Answers [`SessionEvent::HostKey`]: trust and remember the new key, or stop.
    pub fn host_key(&self, trust: bool) {
        self.host_key_answer(trust.into());
    }

    /// Answers [`SessionEvent::HostKey`] with `answer`, e.g. trusting it this once.
    pub fn host_key_answer(&self, answer: annulus_ssh::Trust) {
        if let Some(reply) = self
            .0
            .lock()
            .ok()
            .and_then(|mut pending| pending.host_key.take())
        {
            let _ = reply.send(answer);
        }
    }

    /// Answers [`SessionEvent::Password`]; `None` gives up.
    pub fn password(&self, password: Option<Zeroizing<String>>) {
        if let Some(reply) = self
            .0
            .lock()
            .ok()
            .and_then(|mut pending| pending.password.take())
        {
            let _ = reply.send(password);
        }
    }
}

/// A [`Prompt`] that asks the UI with [`SessionEvent::HostKey`] and
/// [`SessionEvent::Password`] and waits for the [`Answers`]. A saved password is offered
/// once before asking.
pub struct AskUi {
    answers: Answers,
    saved: Mutex<Option<Zeroizing<String>>>,
    notify: Box<dyn Fn(SessionEvent) + Send + Sync>,
}

impl AskUi {
    /// Asks through `notify`; the UI replies on `answers`.
    pub fn new(
        answers: Answers,
        saved_password: Option<Zeroizing<String>>,
        notify: impl Fn(SessionEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            answers,
            saved: Mutex::new(saved_password),
            notify: Box::new(notify),
        }
    }
}

impl Prompt for AskUi {
    fn trust_new_host_key(
        &self,
        question: HostKeyQuestion,
    ) -> tokio::sync::oneshot::Receiver<annulus_ssh::Trust> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Ok(mut pending) = self.answers.0.lock() {
            pending.host_key = Some(tx);
        }
        (self.notify)(SessionEvent::HostKey(question));
        rx
    }

    fn password(
        &self,
        user: String,
        host: String,
    ) -> tokio::sync::oneshot::Receiver<Option<Zeroizing<String>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(saved) = self.saved.lock().ok().and_then(|mut saved| saved.take()) {
            let _ = tx.send(Some(saved));
            return rx;
        }
        if let Ok(mut pending) = self.answers.0.lock() {
            pending.password = Some(tx);
        }
        (self.notify)(SessionEvent::Password { user, host });
        rx
    }
}

/// A running session.
pub struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    backend: Backend,
    events: Receiver<SessionEvent>,
    answers: Answers,
    palette: Palette,
}

impl Session {
    /// Starts `program` in a terminal of `size`, with cells of `cell_width` by `cell_height`
    /// pixels. `wake` is called (from another thread) whenever there is something to redraw.
    ///
    /// # Errors
    /// Any OS error creating the PTY or starting a local program. SSH failures arrive as
    /// [`SessionEvent::Failed`] instead, since they happen after the tab is open.
    pub fn spawn(
        program: &Program,
        size: Size,
        cell_width: u16,
        cell_height: u16,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (events_tx, events) = channel();
        let listener = Listener {
            events: events_tx,
            wake: Arc::new(wake),
            reply: Arc::new(Mutex::new(None)),
        };
        let term = Arc::new(FairMutex::new(Term::new(config(), &size, listener.clone())));
        let answers = Answers::default();
        let backend = match program {
            Program::Ssh {
                target,
                files,
                password,
                identity,
                via,
                proxy,
                env,
                startup,
                term: term_type,
                ..
            } => Backend::Ssh(spawn_ssh(
                SshLogin {
                    last: Hop {
                        target: target.clone(),
                        password: password.clone(),
                        identity: identity.clone(),
                        proxy: proxy.clone(),
                    },
                    files: files.clone(),
                    via: via.clone(),
                    env: env.clone(),
                    startup: startup.clone(),
                    term: term_type.clone(),
                },
                size,
                term.clone(),
                listener.clone(),
                answers.clone(),
            )?),
            Program::DefaultShell | Program::Command { .. } => {
                let shell = match program {
                    Program::Command { program, args } => {
                        Some(tty::Shell::new(program.clone(), args.clone()))
                    }
                    _ => None,
                };
                let options = tty::Options {
                    shell,
                    drain_on_exit: true,
                    ..tty::Options::default()
                };
                let pty = tty::new(&options, window_size(size, cell_width, cell_height), 0)?;
                let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
                let sender = event_loop.channel();
                let replies = event_loop.channel();
                if let Ok(mut slot) = listener.reply.lock() {
                    *slot = Some(Box::new(move |bytes| {
                        let _ = replies.send(Msg::Input(Cow::Owned(bytes)));
                    }));
                }
                event_loop.spawn();
                Backend::Pty(sender)
            }
        };
        Ok(Self {
            term,
            backend,
            events,
            answers,
            palette: Palette::annulus_dark(),
        })
    }

    /// Sends input (typed text, encoded keys, pasted text) to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        match &self.backend {
            Backend::Pty(pty) => {
                let _ = pty.send(Msg::Input(Cow::Owned(bytes.into())));
            }
            Backend::Ssh(input) => {
                let _ = input.send(SshInput::Data(bytes.into()));
            }
        }
    }

    /// Resizes the terminal and tells the program.
    pub fn resize(&self, size: Size, cell_width: u16, cell_height: u16) {
        self.term.lock().resize(size);
        match &self.backend {
            Backend::Pty(pty) => {
                let _ = pty.send(Msg::Resize(window_size(size, cell_width, cell_height)));
            }
            Backend::Ssh(input) => {
                let _ = input.send(SshInput::Resize(size));
            }
        }
    }

    /// Where the UI answers [`SessionEvent::HostKey`] and [`SessionEvent::Password`].
    #[must_use]
    pub fn answers(&self) -> &Answers {
        &self.answers
    }

    /// Scrolls the view through the scrollback by `lines` (positive scrolls back in time).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Jumps the view back to the newest output, as typing does.
    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Forgets the scrollback (Ctrl+Shift+Backspace).
    pub fn clear_scrollback(&self) {
        annulus_term::clear_scrollback(&mut self.term.lock());
    }

    /// Resets the terminal: modes, colors, screen and scrollback.
    pub fn reset(&self) {
        annulus_term::reset(&mut self.term.lock());
    }

    /// Steps `find` to its next match (older text when `backwards`) and scrolls it into
    /// view; false when nothing matches.
    pub fn find_step(&self, find: &mut Find, backwards: bool) -> bool {
        find.step(&mut self.term.lock(), backwards).is_some()
    }

    /// The matches of `find` on screen.
    pub fn find_visible(&self, find: &mut Find) -> Vec<Highlight> {
        find.visible(&self.term.lock())
    }

    /// The link at screen cell (`row`, `column`), if any.
    #[must_use]
    pub fn link_at(&self, row: usize, column: usize) -> Option<Link> {
        links::link_at(&self.term.lock(), row, column)
    }

    /// Every link on the screen, top to bottom, once each.
    #[must_use]
    pub fn visible_links(&self) -> Vec<String> {
        links::visible_links(&self.term.lock())
    }

    /// Shows `text` in the terminal as if the program had printed it: the output of an
    /// earlier connection, kept across a reconnect (F-SSH-009).
    pub fn show(&self, text: &str) {
        let text = text.replace('\n', "\r\n");
        Processor::<annulus_term::alacritty_terminal::vte::ansi::StdSyncHandler>::new()
            .advance(&mut *self.term.lock(), text.as_bytes());
    }

    /// The whole text of the session, scrollback included, for a session log.
    #[must_use]
    pub fn transcript(&self) -> String {
        annulus_term::transcript::transcript(&self.term.lock())
    }

    /// Draws with `palette` from now on (a theme change in Settings).
    pub fn set_palette(&mut self, palette: &Palette) {
        if &self.palette != palette {
            self.palette = palette.clone();
        }
    }

    /// What the terminal shows now.
    #[must_use]
    pub fn screen(&self) -> Screen {
        snapshot(&self.term.lock(), &self.palette)
    }

    /// Starts a mouse selection at screen cell (`row`, `column`).
    pub fn select_start(&self, row: usize, column: usize, kind: SelectionKind) {
        select::start(&mut self.term.lock(), row, column, kind);
    }

    /// Extends the selection to screen cell (`row`, `column`).
    pub fn select_to(&self, row: usize, column: usize) {
        select::extend(&mut self.term.lock(), row, column);
    }

    /// The selected text, if any.
    #[must_use]
    pub fn selection_text(&self) -> Option<String> {
        select::text(&self.term.lock())
    }

    /// Removes the selection.
    pub fn clear_selection(&self) {
        select::clear(&mut self.term.lock());
    }

    /// The terminal's input modes (application cursor keys, bracketed paste).
    #[must_use]
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Events since the last call.
    #[must_use]
    pub fn take_events(&self) -> Vec<SessionEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Both need a word: the terminal's reply hook holds a sender of the SSH input too, so
        // the channel never closes by itself and the connection would outlive its tab.
        match &self.backend {
            Backend::Pty(pty) => {
                let _ = pty.send(Msg::Shutdown);
            }
            Backend::Ssh(input) => {
                let _ = input.send(SshInput::Close);
            }
        }
    }
}

/// Runs an SSH session on its own thread: connect (asking the UI as needed), open a shell,
/// then move bytes between the channel and the terminal until either side closes.
/// What an SSH session logs in with.
struct SshLogin {
    last: Hop,
    files: SshFiles,
    via: Vec<Hop>,
    env: Vec<(String, String)>,
    startup: Option<String>,
    term: String,
}

fn spawn_ssh(
    mut login: SshLogin,
    size: Size,
    term: Arc<FairMutex<Term<Listener>>>,
    listener: Listener,
    answers: Answers,
) -> std::io::Result<tokio::sync::mpsc::UnboundedSender<SshInput>> {
    let (input_tx, mut input) = tokio::sync::mpsc::unbounded_channel();
    let replies = input_tx.clone();
    if let Ok(mut slot) = listener.reply.lock() {
        *slot = Some(Box::new(move |bytes| {
            let _ = replies.send(SshInput::Data(bytes));
        }));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    std::thread::Builder::new()
        .name("ssh session".into())
        .spawn(move || {
            runtime.block_on(async move {
                let notifier = listener.clone();
                let notify: Arc<dyn Fn(SessionEvent) + Send + Sync> =
                    Arc::new(move |event| notifier.send(event));
                // The tab can close while this waits (on a host-key or password question):
                // then it stops, and what was typed meanwhile is kept for the shell.
                let mut early = Vec::new();
                let connection = tokio::select! {
                    result = connect_chain(&login.via, &login.last, &login.files, &answers, notify) => {
                        match result {
                            Ok(connection) => connection,
                            Err(message) => return listener.send(SessionEvent::Failed(message)),
                        }
                    }
                    () = closed(&mut input, &mut early) => return,
                };
                let columns = u32::try_from(size.columns).unwrap_or(u32::MAX);
                let rows = u32::try_from(size.rows).unwrap_or(u32::MAX);
                // Logged in: the secrets are not needed again (a reconnect is a new session).
                login.last.forget_secrets();
                login.via.iter_mut().for_each(Hop::forget_secrets);
                let mut shell = tokio::select! {
                    result = connection.shell_with_env(columns, rows, &login.term, &login.env) => {
                        match result {
                            Ok(shell) => shell,
                            Err(error) => {
                                return listener.send(SessionEvent::Failed(error.to_string()));
                            }
                        }
                    }
                    () = closed(&mut input, &mut early) => return,
                };
                // The PTY holds the typed command until the shell reads it.
                if let Some(startup) = login
                    .startup
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    && shell
                        .write(format!("{startup}\r").as_bytes())
                        .await
                        .is_err()
                {
                    return listener.send(SessionEvent::Lost);
                }
                // What was typed before the shell was there, in order.
                for message in early {
                    match message {
                        SshInput::Data(bytes) => {
                            if shell.write(&bytes).await.is_err() {
                                return listener.send(SessionEvent::Lost);
                            }
                        }
                        SshInput::Resize(size) => {
                            let columns = u32::try_from(size.columns).unwrap_or(u32::MAX);
                            let rows = u32::try_from(size.rows).unwrap_or(u32::MAX);
                            let _ = shell.resize(columns, rows).await;
                        }
                        SshInput::Close => return,
                    }
                }
                let mut parser: Processor = Processor::new();
                loop {
                    tokio::select! {
                        output = shell.next() => match output {
                            Output::Data(bytes) => {
                                parser.advance(&mut *term.lock(), &bytes);
                                (listener.wake)();
                            }
                            Output::Exited => return listener.send(SessionEvent::Exited),
                            Output::Lost => return listener.send(SessionEvent::Lost),
                        },
                        message = input.recv() => match message {
                            Some(SshInput::Data(bytes)) => {
                                if shell.write(&bytes).await.is_err() {
                                    return listener.send(SessionEvent::Lost);
                                }
                            }
                            Some(SshInput::Resize(size)) => {
                                let columns = u32::try_from(size.columns).unwrap_or(u32::MAX);
                                let rows = u32::try_from(size.rows).unwrap_or(u32::MAX);
                                let _ = shell.resize(columns, rows).await;
                            }
                            // The tab was closed.
                            Some(SshInput::Close) | None => return,
                        },
                    }
                }
            });
        })?;
    Ok(input_tx)
}

/// Resolves once the tab is closed (or its session dropped), keeping the input that arrives
/// meanwhile in `early`.
async fn closed(
    input: &mut tokio::sync::mpsc::UnboundedReceiver<SshInput>,
    early: &mut Vec<SshInput>,
) {
    loop {
        match input.recv().await {
            Some(SshInput::Close) | None => return,
            Some(other) => early.push(other),
        }
    }
}

fn window_size(size: Size, cell_width: u16, cell_height: u16) -> WindowSize {
    let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    WindowSize {
        num_lines: clamp(size.rows),
        num_cols: clamp(size.columns),
        cell_width,
        cell_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(program: &Program) -> Vec<String> {
        match program {
            Program::Command { args, .. } => args.clone(),
            _ => Vec::new(),
        }
    }

    /// A server that takes the connection and never speaks: the handshake waits as it does
    /// on an unanswered host-key question. Returns its port and a receiver that reports when
    /// the client's end of the socket closes.
    pub(crate) fn silent_server() -> (u16, std::sync::mpsc::Receiver<()>) {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (closed, closing) = channel();
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = listener.accept() {
                let mut buffer = [0_u8; 256];
                // Everything until the client hangs up (0 bytes, or an error).
                while matches!(socket.read(&mut buffer), Ok(n) if n > 0) {}
                let _ = closed.send(());
            }
        });
        (port, closing)
    }

    #[test]
    fn closing_a_tab_while_it_connects_closes_the_connection() {
        let (port, closing) = silent_server();
        let program = Program::Ssh {
            target: Target {
                host: "127.0.0.1".into(),
                port,
                user: "nobody".into(),
            },
            files: SshFiles {
                known_hosts: std::env::temp_dir().join("annulus-silent-known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: false,
            env: Vec::new(),
            startup: None,
            term: String::new(),
        };
        let session = Session::spawn(&program, Size::new(80, 24), 8, 16, || {}).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(session);
        assert!(
            closing
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok(),
            "the socket stayed open after the tab closed"
        );
    }

    #[test]
    fn secrets_never_show_in_debug_output() {
        let hop = Hop {
            target: Target {
                host: "db".into(),
                port: 22,
                user: "me".into(),
            },
            password: Some(Password::new("hunter2-password")),
            identity: Some(Password::new(
                "-----BEGIN OPENSSH PRIVATE KEY----- secret-key",
            )),
            proxy: Some(Password::new("socks5://me:proxy-secret@10.0.0.1:1080")),
        };
        let program = Program::Ssh {
            target: hop.target.clone(),
            files: SshFiles {
                known_hosts: PathBuf::from("known_hosts"),
                key_dir: None,
            },
            password: hop.password.clone(),
            identity: hop.identity.clone(),
            via: vec![hop.clone()],
            proxy: hop.proxy.clone(),
            reconnect: true,
            env: Vec::new(),
            startup: None,
            term: String::new(),
        };
        // A panic message or a log line made from these must not carry a secret (F-SEC-006).
        for text in [format!("{hop:?}"), format!("{program:?}")] {
            for secret in ["hunter2-password", "secret-key", "proxy-secret"] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
    }

    #[test]
    fn system_ssh_targets_become_arguments() {
        assert_eq!(
            args(&Program::system_ssh("admin@prod-db")),
            ["--", "admin@prod-db"]
        );
        assert_eq!(
            args(&Program::system_ssh(" admin@10.0.4.12:2222 ")),
            ["-p", "2222", "--", "admin@10.0.4.12"]
        );
        assert_eq!(
            args(&Program::system_ssh("[fe80::1]:22")),
            ["-p", "22", "--", "fe80::1"]
        );
        assert_eq!(args(&Program::system_ssh("fe80::1")), ["--", "fe80::1"]);
        assert_eq!(
            args(&Program::system_ssh("host:notaport")),
            ["--", "host:notaport"]
        );
    }

    #[test]
    fn a_target_that_looks_like_an_option_stays_a_destination() {
        // `ssh -oProxyCommand=calc x` would run calc; after `--` it is only a host name.
        let args = args(&Program::system_ssh("-oProxyCommand=calc x"));
        let end = args.iter().position(|arg| arg == "--").unwrap();
        assert!(
            args[end + 1..]
                .iter()
                .all(|arg| arg.starts_with("-oProxyCommand")),
            "{args:?}"
        );
        assert!(args[..end].iter().all(|arg| !arg.contains("ProxyCommand")));
    }

    fn wait_for(session: &Session, text: &str) -> bool {
        // Waits on real process or network output; bounded so a failure ends instead of hanging.
        for _ in 0..200 {
            let screen = session.screen();
            if screen.rows.iter().any(|row| {
                row.iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .contains(text)
            }) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_shell_runs_and_its_output_reaches_the_screen() {
        let program = if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/C".into(), "echo annulus-ok".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: vec!["-c".into(), "echo annulus-ok".into()],
            }
        };
        let session = Session::spawn(&program, Size::new(80, 24), 8, 16, || {}).unwrap();
        assert!(
            wait_for(&session, "annulus-ok"),
            "the program's output never appeared"
        );
    }

    #[test]
    fn an_unreachable_ssh_host_reports_failure_in_the_tab() {
        let files = SshFiles {
            known_hosts: std::env::temp_dir().join("annulus-session-kh"),
            key_dir: None,
        };
        // Port 1 on localhost refuses connections.
        let target = Target {
            host: "127.0.0.1".into(),
            port: 1,
            user: "nobody".into(),
        };
        let session = Session::spawn(
            &Program::Ssh {
                target,
                files,
                password: None,
                identity: None,
                via: Vec::new(),
                proxy: None,
                reconnect: false,
                env: Vec::new(),
                startup: None,
                term: String::new(),
            },
            Size::new(80, 24),
            8,
            16,
            || {},
        )
        .unwrap();
        let mut failed = false;
        for _ in 0..200 {
            if session
                .take_events()
                .iter()
                .any(|event| matches!(event, SessionEvent::Failed(_)))
            {
                failed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(failed, "no failure was reported");
    }
}
