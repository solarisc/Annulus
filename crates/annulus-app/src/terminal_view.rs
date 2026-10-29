//! A terminal tab: starts its session on first show, draws the screen cell by cell, and
//! turns keyboard, paste and wheel input into bytes for the program.

use eframe::egui::{
    self, Color32, EventFilter, FontId, Key as EguiKey, Pos2, Rect, Sense, Ui, Vec2,
};
use annulus_session::{Program, Session, SessionEvent};
use annulus_term::alacritty_terminal::term::TermMode;
use annulus_term::alacritty_terminal::vte::ansi::Rgb;
use annulus_term::find::{Find, Highlight};
use annulus_term::keys::{self, Key, Mods};
use annulus_term::links;
use annulus_term::select::SelectionKind;
use annulus_term::{Screen, Size};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ssh_dialogs::SshDialogs;

/// What to try next after a failed connection, by what went wrong (F-APP-007).
fn next_step(message: &str) -> Option<&'static str> {
    let message = message.to_lowercase();
    if message.contains("has changed") {
        Some(
            "If the server was reinstalled, forget its old key on the Known Hosts screen and \
             connect again. If not, do not connect: someone may be in between.",
        )
    } else if message.contains("was not trusted") {
        Some("Connect again and trust the key if its fingerprint matches the server's.")
    } else if message.contains("authentication failed") {
        Some(
            "Check the username and the key or password in the host's settings; the server's \
             accepted methods are listed above.",
        )
    } else if message.contains("proxy") {
        Some("Check the proxy address and login in the host's settings.")
    } else if message.contains("could not reach") {
        Some(
            "Check the address and port, and that the server is on and reachable from here \
             (VPN, firewall).",
        )
    } else {
        None
    }
}

/// The text "Copy details" puts on the clipboard: the app version, the tab and the error.
fn diagnostic(title: &str, message: &str) -> String {
    format!(
        "Annulus {} on {}\nConnecting to: {title}\nError: {message}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// Smallest and largest terminal font size.
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=32.0;

/// Something a pane's context menu asks the tab or the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneRequest {
    /// Split this pane to the right.
    SplitRight,
    /// Split this pane downwards.
    SplitDown,
    /// Open an SFTP tab for this pane's host.
    OpenSftp,
    /// Show the snippet panel.
    Snippets,
    /// Leave this pane out of broadcast input, or take it back in (F-WS-003).
    ToggleBroadcast,
}

/// What a tab's status dot says (F-APP-002), least to most urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TabStatus {
    /// Output arrived while the tab was not shown.
    Unread,
    /// A question (host key, password) waits for an answer.
    Input,
    /// The session failed or its connection dropped.
    Error,
}

impl TabStatus {
    /// The tooltip and accessible name of the dot.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            TabStatus::Unread => "New output",
            TabStatus::Input => "Waiting for your answer",
            TabStatus::Error => "Disconnected or failed",
        }
    }
}

/// One terminal tab.
pub struct TerminalTab {
    /// The tab's title: the target, until the program sets its own.
    pub title: String,
    program: Program,
    session: Option<Session>,
    failed: Option<String>,
    exited: bool,
    size: Option<(Size, u16, u16)>,
    focused_once: bool,
    dialogs: SshDialogs,
    find: Option<FindBar>,
    /// The text of a session that ended here, waiting to be saved as a log.
    finished: Option<String>,
    /// What the keyboard sent this frame, for broadcast to other panes.
    sent: Vec<Vec<u8>>,
    /// A context-menu choice for the tab or window to act on.
    request: Option<PaneRequest>,
    /// The connection dropped: the egui time of the next automatic try (F-SSH-009).
    lost: Option<f64>,
    /// Automatic tries in this run of drops.
    tries: u32,
    /// Earlier output to show again once the next session starts.
    carry: Option<String>,
    /// When the current session started (egui time), to tell a fresh drop from a retry.
    started: f64,
    /// Set by the session when it has something new to draw; cleared when drawn.
    unread: Arc<AtomicBool>,
    /// The current session has drawn something (cleared when it starts again).
    drawn: Arc<AtomicBool>,
    /// Broadcast input from another pane does not reach this one.
    pub skip_broadcast: bool,
}

/// Tries before an automatic reconnect gives up and shows the error.
const RECONNECT_TRIES: u32 = 8;

/// Seconds to wait before automatic reconnect `attempt` (1-based): 1, 2, 4 … up to 30.
fn backoff(attempt: u32) -> f64 {
    f64::from(1_u32 << attempt.saturating_sub(1).min(5)).min(30.0)
}

