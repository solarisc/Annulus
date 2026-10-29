//! A live terminal in the new UI: a local shell or an SSH session from
//! `annulus-session`, drawn by our renderers. Keys go through the xterm encoder, pastes use
//! bracketed paste when the program asks for it (and a paste of several lines is confirmed
//! first when it does not), SSH questions (a new host key, a password) are answered inline
//! over the terminal, and a dropped SSH session reconnects by itself with a growing pause,
//! keeping its earlier output (F-SSH-009).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::{Program, Session, SessionEvent};
use annulus_term::alacritty_terminal::term::TermMode;
use annulus_term::find::Find;
use annulus_term::keys::{self, Mods};
use annulus_term::select::SelectionKind;
use annulus_term::{Palette, Size};
use annulus_ui::gfx::{Align, Font, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Role, Typed, Ui};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, button_width,
};

use super::ask::{self, Ask};
use super::terminal;
use crate::terminal_view::{
    Reconnect, TabStatus, carried, diagnostic, needs_confirm, next_step, one_line, paste_bytes,
};

/// One live session and what the tab shows about it.
pub struct LiveTerminal {
    program: Program,
    session: Option<Session>,
    grid: Option<(usize, usize)>,
    /// The title the program set, if any.
    pub title: Option<String>,
    failed: Option<String>,
    exited: bool,
    ask: Option<Ask>,
    unread: Arc<AtomicBool>,
    /// A drag selection in progress, from this cell.
    selecting: Option<(usize, usize)>,
    /// Wheel movement not yet a whole line.
    wheel: f32,
    find: Option<FindBar>,
    /// What the keyboard sent this frame, for broadcast to other panes.
    pub sent: Vec<Vec<u8>>,
    /// Automatic reconnects of a dropped SSH session.
    reconnect: Reconnect,
    /// Earlier output to show again once the next session starts.
    carry: Option<String>,
    /// Cell size in device pixels, for a reconnect while the tab is not drawn.
    cell: Option<(u16, u16)>,
    /// The current session has drawn something (cleared when it starts again).
    drawn: Arc<AtomicBool>,
    /// A paste of several lines waiting for a yes, since each line would run (F-TERM-006).
    held_paste: Option<String>,
}

/// The find bar of a terminal (Ctrl+Shift+F).
#[derive(Default)]
struct FindBar {
    query: String,
    search: Option<Find>,
    case: bool,
    focus: bool,
}

impl LiveTerminal {
    /// A terminal that starts `program` when first drawn.
    #[must_use]
    pub fn new(program: Program) -> Self {
        Self {
            program,
            session: None,
            grid: None,
            title: None,
            failed: None,
            exited: false,
            ask: None,
            unread: Arc::new(AtomicBool::new(false)),
            selecting: None,
            wheel: 0.0,
            find: None,
            sent: Vec::new(),
            reconnect: Reconnect::default(),
            carry: None,
            cell: None,
            drawn: Arc::new(AtomicBool::new(false)),
            held_paste: None,
        }
    }

    /// An SSH session that has not drawn anything yet: the tab says it is connecting.
    fn connecting(&self) -> bool {
        matches!(self.program, Program::Ssh { .. })
            && self.session.is_some()
            && self.failed.is_none()
            && !self.exited
            && !self.drawn.load(Ordering::Relaxed)
    }

    /// Takes in the session's news while the tab is not shown, so its dot stays current, and
    /// reconnects a dropped session when its time comes, at the size it last had. Returns
    /// the seconds until the next try, while one waits.
    pub fn poll(&mut self, now: f64, waker: Option<&Waker>) -> Option<f64> {
        self.drain(now);
        let at = self.reconnect.at?;
        if now < at {
            return Some(at - now);
        }
        self.restart();
        if let (Some(grid), Some(cell)) = (self.grid, self.cell) {
            self.spawn(grid, cell, waker, now);
        }
        None
    }

    /// Starts the program again: an SSH reconnect keeps the earlier output above a line.
    fn restart(&mut self) {
        if matches!(self.program, Program::Ssh { .. })
            && let Some(text) = self.transcript().filter(|text| !text.trim().is_empty())
        {
            self.carry = Some(carried(&text));
        }
        self.reconnect.at = None;
        self.session = None;
        self.failed = None;
        self.exited = false;
        self.ask = None;
        self.held_paste = None;
    }

