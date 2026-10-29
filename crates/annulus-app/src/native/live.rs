//! A live terminal in the new UI: a local shell or an SSH session from
//! `annulus-session`, drawn by our renderers. Keys go through the xterm encoder, pastes use
//! bracketed paste when the program asks for it, and SSH questions (a new host key, a
//! password) are answered inline over the terminal.

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
use annulus_ui::widgets::{BODY, BODY_STRONG, FieldStyle, Kind, button_width};
use zeroize::Zeroizing;

use super::ask::{self, Ask};
use super::terminal;
use crate::terminal_view::TabStatus;

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
        }
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
        if self.failed.is_some() {
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
        let (cell_w, cell_h) = ((cell_w * scale) as u16, (cell_h * scale) as u16);
        let unread = self.unread.clone();
        let waker = waker.cloned();
        let wake = move || {
            unread.store(true, Ordering::Relaxed);
            if let Some(waker) = &waker {
                waker();
            }
        };
        match Session::spawn(
            &self.program,
            Size::new(columns, rows),
            cell_w,
            cell_h,
            wake,
        ) {
            Ok(session) => self.session = Some(session),
            Err(error) => self.failed = Some(format!("Could not start: {error}")),
        }
        self.grid = Some((columns, rows));
    }

    fn drain(&mut self) {
        let Some(session) = &self.session else { return };
        for event in session.take_events() {
            match event {
                SessionEvent::Title(title) => self.title = Some(title),
                SessionEvent::Bell => {}
                SessionEvent::Exited => self.exited = true,
                SessionEvent::Lost => self.failed = Some("The connection dropped.".into()),
                SessionEvent::HostKey(question) => self.ask = Some(Ask::HostKey(question)),
                SessionEvent::Password { user, host } => {
                    self.ask = Some(Ask::Password {
                        user,
                        host,
                        secret: Zeroizing::new(String::new()),
                    });
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
        self.drain();
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
        if response.focused && self.ask.is_none() {
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
        if let Some(bar) = bar {
            self.find_bar(ui, bar);
        }
        self.ask_card(ui, rect);
        self.ended_banner(ui, rect);
    }

    fn ask_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let (Some(ask), Some(session)) = (self.ask.as_mut(), &self.session) else {
            return;
        };
        if ask::card(ui, rect, ask, session.answers()) {
            self.ask = None;
        }
    }

    fn ended_banner(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        if self.failed.is_none() && !self.exited {
            return;
        }
        let t = ui.theme;
        let error = self.failed.is_some();
        let message = self
            .failed
            .clone()
            .unwrap_or_else(|| "The session ended.".into());
        let bar = Rect::new(
            rect.x + space::L,
            rect.b() - 56.0,
            rect.w - 2.0 * space::L,
            44.0,
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
        let again = inner.take_right(button_width(ui.gfx, "Reconnect", Some(icons::SYNC)));
        let again = again.centered(again.w, 30.0);
        inner.take_right(space::S);
        ui.gfx.text(
            inner,
            &message,
            if error { BODY_STRONG } else { BODY },
            t.text,
            Align::Start,
        );
        if ui
            .button(
                Id::new("live.again"),
                again,
                "Reconnect",
                Some(icons::SYNC),
                Kind::Secondary,
            )
            .clicked
        {
            self.session = None;
            self.failed = None;
            self.exited = false;
            self.ask = None;
        }
    }
}

/// The bytes one typed input sends to a program in `mode`, or `None` for input it ignores.
fn encode(typed: &Typed, mode: TermMode) -> Option<Vec<u8>> {
    match typed {
        Typed::Text(text) => Some(text.as_bytes().to_vec()),
        Typed::Paste(text) => {
            let text = text.replace("\r\n", "\r").replace('\n', "\r");
            Some(if mode.contains(TermMode::BRACKETED_PASTE) {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.into_bytes()
            })
        }
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
    }
}