/// The earlier output with a line that marks where the new connection begins.
fn carried(transcript: &str) -> String {
    format!(
        "{}\n\u{2500}\u{2500} reconnected \u{2500}\u{2500}\n",
        transcript.trim_end()
    )
}

impl TerminalTab {
    /// A tab that will run `program` when first shown.
    #[must_use]
    pub fn new(title: impl Into<String>, program: Program) -> Self {
        Self {
            title: title.into(),
            program,
            session: None,
            failed: None,
            exited: false,
            size: None,
            focused_once: false,
            dialogs: SshDialogs::default(),
            find: None,
            finished: None,
            sent: Vec::new(),
            request: None,
            lost: None,
            tries: 0,
            carry: None,
            started: 0.0,
            unread: Arc::new(AtomicBool::new(false)),
            drawn: Arc::new(AtomicBool::new(false)),
            skip_broadcast: false,
        }
    }

    /// The status dot for this pane, if any.
    #[must_use]
    pub fn status(&self) -> Option<TabStatus> {
        if self.failed.is_some() || self.lost.is_some() {
            Some(TabStatus::Error)
        } else if self.dialogs.is_asking() {
            Some(TabStatus::Input)
        } else if self.unread.load(Ordering::Relaxed) {
            Some(TabStatus::Unread)
        } else {
            None
        }
    }