    /// The connection dropped at `now`, or a reconnect failed: tries again after a pause, or
    /// gives up with the error.
    fn dropped(&mut self, now: f64, failure: Option<String>) {
        match self.reconnect.dropped(now, failure) {
            Some(error) => self.failed = Some(error),
            None => self.exited = true,
        }
    }

    /// The name the failure details give the tab.
    fn name(&self) -> String {
        self.title.clone().unwrap_or_else(|| match &self.program {
            Program::Ssh { target, .. } => format!("{}@{}", target.user, target.host),
            _ => "Local terminal".to_owned(),
        })
    }

    /// The program this terminal runs (a split pane runs it again).
    #[must_use]
    pub fn program(&self) -> Program {
        self.program.clone()
    }

    /// What the session showed, for its log; `None` if it never started.
    #[must_use]
    pub fn transcript(&self) -> Option<String> {
        self.session.as_ref().map(Session::transcript)
    }

    /// Sends bytes typed in another pane (broadcast input).
    pub fn write(&self, bytes: &[u8]) {
        if let Some(session) = &self.session {
            session.scroll_to_bottom();
            session.write(bytes.to_vec());
        }
    }

    /// The wheel scrolls back through history; in a full-screen program (vim, less) it
    /// sends arrow keys instead, as xterm does.
    fn scroll(&mut self, ui: &mut Ui<'_>, session: &Session, inner: Rect, cell_h: f32) {
        let wheel = ui.take_wheel(inner);
        if wheel == 0.0 {
            return;
        }
        self.wheel += wheel / cell_h;
        let lines = self.wheel.trunc();
        if lines == 0.0 {
            return;
        }
        self.wheel -= lines;
        #[allow(clippy::cast_possible_truncation)] // A few lines per frame.
        let lines = lines as i32;
        let mode = session.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key = if lines > 0 {
                keys::Key::Up
            } else {
                keys::Key::Down
            };
            if let Some(bytes) = keys::encode(key, Mods::default(), mode) {
                for _ in 0..lines.unsigned_abs() {
                    session.write(bytes.clone());
                }
            }
        } else {
            session.scroll(lines);
        }
    }

    /// Drag selects, a double click selects a word, Ctrl+click opens a link.
    fn mouse(
        &mut self,
        ui: &mut Ui<'_>,
        session: &Session,
        response: &annulus_ui::kit::Response,
        id: Id,
        cell_at: impl Fn(annulus_ui::gfx::Point) -> (usize, usize),
    ) {
        let Some(pointer) = ui.pointer() else { return };
        let (row, column) = cell_at(pointer);
        let ctrl = ui.modifiers().ctrl;
        if ctrl && response.hovered && session.link_at(row, column).is_some() {
            ui.set_cursor(annulus_ui::kit::Cursor::Pointer);
        } else if response.hovered {
            ui.set_cursor(annulus_ui::kit::Cursor::Text);
        }
        if response.clicked && ctrl {
            if let Some(link) = session.link_at(row, column)
                && annulus_term::links::allowed(&link.url)
            {
                annulus_platform::window::open_url(&link.url);
            }
            return;
        }
        if response.double_clicked {
            session.select_start(row, column, SelectionKind::Words);
            session.select_to(row, column);
            self.selecting = None;
            return;
        }
        if ui.dragging(id) {
            match self.selecting {
                None => {
                    session.select_start(row, column, SelectionKind::Characters);
                    self.selecting = Some((row, column));
                }
                Some(_) => session.select_to(row, column),
            }
        } else if let Some(start) = self.selecting.take()
            && start == (row, column)
        {
            // A click without a drag clears the selection.
            session.clear_selection();
        }
    }

    /// Keys for the program, and the terminal's own shortcuts.
    fn keyboard(&mut self, ui: &mut Ui<'_>) {
        let Some(session) = &self.session else { return };
        let mode = session.mode();
        self.sent.clear();
        for typed in ui.take_typed() {
            match typed {
                Typed::Key(Key::Char('c'), modifiers) if modifiers == Modifiers::CTRL_SHIFT => {
                    if let Some(text) = session.selection_text() {
                        ui.copy(text);
                    }
                }
                Typed::Key(Key::Char('f'), modifiers) if modifiers == Modifiers::CTRL_SHIFT => {
                    self.find.get_or_insert_with(FindBar::default).focus = true;
                }
                Typed::Key(Key::Backspace, modifiers) if modifiers == Modifiers::CTRL_SHIFT => {
                    session.clear_scrollback();
                }
                // Each line would run as it arrives: ask first.
                Typed::Paste(text) if needs_confirm(&text, mode) => self.held_paste = Some(text),
                typed => {
                    if let Some(bytes) = encode(&typed, mode) {
                        session.scroll_to_bottom();
                        session.clear_selection();
                        self.sent.push(bytes.clone());
                        session.write(bytes);
                    }
                }
            }
        }
    }

    fn find_bar(&mut self, ui: &mut Ui<'_>, bar: Rect) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let Some(session) = &self.session else { return };
        let t = ui.theme;
        let panel = bar.inset(0.0, 4.0);
        ui.gfx.fill_rect(panel, 10.0, t.raised);
        ui.gfx.stroke_rect(panel, 10.0, 1.0, t.line);
        let mut row = panel.inset(space::S, 4.0);
        let close = row.take_right(28.0);
        row.take_right(4.0);
        let next = row.take_right(28.0);
        row.take_right(4.0);
        let previous = row.take_right(28.0);
        row.take_right(space::S);
        let case = row.take_right(button_width(ui.gfx, "Aa", None));
        row.take_right(space::S);
        let field_id = Id::new(("find", std::ptr::from_ref(&self.program) as usize));
        if find.focus {
            ui.state.set_focus(Some(field_id), false);
            find.focus = false;
        }
        let field = ui.text_field(
            field_id,
            row,
            &mut find.query,
            FieldStyle {
                placeholder: "Find in this terminal",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        if ui
            .button(
                Id::new("find.case"),
                case,
                "Aa",
                None,
                if find.case {
                    Kind::Secondary
                } else {
                    Kind::Ghost
                },
            )
            .clicked
        {
            find.case = !find.case;
            find.search = None;
        }
        if field.changed || (find.search.is_none() && !find.query.is_empty()) {
            find.search = Find::new(&find.query, false, find.case).ok();
        }
        let back = ui
            .icon_button(
                Id::new("find.previous"),
                previous,
                icons::CHEVRON_UP,
                "Previous match",
                false,
            )
            .clicked
            || ui.key(Key::Enter, Modifiers::SHIFT);
        let forward = ui
            .icon_button(
                Id::new("find.next"),
                next,
                icons::CHEVRON_DOWN,
                "Next match",
                false,
            )
            .clicked
            || field.submitted;
        if let Some(search) = &mut find.search
            && (back || forward)
        {
            session.find_step(search, back);
        }
        let closed = ui
            .icon_button(
                Id::new("find.close"),
                close,
                icons::CLOSE,
                "Close find",
                false,
            )
            .clicked
            || (field.response.focused && ui.key(Key::Escape, Modifiers::NONE));
        if closed {
            self.find = None;
        }
    }

    /// The tab's status dot.
    #[must_use]
    pub fn status(&self) -> Option<TabStatus> {
        if self.failed.is_some() || self.reconnect.at.is_some() {
            Some(TabStatus::Error)
        } else if self.ask.is_some() {
            Some(TabStatus::Input)
        } else if self.unread.load(Ordering::Relaxed) {
            Some(TabStatus::Unread)
        } else {
            None
        }
    }

    fn start(
        &mut self,
        ui: &mut Ui<'_>,
        font: Font,
        (columns, rows): (usize, usize),
        waker: Option<&Waker>,
    ) {
        let (cell_w, cell_h) = terminal::cell_size(ui, font);
        let scale = ui.gfx.scale();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small pixels.
        let cell = ((cell_w * scale) as u16, (cell_h * scale) as u16);
        self.spawn((columns, rows), cell, waker, ui.time());
    }

    /// Starts the session in a grid of `columns` x `rows` cells `cell` device pixels large,
    /// showing any output carried over from before a reconnect.
    fn spawn(
        &mut self,
        (columns, rows): (usize, usize),
        cell: (u16, u16),
        waker: Option<&Waker>,
        now: f64,
    ) {
        let unread = self.unread.clone();
        self.drawn = Arc::new(AtomicBool::new(false));
        let drawn = self.drawn.clone();
        let waker = waker.cloned();
        let wake = move || {
            unread.store(true, Ordering::Relaxed);
            drawn.store(true, Ordering::Relaxed);
            if let Some(waker) = &waker {
                waker();
            }
        };
        match Session::spawn(
            &self.program,
            Size::new(columns, rows),
            cell.0,
            cell.1,
            wake,
        ) {
            Ok(session) => {
                if let Some(text) = self.carry.take() {
                    session.show(&text);
                }
                self.session = Some(session);
                self.reconnect.started = now;
            }
            Err(error) => self.failed = Some(format!("Could not start: {error}")),
        }
        self.grid = Some((columns, rows));
        self.cell = Some(cell);
    }

    fn drain(&mut self, now: f64) {
        let Some(session) = &self.session else { return };
        let events = session.take_events();
        let reconnects = matches!(
            self.program,
            Program::Ssh {
                reconnect: true,
                ..
            }
        );
        for event in events {
            match event {
                SessionEvent::Title(title) => self.title = Some(title),
                SessionEvent::Bell => {}
                SessionEvent::Exited => {
                    self.exited = true;
                    self.reconnect.stop();
                }
                SessionEvent::Lost if reconnects => self.dropped(now, None),
                SessionEvent::Lost => self.failed = Some("The connection dropped.".into()),
                SessionEvent::HostKey(_) | SessionEvent::Password { .. } => {
                    self.ask = Ask::from_event(event);
                }
                // A reconnect that fails waits and tries again.
                SessionEvent::Failed(message) if self.reconnect.tries > 0 => {
                    self.dropped(now, Some(message));
                }
                SessionEvent::Failed(message) => self.failed = Some(message),
            }
        }
    }

    /// Draws the terminal into `rect` and feeds it the keyboard while focused.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        rect: Rect,
        (font, palette): (Font, &Palette),
        waker: Option<&Waker>,
        id: Id,
    ) {
        let inner = rect.inset(space::L, space::M);
        let grid = terminal::grid_size(ui, inner, font);
        if self.session.is_none() && self.failed.is_none() && !self.exited {
            self.start(ui, font, grid, waker);
            ui.state.set_focus(Some(id), false);
        } else if self.grid != Some(grid)
            && let Some(session) = &self.session
        {
            let (cell_w, cell_h) = terminal::cell_size(ui, font);
            let scale = ui.gfx.scale();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small pixels.
            session.resize(
                Size::new(grid.0, grid.1),
                (cell_w * scale) as u16,
                (cell_h * scale) as u16,
            );
            self.grid = Some(grid);
        }
        if let Some(session) = &mut self.session {
            session.set_palette(palette);
        }
        if let Some(wait) = self.poll(ui.time(), waker) {
            ui.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        // The find bar takes the bottom of the tab while open.
        let mut inner = inner;
        let bar = self.find.is_some().then(|| inner.take_bottom(44.0));
        let response = ui.zone(id, inner, Role::Terminal, "Terminal");
        let (cell_w, cell_h) = terminal::cell_size(ui, font);
        let cell_at = |point: annulus_ui::gfx::Point| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let column = ((point.x - inner.x) / cell_w).max(0.0) as usize;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let row = ((point.y - inner.y) / cell_h).max(0.0) as usize;
            (
                row.min(grid.1.saturating_sub(1)),
                column.min(grid.0.saturating_sub(1)),
            )
        };
        if let Some(session) = self.session.take() {
            self.scroll(ui, &session, inner, cell_h);
            self.mouse(ui, &session, &response, id, cell_at);
            self.session = Some(session);
        }
        if response.focused && self.exited && self.failed.is_none() {
            // Enter starts an ended session again; Escape stops a waiting reconnect.
            if ui.key(Key::Enter, Modifiers::NONE) {
                self.restart();
            } else if ui.key(Key::Escape, Modifiers::NONE) {
                self.reconnect.stop();
            }
        }
        if response.focused && self.ask.is_none() && self.held_paste.is_none() {
            self.unread.store(false, Ordering::Relaxed);
            self.keyboard(ui);
        }
        if let Some(session) = &self.session {
            let screen = session.screen();
            terminal::draw(ui, inner, &screen, font, response.focused);
            if let Some(find) = &mut self.find
                && let Some(search) = &mut find.search
            {
                let t = ui.theme;
                ui.push_clip(inner);
                for mark in session.find_visible(search) {
                    #[allow(clippy::cast_precision_loss)] // Screen cells.
                    let cell = Rect::new(
                        inner.x + mark.start as f32 * cell_w,
                        inner.y + mark.row as f32 * cell_h,
                        (mark.end + 1 - mark.start) as f32 * cell_w,
                        cell_h,
                    );
                    let color = if mark.current { t.warn } else { t.accent };
                    ui.gfx.fill_rect(cell, 2.0, color.alpha(0.35));
                }
                ui.pop_clip();
            }
        }
        if self.connecting() {
            let t = ui.theme;
            let line = Rect::new(inner.x, inner.y, inner.w, 20.0);
            ui.gfx.text(
                line,
                &format!("Connecting to {}\u{2026}", self.name()),
                BODY,
                t.text2,
                Align::Start,
            );
        }
        if let Some(bar) = bar {
            self.find_bar(ui, bar);
        }
        self.ask_card(ui, rect);
        self.paste_card(ui, rect, id);
        self.ended_banner(ui, rect);
    }

    /// Asks before pasting several lines into a program that did not ask for bracketed
    /// paste: each line would run as soon as it arrives.
    fn paste_card(&mut self, ui: &mut Ui<'_>, rect: Rect, terminal: Id) {
        let Some(text) = &self.held_paste else { return };
        let t = ui.theme;
        let lines = text.lines().count().max(1);
        ui.gfx.fill_rect(rect, 0.0, t.scrim);
        ui.zone(Id::new("live.paste.backdrop"), rect, Role::Backdrop, "");
        let card = rect.centered(rect.w.min(480.0), 186.0);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        ui.strict_text(
            column.take_top(24.0),
            "Paste several lines?",
            HEADING,
            t.text,
            Align::Start,
        );
        column.take_top(space::XS);
        ui.gfx.text(
            column.take_top(20.0),
            &format!(
                "The text has {lines} lines. Each one runs as a command as soon as it is pasted."
            ),
            SMALL,
            t.text2,
            Align::Start,
        );
        column.take_top(space::L);
        let mut row = column.take_top(CONTROL);
        let paste = row.take_right(button_width(ui.gfx, "Paste", None).max(80.0));
        row.take_right(space::S);
        let one = row.take_right(button_width(ui.gfx, "Paste as one line", None));
        row.take_right(space::S);
        let cancel = row.take_right(button_width(ui.gfx, "Cancel", None));
        let mut choice = None;
        // Drawn left to right, so Tab follows the eye.
        if ui
            .button(
                Id::new("live.paste.cancel"),
                cancel,
                "Cancel",
                None,
                Kind::Ghost,
            )
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE)
        {
            choice = Some(None);
        }
        if ui
            .button(
                Id::new("live.paste.one"),
                one,
                "Paste as one line",
                None,
                Kind::Secondary,
            )
            .clicked
        {
            choice = Some(Some(one_line(text)));
        }
        if ui
            .button(
                Id::new("live.paste.go"),
                paste,
                "Paste",
                None,
                Kind::Primary,
            )
            .clicked
        {
            choice = Some(Some(text.clone()));
        }
        let Some(choice) = choice else { return };
        self.held_paste = None;
        // The keyboard goes back to the terminal.
        ui.state.set_focus(Some(terminal), false);
        if let (Some(text), Some(session)) = (choice, &self.session) {
            let bytes = paste_bytes(&text, session.mode());
            session.scroll_to_bottom();
            self.sent.push(bytes.clone());
            session.write(bytes);
        }
    }

    fn ask_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let (Some(ask), Some(session)) = (self.ask.as_mut(), &self.session) else {
            return;
        };
        if ask::card(ui, rect, ask, session.answers()) {
            self.ask = None;
        }
    }

    /// The bar over a session that ended, dropped or failed: why, what to try, and a way to
    /// start again.
    #[allow(clippy::too_many_lines)] // Three states of one bar, laid out once.
    fn ended_banner(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        if self.failed.is_none() && !self.exited {
            return;
        }
        let t = ui.theme;
        let now = ui.time();
        let error = self.failed.is_some();
        let waiting = self.reconnect.at;
        let message = match (&self.failed, waiting) {
            (Some(failed), _) => failed.clone(),
            (None, Some(at)) => format!(
                "Connection lost \u{2014} reconnecting in {:.0} s",
                (at - now).max(0.0).ceil()
            ),
            (None, None) => "The session ended. Press Enter to start it again.".into(),
        };
        let step = self.failed.as_deref().and_then(next_step);
        let height = if step.is_some() { 64.0 } else { 44.0 };
        let bar = Rect::new(
            rect.x + space::L,
            rect.b() - height - 12.0,
            rect.w - 2.0 * space::L,
            height,
        );
        ui.gfx.shadow(bar, 10.0, 14.0, 4.0, t.shadow);
        ui.gfx.fill_rect(bar, 10.0, t.raised);
        let edge = if error { t.error } else { t.line_strong };
        ui.gfx.stroke_rect(bar, 10.0, 1.0, edge);
        let mut inner = bar.inset(space::M, 0.0);
        let icon = inner.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(
            if error { icons::WARNING } else { icons::INFO },
            icon,
            if error { t.error } else { t.text2 },
        );
        inner.take_left(space::S);
        let ssh = matches!(self.program, Program::Ssh { .. });
        let again_label = match (waiting, ssh) {
            (Some(_), _) => "Reconnect now",
            (None, true) => "Reconnect",
            (None, false) => "Start again",
        };
        let again = inner.take_right(button_width(ui.gfx, again_label, Some(icons::SYNC)));
        let again = again.centered(again.w, 30.0);
        inner.take_right(space::S);
        let second = if waiting.is_some() {
            Some("Stop")
        } else if error {
            Some("Copy details")
        } else {
            None
        };
        let second_rect = second.map(|label| {
            let slot = inner.take_right(button_width(ui.gfx, label, None));
            inner.take_right(space::S);
            slot.centered(slot.w, 30.0)
        });
        match step {
            Some(step) => {
                let top = Rect::new(inner.x, inner.center().y - 19.0, inner.w, 20.0);
                let bottom = Rect::new(inner.x, inner.center().y + 1.0, inner.w, 18.0);
                ui.gfx
                    .text(top, &message, BODY_STRONG, t.text, Align::Start);
                ui.gfx.text(bottom, step, SMALL, t.text2, Align::Start);
            }
            None => {
                ui.gfx.text(
                    inner,
                    &message,
                    if error { BODY_STRONG } else { BODY },
                    t.text,
                    Align::Start,
                );
            }
        }
        if waiting.is_some() {
            ui.request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Drawn left to right, so Tab follows the eye.
        if let (Some(label), Some(slot)) = (second, second_rect)
            && ui
                .button(Id::new("live.second"), slot, label, None, Kind::Ghost)
                .clicked
        {
            if waiting.is_some() {
                self.reconnect.stop();
            } else if let Some(failed) = &self.failed {
                // For a bug report: the version, the tab and the error; no secrets.
                ui.copy(diagnostic(&self.name(), failed));
            }
        }
        if ui
            .button(
                Id::new("live.again"),
                again,
                again_label,
                Some(icons::SYNC),
                Kind::Secondary,
            )
            .clicked
        {
            self.reconnect.stop();
            self.restart();
        }
    }
}