    /// Takes in the session's news while the tab is not shown, so its dot stays current.
    /// A dropped connection reconnects here too, at the size it last had, so a tab in the
    /// background is back by the time it is shown.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.drain_events(ctx);
        let Some(at) = self.lost else { return };
        let now = ctx.input(|input| input.time);
        if now < at {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(at - now));
            return;
        }
        let size = self.size;
        self.restart();
        if let Some(size) = size {
            self.spawn(ctx, size);
        }
    }

    /// Starts the session in a terminal of `size` (cells, and cell width and height in
    /// pixels), showing any output carried over from before a reconnect.
    fn spawn(&mut self, ctx: &egui::Context, size: (Size, u16, u16)) {
        let (cells, cell_w, cell_h) = size;
        let wake_ctx = ctx.clone();
        let unread = self.unread.clone();
        self.drawn = Arc::new(AtomicBool::new(false));
        let drawn = self.drawn.clone();
        match Session::spawn(&self.program, cells, cell_w, cell_h, move || {
            unread.store(true, Ordering::Relaxed);
            drawn.store(true, Ordering::Relaxed);
            wake_ctx.request_repaint();
        }) {
            Ok(session) => {
                if let Some(text) = self.carry.take() {
                    session.show(&text);
                }
                self.session = Some(session);
                self.size = Some(size);
                self.started = ctx.input(|input| input.time);
            }
            Err(error) => self.failed = Some(format!("Could not start the session: {error}")),
        }
    }

    /// The connection dropped (or a reconnect failed): schedules the next try, or gives up
    /// after [`RECONNECT_TRIES`]. A drop a minute after the last connect starts a new run.
    fn schedule_reconnect(&mut self, now: f64, failure: Option<String>) {
        if failure.is_none() && now - self.started >= 60.0 {
            self.tries = 0;
        }
        self.tries += 1;
        if self.tries > RECONNECT_TRIES {
            self.tries = 0;
            self.lost = None;
            self.failed = Some(failure.unwrap_or_else(|| "The connection keeps dropping.".into()));
            return;
        }
        self.exited = true;
        self.lost = Some(now + backoff(self.tries));
    }

    /// Applies what the session reported since the last frame.
    fn drain_events(&mut self, ctx: &egui::Context) {
        let events = self
            .session
            .as_ref()
            .map(Session::take_events)
            .unwrap_or_default();
        for event in events {
            match event {
                SessionEvent::Title(title) if !title.trim().is_empty() => self.title = title,
                SessionEvent::Exited => {
                    self.exited = true;
                    (self.lost, self.tries) = (None, 0);
                    self.finished = self.transcript();
                }
                SessionEvent::Lost
                    if !matches!(
                        self.program,
                        Program::Ssh {
                            reconnect: true,
                            ..
                        }
                    ) =>
                {
                    self.exited = true;
                    self.finished = self.transcript();
                }
                SessionEvent::Lost => {
                    if self.tries == 0 {
                        self.finished = self.transcript();
                    }
                    self.schedule_reconnect(ctx.input(|input| input.time), None);
                }
                // A bell asks for attention (the taskbar flashes) instead of making a sound.
                SessionEvent::Bell => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Informational,
                    ));
                }
                SessionEvent::HostKey(_) | SessionEvent::Password { .. } => {
                    self.dialogs.take(event);
                }
                // A reconnect that fails waits and tries again.
                SessionEvent::Failed(message) if self.tries > 0 => {
                    self.schedule_reconnect(ctx.input(|input| input.time), Some(message));
                }
                SessionEvent::Failed(message) => self.failed = Some(message),
                SessionEvent::Title(_) => {}
            }
        }
    }

    /// Opens the find bar on Ctrl+Shift+F and draws it while open.
    fn find_bar(&mut self, ui: &mut Ui) {
        let open = ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, EguiKey::F)
        });
        if open {
            self.find.get_or_insert_with(FindBar::default).focus = true;
        }
        if let (Some(bar), Some(session)) = (&mut self.find, &self.session)
            && bar.ui(ui, session)
        {
            self.find = None;
            // The keyboard goes back to the terminal.
            self.focused_once = false;
        }
    }

    /// Draws the tab and handles its input.
    pub fn ui(&mut self, ui: &mut Ui, font_size: f32) {
        let font = FontId::monospace(font_size);
        let cell = Vec2::new(
            ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M')).max(1.0),
            ui.fonts_mut(|fonts| fonts.row_height(&font)).max(1.0),
        );
        self.find_bar(ui);
        let area = ui.available_rect_before_wrap();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Floored, small.
        let size = Size::new(
            (area.width() / cell.x).floor() as usize,
            (area.height() / cell.y).floor() as usize,
        );
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Cell sizes are small.
        let (cell_w, cell_h) = (cell.x.ceil() as u16, cell.y.ceil() as u16);

        // Shown now, so nothing here is unread.
        self.unread.store(false, Ordering::Relaxed);
        if self.session.is_none() && self.failed.is_none() {
            self.spawn(ui.ctx(), (size, cell_w, cell_h));
        }
        if self.failed.is_some() {
            self.failed_ui(ui);
            return;
        }
        let Some(session) = &self.session else { return };

        if self.size != Some((size, cell_w, cell_h)) {
            session.resize(size, cell_w, cell_h);
            self.size = Some((size, cell_w, cell_h));
        }
        self.drain_events(ui.ctx());
        if self.failed.is_some() {
            self.failed_ui(ui);
            return;
        }
        let prompting = self.prompts(ui.ctx());
        let Some(session) = &self.session else { return };

        let id = ui.scope_id().with("terminal");
        let response = ui.interact(area, id, Sense::click_and_drag());
        handle_mouse(&response, session, area.min, cell);
        let (request, open_find) = context_menu(
            &response,
            session,
            matches!(self.program, Program::Ssh { .. }),
            self.skip_broadcast,
        );
        if request.is_some() {
            self.request = request;
        }
        if open_find {
            self.find.get_or_insert_with(FindBar::default).focus = true;
        }
        // A new tab takes the keyboard, so typing works without a click first.
        if response.clicked() || !self.focused_once {
            response.request_focus();
            self.focused_once = true;
        }
        if self.ended_ui(ui, &response) {
            return;
        }
        let Some(session) = &self.session else { return };
        if response.has_focus() {
            keep_keys(ui, id);
            if !prompting {
                self.sent = handle_input(ui, session);
            }
        }
        // Ctrl+wheel zooms (handled by the shell); a plain wheel scrolls back.
        if response.hovered() && !ui.input(|input| input.modifiers.ctrl) {
            let scroll = ui.input(|input| input.smooth_scroll_delta().y);
            if scroll.abs() > 0.0 {
                #[allow(clippy::cast_possible_truncation)] // A few lines per wheel step.
                session.scroll((scroll / cell.y).round() as i32);
            }
        }

        paint(
            ui,
            area.min,
            cell,
            &session.screen(),
            &font,
            response.has_focus(),
        );
        if let Some(search) = self.find.as_mut().and_then(|bar| bar.search.as_mut()) {
            paint_matches(ui, area.min, cell, &session.find_visible(search));
        }
        if self.exited {
            let note = self.ended_note(ui.input(|input| input.time));
            paint_ended(ui, area.min, cell, size.rows, font, &note);
        } else if self.connecting() {
            paint_ended(
                ui,
                area.min,
                cell,
                1,
                font,
                &format!("Connecting to {}\u{2026}", self.title),
            );
        }
    }

    /// An SSH session that has not drawn anything yet: the line says it is connecting.
    fn connecting(&self) -> bool {
        matches!(self.program, Program::Ssh { .. })
            && self.session.is_some()
            && self.failed.is_none()
            && !self.exited
            && !self.drawn.load(Ordering::Relaxed)
    }

    /// A finished session starts again on Enter (an SSH tab reconnects); a dropped one also
    /// on its own when its time comes, and Escape stops that. True when it restarted.
    fn ended_ui(&mut self, ui: &Ui, response: &egui::Response) -> bool {
        if self.exited && response.has_focus() {
            if ui.input(|input| input.key_pressed(EguiKey::Enter)) {
                self.restart();
                return true;
            }
            if ui.input(|input| input.key_pressed(EguiKey::Escape)) {
                (self.lost, self.tries) = (None, 0);
            }
        }
        if let Some(at) = self.lost {
            let now = ui.input(|input| input.time);
            if now >= at {
                self.restart();
                return true;
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(at - now));
        }
        false
    }

    /// The line shown under a session that ended or dropped.
    fn ended_note(&self, now: f64) -> String {
        match self.lost {
            Some(at) => format!(
                "[connection lost \u{2014} reconnecting in {:.0} s; Enter now, Esc to stop]",
                (at - now).max(0.0).ceil()
            ),
            None => "[session ended \u{2014} press Enter to start it again]".to_owned(),
        }
    }

    /// The session's text so far, if it has any.
    #[must_use]
    pub fn transcript(&self) -> Option<String> {
        self.session
            .as_ref()
            .map(Session::transcript)
            .filter(|text| !text.trim().is_empty())
    }

    /// The context-menu choice made this frame (taken once).
    pub fn take_request(&mut self) -> Option<PaneRequest> {
        self.request.take()
    }

    /// What the keyboard sent this frame (taken once).
    pub fn take_sent(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.sent)
    }

    /// Sends bytes to the program as if typed here (broadcast from another pane).
    pub fn write(&self, bytes: &[u8]) {
        if let Some(session) = &self.session {
            session.scroll_to_bottom();
            session.write(bytes.to_vec());
        }
    }

    /// The text of a session that ended, once: the caller saves it as a log.
    pub fn take_finished(&mut self) -> Option<String> {
        self.finished.take()
    }

    /// Gives this pane the keyboard on its next frame.
    pub fn take_focus(&mut self) {
        self.focused_once = false;
    }

    /// What this pane runs.
    #[must_use]
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Starts the program again in this tab: a reconnect for SSH (which keeps the earlier
    /// output above a separator), a new shell otherwise.
    fn restart(&mut self) {
        if matches!(self.program, Program::Ssh { .. })
            && let Some(text) = self.transcript()
        {
            self.carry = Some(carried(&text));
        }
        self.lost = None;
        self.session = None;
        self.failed = None;
        self.exited = false;
        self.size = None;
        self.focused_once = false;
    }

    /// The reason a session could not start, and a way to try again.
    fn failed_ui(&mut self, ui: &mut Ui) {
        if let Some(message) = &self.failed {
            ui.colored_label(Color32::from_rgb(0xE8, 0x64, 0x5D), message);
            if let Some(step) = next_step(message) {
                ui.label(step);
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Try again").clicked() {
                self.restart();
            }
            if let Some(message) = &self.failed
                && ui
                    .button("Copy details")
                    .on_hover_text("For a bug report; it holds no passwords or keys")
                    .clicked()
            {
                ui.ctx().copy_text(diagnostic(&self.title, message));
            }
        });
    }

    /// Draws with `palette` (the terminal theme from Settings).
    pub fn set_palette(&mut self, palette: &annulus_term::Palette) {
        if let Some(session) = &mut self.session {
            session.set_palette(palette);
        }
    }

    /// Sends a snippet to the program: typed and run, or pasted at the prompt.
    pub fn send_snippet(&mut self, snippet: &annulus_vault::snippets::Snippet, run: bool) {
        let Some(session) = &self.session else { return };
        session.scroll_to_bottom();
        if run {
            session.write(snippet.input(true));
        } else {
            let text = String::from_utf8_lossy(&snippet.input(false)).into_owned();
            session.write(paste_bytes(&text, session.mode()));
        }
        // The keyboard goes back to the terminal.
        self.focused_once = false;
    }

    /// Shows a pending SSH question; true while one is open.
    fn prompts(&mut self, ctx: &egui::Context) -> bool {
        match &self.session {
            Some(session) => self.dialogs.show(ctx, session.answers()),
            None => false,
        }
    }
}