/// The bytes one typed input sends to a program in `mode`, or `None` for input it ignores.
fn encode(typed: &Typed, mode: TermMode) -> Option<Vec<u8>> {
    match typed {
        Typed::Text(text) => Some(text.as_bytes().to_vec()),
        // Wrapped when the program asked for it, and never able to end the bracket early.
        Typed::Paste(text) => Some(paste_bytes(text, mode)),
        Typed::Key(key, modifiers) => {
            let held = Mods {
                shift: modifiers.shift,
                alt: modifiers.alt,
                ctrl: modifiers.ctrl,
            };
            let key = match key {
                Key::Enter => keys::Key::Enter,
                Key::Backspace => keys::Key::Backspace,
                Key::Tab => keys::Key::Tab,
                Key::Escape => keys::Key::Escape,
                Key::Up => keys::Key::Up,
                Key::Down => keys::Key::Down,
                Key::Left => keys::Key::Left,
                Key::Right => keys::Key::Right,
                Key::Home => keys::Key::Home,
                Key::End => keys::Key::End,
                Key::PageUp => keys::Key::PageUp,
                Key::PageDown => keys::Key::PageDown,
                Key::Delete => keys::Key::Delete,
                Key::F(n) => keys::Key::F(*n),
                // Ctrl+letter is the shell's (UI-SPEC §7); Ctrl+Shift+letter is the app's.
                Key::Char(c) if modifiers.ctrl && !modifiers.shift => keys::Key::Ctrl(*c),
                // Alt+letter sends ESC and the letter, as xterm's meta does.
                Key::Char(c) if modifiers.alt && !modifiers.ctrl => {
                    return Some(format!("\x1b{c}").into_bytes());
                }
                // Plain characters and Space arrive as text.
                Key::Char(_) | Key::Space => return None,
            };
            keys::encode(key, held, mode)
        }
    }
}