/// The find bar over a terminal (F-TERM-007), opened with Ctrl+Shift+F.
#[derive(Default)]
struct FindBar {
    query: String,
    regex: bool,
    case_sensitive: bool,
    search: Option<Find>,
    /// Why the query is not a valid regex, or that nothing matches.
    message: Option<String>,
    /// Move the keyboard to the query field on the next frame.
    focus: bool,
}

impl FindBar {
    /// Draws the bar and runs its actions; true when it was closed.
    fn ui(&mut self, ui: &mut Ui, session: &Session) -> bool {
        let mut closed = false;
        let mut step = None;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Find")
                    .desired_width(240.0),
            );
            if std::mem::take(&mut self.focus) {
                field.request_focus();
            }
            let mut changed = field.changed();
            changed |= ui.checkbox(&mut self.regex, "Regex").changed();
            changed |= ui
                .checkbox(&mut self.case_sensitive, "Match case")
                .changed();
            if changed {
                self.rebuild();
            }
            if field.lost_focus() && ui.input(|input| input.key_pressed(EguiKey::Enter)) {
                step = Some(!ui.input(|input| input.modifiers.shift));
                self.focus = true;
            }
            if ui
                .button("Previous")
                .on_hover_text("Older text (Enter)")
                .clicked()
            {
                step = Some(true);
            }
            if ui
                .button("Next")
                .on_hover_text("Newer text (Shift+Enter)")
                .clicked()
            {
                step = Some(false);
            }
            if ui.button("Close").clicked() || ui.input(|input| input.key_pressed(EguiKey::Escape))
            {
                closed = true;
            }
            if let Some(message) = &self.message {
                ui.label(message);
            }
        });
        if let (Some(backwards), Some(search)) = (step, self.search.as_mut()) {
            self.message = (!session.find_step(search, backwards)).then(|| "No matches".to_owned());
        }
        closed
    }

    fn rebuild(&mut self) {
        self.search = None;
        self.message = None;
        if self.query.is_empty() {
            return;
        }
        match Find::new(&self.query, self.regex, self.case_sensitive) {
            Ok(search) => self.search = Some(search),
            Err(error) => self.message = Some(format!("Not a valid regex: {error}")),
        }
    }
}

/// Tints the cells of each match; the current one stronger.
#[allow(clippy::cast_precision_loss)] // Grid coordinates are small.
fn paint_matches(ui: &Ui, origin: Pos2, cell: Vec2, highlights: &[Highlight]) {
    for highlight in highlights {
        let min = origin
            + Vec2::new(
                cell.x * highlight.start as f32,
                cell.y * highlight.row as f32,
            );
        let width = cell.x * (highlight.end + 1 - highlight.start) as f32;
        let alpha = if highlight.current { 150 } else { 70 };
        ui.painter().rect_filled(
            Rect::from_min_size(min, Vec2::new(width, cell.y)),
            0.0,
            Color32::from_rgba_unmultiplied(0xF2, 0xC1, 0x4E, alpha),
        );
    }
}

/// The line that says the session ended, on the last row.
#[allow(clippy::cast_precision_loss)] // Row counts are small.
fn paint_ended(ui: &Ui, origin: Pos2, cell: Vec2, rows: usize, font: FontId, note: &str) {
    let at = origin + Vec2::new(4.0, cell.y * (rows.saturating_sub(1)) as f32);
    ui.painter()
        .text(at, egui::Align2::LEFT_TOP, note, font, Color32::GRAY);
}

/// The right-click menu of a terminal (UI-SPEC: Copy, Paste, Find, Split, Snippets, Open
/// SFTP here). Returns a choice for the tab or window, and whether Find was chosen.
fn context_menu(
    response: &egui::Response,
    session: &Session,
    is_ssh: bool,
    skipped: bool,
) -> (Option<PaneRequest>, bool) {
    let mut chosen = None;
    let mut open_find = false;
    response.context_menu(|ui| {
        let selection = session.selection_text();
        if ui
            .add_enabled(selection.is_some(), egui::Button::new("Copy"))
            .clicked()
        {
            if let Some(text) = selection {
                ui.ctx().copy_text(text);
            }
            ui.close();
        }
        if ui.button("Paste").clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            ui.close();
        }
        if ui.button("Find\u{2026}").clicked() {
            open_find = true;
            ui.close();
        }
        ui.separator();
        for (label, request) in [
            ("Split right", PaneRequest::SplitRight),
            ("Split down", PaneRequest::SplitDown),
            ("Snippets", PaneRequest::Snippets),
        ] {
            if ui.button(label).clicked() {
                chosen = Some(request);
                ui.close();
            }
        }
        if is_ssh && ui.button("Open SFTP here").clicked() {
            chosen = Some(PaneRequest::OpenSftp);
            ui.close();
        }
        let broadcast = if skipped {
            "Take into broadcast"
        } else {
            "Leave out of broadcast"
        };
        if ui.button(broadcast).clicked() {
            chosen = Some(PaneRequest::ToggleBroadcast);
            ui.close();
        }
    });
    (chosen, open_find)
}