#[cfg(test)]
mod tests {
    use annulus_config::tokens::Appearance;
    use annulus_platform::cpu::{CpuCanvas, TextEngine};
    use annulus_ui::gfx::Rect;
    use annulus_ui::kit::{Event, Theme, UiState};

    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_real_shell_runs_in_the_new_terminal_and_gets_what_is_typed() {
        let program = if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: Vec::new(),
            }
        };
        let mut live = LiveTerminal::new(program);
        let mut state = UiState::default();
        let mut text = Some(TextEngine::new());
        let id = Id::new("live-test");
        let started = std::time::Instant::now();
        let mut sent = false;
        let mut seen = String::new();
        while started.elapsed() < std::time::Duration::from_secs(20) {
            let mut canvas = CpuCanvas::new(800, 400, 1.0, text.take().unwrap()).unwrap();
            let events = if live.session.is_some() && !sent {
                sent = true;
                vec![
                    Event::Text("echo got:typed".into()),
                    Event::Key(Key::Enter, Modifiers::NONE),
                ]
            } else {
                Vec::new()
            };
            state.begin_frame(&events, started.elapsed().as_secs_f64());
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                &mut state,
                Rect::new(0.0, 0.0, 800.0, 400.0),
            );
            let palette = Palette::annulus_dark();
            live.draw(
                &mut ui,
                Rect::new(0.0, 0.0, 800.0, 400.0),
                (Font::mono(13.0), &palette),
                None,
                id,
            );
            state.end_frame();
            text = Some(canvas.into_text_engine());
            if let Some(session) = &live.session {
                seen = session
                    .screen()
                    .rows
                    .iter()
                    .map(|row| row.iter().map(|cell| cell.ch).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n");
                if seen
                    .lines()
                    .any(|line| line.trim_start().starts_with("got:typed"))
                {
                    assert_eq!(live.failed, None);
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        unreachable!(
            "no output from the shell; screen:\n{seen}\nfailed: {:?}",
            live.failed
        );
    }

    fn shell() -> Program {
        if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: Vec::new(),
            }
        }
    }

    /// Draws `live` once with `events` at `time`; returns the frame's zones.
    #[allow(clippy::unwrap_used)]
    fn frame(
        live: &mut LiveTerminal,
        state: &mut UiState,
        events: &[Event],
        time: f64,
    ) -> Vec<annulus_ui::kit::Zone> {
        let mut canvas = CpuCanvas::new(800, 400, 1.0, TextEngine::new()).unwrap();
        state.begin_frame(events, time);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            state,
            Rect::new(0.0, 0.0, 800.0, 400.0),
        );
        let palette = Palette::annulus_dark();
        live.draw(
            &mut ui,
            Rect::new(0.0, 0.0, 800.0, 400.0),
            (Font::mono(13.0), &palette),
            None,
            Id::new("live-test"),
        );
        state.end_frame().zones
    }

    fn click(zones: &[annulus_ui::kit::Zone], name: &str) -> Vec<Event> {
        let zone = zones
            .iter()
            .rev()
            .find(|zone| zone.label == name)
            .unwrap_or_else(|| panic!("no zone {name:?}"));
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(annulus_ui::kit::Button::Primary),
            Event::PointerUp(annulus_ui::kit::Button::Primary),
        ]
    }

    #[test]
    fn the_dot_shows_the_most_urgent_state() {
        let mut live = LiveTerminal::new(Program::system_ssh("web"));
        assert_eq!(live.status(), None);
        live.unread.store(true, Ordering::Relaxed);
        assert_eq!(live.status(), Some(TabStatus::Unread));
        live.ask = Ask::from_event(SessionEvent::Password {
            user: "me".into(),
            host: "web".into(),
        });
        assert_eq!(live.status(), Some(TabStatus::Input));
        live.reconnect.at = Some(1.0);
        assert_eq!(live.status(), Some(TabStatus::Error));
        assert!(TabStatus::Error > TabStatus::Input && TabStatus::Input > TabStatus::Unread);
    }

    #[test]
    fn a_background_tab_reconnects_when_its_time_comes() {
        let mut live = LiveTerminal::new(Program::Command {
            program: "annulus-test-no-such-program".into(),
            args: Vec::new(),
        });
        // It ran before, at this size.
        (live.grid, live.cell) = (Some((80, 24)), Some((8, 16)));
        live.reconnect.started = -100.0;
        live.dropped(0.0, None);
        assert_eq!(
            live.poll(0.5, None),
            Some(0.5),
            "the first try waits a second"
        );
        assert_eq!(live.poll(5.0, None), None);
        assert_eq!(live.reconnect.at, None);
        assert!(
            live.session.is_some() || live.failed.is_some(),
            "it tried again without being drawn"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn an_ssh_tab_says_it_is_connecting_until_it_draws() {
        let program = Program::Ssh {
            target: annulus_session::Target {
                host: "127.0.0.1".into(),
                port: 1,
                user: "nobody".into(),
            },
            files: annulus_session::SshFiles {
                known_hosts: std::env::temp_dir().join("annulus-native-connecting-known_hosts"),
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
        let mut live = LiveTerminal::new(program);
        assert!(!live.connecting(), "nothing started yet");
        live.spawn((80, 24), (8, 16), None, 0.0);
        assert!(live.session.is_some());
        assert!(live.connecting());
        assert_eq!(live.name(), "nobody@127.0.0.1");
        live.drawn.store(true, Ordering::Relaxed);
        assert!(!live.connecting(), "output arrived");
        live.drawn.store(false, Ordering::Relaxed);
        live.failed = Some("refused".into());
        assert!(!live.connecting(), "a failure shows instead");
    }

    #[test]
    fn a_reconnect_keeps_the_earlier_output_above_a_line() {
        let mut live = LiveTerminal::new(Program::system_ssh("web"));
        live.restart();
        assert_eq!(live.carry, None, "nothing was shown yet");
        live.reconnect.at = Some(3.0);
        live.restart();
        assert_eq!(live.reconnect.at, None, "a try clears the timer");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_paste_of_several_lines_waits_for_a_yes() {
        let mut live = LiveTerminal::new(shell());
        let mut state = UiState::default();
        let started = std::time::Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        frame(&mut live, &mut state, &[], clock());
        let mode = live.session.as_ref().unwrap().mode();
        if mode.contains(TermMode::BRACKETED_PASTE) {
            // This shell brackets pastes: nothing would run, so nothing is asked.
            return;
        }
        let paste = [Event::Paste("echo one\necho two".into())];
        let mut zones = frame(&mut live, &mut state, &paste, clock());
        zones = frame(&mut live, &mut state, &[], clock())
            .into_iter()
            .chain(zones)
            .collect();
        assert!(live.held_paste.is_some(), "the paste waits");
        assert!(live.sent.is_empty(), "nothing reached the shell yet");
        frame(&mut live, &mut state, &click(&zones, "Cancel"), clock());
        assert_eq!(live.held_paste, None);
        assert!(live.sent.is_empty(), "Cancel sends nothing");
        frame(&mut live, &mut state, &paste, clock());
        let zones = frame(&mut live, &mut state, &[], clock());
        frame(
            &mut live,
            &mut state,
            &click(&zones, "Paste as one line"),
            clock(),
        );
        assert_eq!(live.held_paste, None);
        assert_eq!(live.sent, [b"echo one echo two".to_vec()]);
    }

    #[test]
    fn keys_text_and_pastes_become_the_bytes_a_program_expects() {
        let plain = TermMode::empty();
        assert_eq!(
            encode(&Typed::Text("ls".into()), plain),
            Some(b"ls".to_vec())
        );
        assert_eq!(
            encode(&Typed::Key(Key::Char('c'), Modifiers::CTRL), plain),
            Some(vec![0x03])
        );
        assert_eq!(
            encode(&Typed::Key(Key::Enter, Modifiers::NONE), plain),
            Some(b"\r".to_vec())
        );
        // Ctrl+Shift+letter is an app shortcut, never sent; plain letters come as text.
        assert_eq!(
            encode(&Typed::Key(Key::Char('t'), Modifiers::CTRL_SHIFT), plain),
            None
        );
        assert_eq!(
            encode(&Typed::Key(Key::Char('a'), Modifiers::NONE), plain),
            None
        );
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            encode(&Typed::Key(Key::Char('b'), alt), plain),
            Some(b"\x1bb".to_vec())
        );
        // A paste's line breaks become Enter, wrapped when the program asked for it.
        assert_eq!(
            encode(&Typed::Paste("a\nb".into()), plain),
            Some(b"a\rb".to_vec())
        );
        assert_eq!(
            encode(&Typed::Paste("a".into()), TermMode::BRACKETED_PASTE),
            Some(b"\x1b[200~a\x1b[201~".to_vec())
        );
        // A pasted end marker cannot close the bracket early and run what follows.
        assert_eq!(
            encode(
                &Typed::Paste("x\x1b[201~; reboot".into()),
                TermMode::BRACKETED_PASTE
            ),
            Some(b"\x1b[200~x; reboot\x1b[201~".to_vec())
        );
    }
}