/// Tab, arrows and Escape belong to the program, not to egui's focus navigation.
fn keep_keys(ui: &Ui, id: egui::Id) {
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            id,
            EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });
}

fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

#[allow(clippy::cast_precision_loss)] // Grid coordinates are small.
fn paint(ui: &Ui, origin: Pos2, cell: Vec2, screen: &Screen, font: &FontId, focused: bool) {
    let painter = ui.painter();
    let total = Rect::from_min_size(
        origin,
        Vec2::new(
            cell.x * screen.columns as f32,
            cell.y * screen.rows.len() as f32,
        ),
    );
    painter.rect_filled(total, 0.0, color(screen.background));
    for (row_index, row) in screen.rows.iter().enumerate() {
        let y = origin.y + cell.y * row_index as f32;
        for (column, glyph) in row.iter().enumerate() {
            let x = origin.x + cell.x * column as f32;
            if glyph.bg != screen.background {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, y), cell),
                    0.0,
                    color(glyph.bg),
                );
            }
            let is_cursor = screen.cursor == Some((row_index, column));
            if is_cursor {
                let block = Rect::from_min_size(Pos2::new(x, y), cell);
                if focused {
                    painter.rect_filled(block, 0.0, color(screen.cursor_color));
                } else {
                    painter.rect_stroke(
                        block,
                        0.0,
                        egui::Stroke::new(1.0, color(screen.cursor_color)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            if glyph.spacer || glyph.ch == ' ' {
                continue;
            }
            let ink = if is_cursor && focused {
                color(screen.background)
            } else {
                color(glyph.fg)
            };
            painter.text(
                Pos2::new(x, y),
                egui::Align2::LEFT_TOP,
                glyph.ch,
                font.clone(),
                ink,
            );
            if glyph.underline {
                let base = y + cell.y - 2.0;
                painter.line_segment(
                    [Pos2::new(x, base), Pos2::new(x + cell.x, base)],
                    egui::Stroke::new(1.0, ink),
                );
            }
        }
    }
}

/// Drag selects characters, a double-click words and a triple-click lines; a plain click
/// clears the selection.
fn handle_mouse(response: &egui::Response, session: &Session, origin: Pos2, cell: Vec2) {
    let Some(pointer) = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos())
    else {
        return;
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to the grid below.
    let (row, column) = (
        ((pointer.y - origin.y) / cell.y).max(0.0) as usize,
        ((pointer.x - origin.x) / cell.x).max(0.0) as usize,
    );
    // Ctrl+click (Cmd+click on macOS) opens a link; holding the key shows where it points.
    if response.hovered()
        && response.ctx.input(|input| input.modifiers.command)
        && let Some(link) = session.link_at(row, column)
    {
        let openable = links::allowed(&link.url);
        response.ctx.set_cursor_icon(if openable {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::NotAllowed
        });
        let hint = if openable {
            format!("Open {}", link.url)
        } else {
            format!("{} (this kind of link is not opened)", link.url)
        };
        response.clone().on_hover_text(hint);
        if response.clicked() && openable {
            response.ctx.open_url(egui::OpenUrl::new_tab(link.url));
        }
        return;
    }
    if response.triple_clicked() {
        session.select_start(row, column, SelectionKind::Lines);
        session.select_to(row, column);
    } else if response.double_clicked() {
        session.select_start(row, column, SelectionKind::Words);
        session.select_to(row, column);
    } else if response.drag_started() {
        session.select_start(row, column, SelectionKind::Characters);
    } else if response.dragged() {
        session.select_to(row, column);
    } else if response.clicked() {
        session.clear_selection();
    }
}

/// Sends this frame's keyboard and paste input to the program, and returns what was sent
/// (for broadcast to other panes).
fn handle_input(ui: &Ui, session: &Session) -> Vec<Vec<u8>> {
    let term_mode = session.mode();
    let events = ui.input(|input| input.events.clone());
    let mut sent = Vec::new();
    let mut emit = |bytes: Vec<u8>| {
        session.scroll_to_bottom();
        sent.push(bytes.clone());
        session.write(bytes);
    };
    for event in events {
        match event {
            egui::Event::Text(text) => emit(text.into_bytes()),
            egui::Event::Paste(text) => emit(paste_bytes(&text, term_mode)),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Ctrl+Shift+V pastes and Ctrl+Shift+C copies (UI-SPEC §7): egui reports
                // those as Paste/Copy events, so the raw keys are not sent.
                // Ctrl+Shift+C copies; Ctrl+C copies when there is a selection and otherwise
                // interrupts the program. Ctrl+Shift+V arrives as a Paste event.
                if modifiers.ctrl && key == EguiKey::C {
                    if let Some(selected) = session.selection_text() {
                        ui.ctx().copy_text(selected);
                        session.clear_selection();
                        continue;
                    }
                    if modifiers.shift {
                        continue;
                    }
                }
                if modifiers.ctrl && modifiers.shift {
                    continue;
                }
                let mods = Mods {
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                    ctrl: modifiers.ctrl,
                };
                if let Some(key) = map_key(key, modifiers.ctrl)
                    && let Some(bytes) = keys::encode(key, mods, term_mode)
                {
                    emit(bytes);
                }
            }
            _ => {}
        }
    }
    sent
}

/// Pasted text, wrapped in bracketed-paste markers when the program asked for them, so a
/// pasted line is never run by accident.
fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A paste that contains the end marker could break out of the bracket; drop it.
        let safe = normalized.replace("\x1b[201~", "");
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

fn map_key(key: EguiKey, ctrl: bool) -> Option<Key> {
    Some(match key {
        EguiKey::Enter => Key::Enter,
        EguiKey::Backspace => Key::Backspace,
        EguiKey::Tab => Key::Tab,
        EguiKey::Escape => Key::Escape,
        EguiKey::ArrowUp => Key::Up,
        EguiKey::ArrowDown => Key::Down,
        EguiKey::ArrowLeft => Key::Left,
        EguiKey::ArrowRight => Key::Right,
        EguiKey::Home => Key::Home,
        EguiKey::End => Key::End,
        EguiKey::PageUp => Key::PageUp,
        EguiKey::PageDown => Key::PageDown,
        EguiKey::Insert => Key::Insert,
        EguiKey::Delete => Key::Delete,
        EguiKey::F1 => Key::F(1),
        EguiKey::F2 => Key::F(2),
        EguiKey::F3 => Key::F(3),
        EguiKey::F4 => Key::F(4),
        EguiKey::F5 => Key::F(5),
        EguiKey::F6 => Key::F(6),
        EguiKey::F7 => Key::F(7),
        EguiKey::F8 => Key::F(8),
        EguiKey::F9 => Key::F(9),
        EguiKey::F10 => Key::F(10),
        EguiKey::F11 => Key::F(11),
        EguiKey::F12 => Key::F(12),
        other if ctrl => Key::Ctrl(
            other
                .symbol_or_name()
                .chars()
                .next()
                .filter(|_| other.symbol_or_name().chars().count() == 1)?,
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dot_shows_the_most_urgent_state() {
        let mut tab = TerminalTab::new("web", Program::system_ssh("web"));
        assert_eq!(tab.status(), None);
        tab.unread.store(true, Ordering::Relaxed);
        assert_eq!(tab.status(), Some(TabStatus::Unread));
        tab.dialogs.take(SessionEvent::Password {
            user: "me".into(),
            host: "web".into(),
        });
        assert_eq!(tab.status(), Some(TabStatus::Input));
        tab.lost = Some(1.0);
        assert_eq!(tab.status(), Some(TabStatus::Error));
        assert!(TabStatus::Error > TabStatus::Input && TabStatus::Input > TabStatus::Unread);
    }

    #[test]
    fn a_background_tab_reconnects_when_its_time_comes() {
        let mut tab = TerminalTab::new(
            "gone",
            Program::Command {
                program: "annulus-test-no-such-program".into(),
                args: Vec::new(),
            },
        );
        tab.size = Some((Size::new(80, 24), 8, 16));
        tab.started = -100.0;
        tab.schedule_reconnect(0.0, None);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| tab.poll(ui.ctx()));
        output.textures_delta.clear();
        assert!(tab.lost.is_some(), "not yet: the first try waits a second");
        let later = egui::RawInput {
            time: Some(5.0),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(later, |ui| tab.poll(ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(tab.lost, None);
        assert!(
            tab.session.is_some() || tab.failed.is_some(),
            "it tried again without being shown"
        );
    }

    #[test]
    fn an_ssh_tab_says_it_is_connecting_until_it_draws() {
        let target = annulus_session::Target {
            host: "127.0.0.1".into(),
            port: 1,
            user: "nobody".into(),
        };
        let dir = std::env::temp_dir();
        let program = Program::Ssh {
            target,
            files: annulus_session::SshFiles {
                known_hosts: dir.join("annulus-connecting-known_hosts"),
                key_dir: None,
            },
            password: None,
            identity: None,
            via: Vec::new(),
            proxy: None,
            reconnect: false,
            env: Vec::new(),
            startup: None,
        };
        let mut tab = TerminalTab::new("db", program);
        assert!(!tab.connecting(), "nothing started yet");
        tab.session = Some(Session::spawn(&tab.program, Size::new(80, 24), 8, 16, || {}).unwrap());
        assert!(tab.connecting());
        tab.drawn.store(true, Ordering::Relaxed);
        assert!(!tab.connecting(), "output arrived");
        tab.drawn.store(false, Ordering::Relaxed);
        tab.failed = Some("refused".into());
        assert!(!tab.connecting(), "a failure shows instead");
    }

    #[test]
    fn failures_come_with_a_next_step_and_copyable_details() {
        let changed = "web: the host key of web has changed (SHA256:x); the connection was stopped";
        assert!(next_step(changed).unwrap().contains("Known Hosts"));
        assert!(
            next_step("authentication failed (tried: password; the server accepts: publickey)")
                .unwrap()
                .contains("username")
        );
        assert!(
            next_step("could not reach db:22: refused")
                .unwrap()
                .contains("port")
        );
        assert!(
            next_step("could not reach the proxy: refused")
                .unwrap()
                .contains("proxy")
        );
        assert_eq!(next_step("something else"), None);
        let details = diagnostic("db", "could not reach db:22: refused");
        assert!(details.starts_with("Annulus "));
        assert!(details.contains("Connecting to: db\nError: could not reach db:22: refused"));
    }

    #[test]
    fn reconnects_back_off_up_to_half_a_minute() {
        let waits: Vec<f64> = (1..=8).map(backoff).collect();
        assert_eq!(waits, [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0]);
    }

    #[test]
    fn a_dropped_connection_retries_then_gives_up_with_the_error() {
        let mut tab = TerminalTab::new("web", Program::system_ssh("web"));
        tab.started = 100.0;
        tab.schedule_reconnect(500.0, None);
        assert_eq!((tab.lost, tab.tries), (Some(501.0), 1));
        tab.restart();
        assert_eq!(tab.lost, None, "a try clears the timer");
        for _ in 2..=RECONNECT_TRIES {
            tab.schedule_reconnect(600.0, Some("refused".into()));
        }
        assert_eq!(tab.lost, Some(630.0));
        tab.schedule_reconnect(700.0, Some("refused".into()));
        assert_eq!((tab.lost, tab.tries), (None, 0));
        assert_eq!(tab.failed.as_deref(), Some("refused"));
    }

    #[test]
    fn a_drop_long_after_connecting_starts_the_backoff_over() {
        let mut tab = TerminalTab::new("web", Program::system_ssh("web"));
        tab.tries = 5;
        tab.started = 100.0;
        tab.schedule_reconnect(200.0, None);
        assert_eq!((tab.lost, tab.tries), (Some(201.0), 1));
    }

    #[test]
    fn kept_output_ends_with_a_reconnected_line() {
        assert_eq!(
            carried("$ ls\nfile\n\n"),
            "$ ls\nfile\n\u{2500}\u{2500} reconnected \u{2500}\u{2500}\n"
        );
    }

    #[test]
    fn bracketed_paste_wraps_and_cannot_be_escaped() {
        let pasted = paste_bytes("ls\nrm -rf x\x1b[201~; reboot", TermMode::BRACKETED_PASTE);
        assert_eq!(pasted, b"\x1b[200~ls\rrm -rf x; reboot\x1b[201~");
        assert_eq!(paste_bytes("a\r\nb", TermMode::empty()), b"a\rb");
    }

    #[test]
    fn ctrl_letters_map_to_control_keys_and_plain_letters_are_text() {
        assert_eq!(map_key(EguiKey::C, true), Some(Key::Ctrl('C')));
        assert_eq!(map_key(EguiKey::C, false), None);
        assert_eq!(map_key(EguiKey::ArrowUp, false), Some(Key::Up));
    }
}
