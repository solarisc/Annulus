//! A live terminal in the new UI: a local shell or an SSH session from
//! `annulus-session`, drawn by our renderers. Keys go through the xterm encoder, pastes use
//! bracketed paste when the program asks for it, and SSH questions (a new host key, a
//! password) are answered inline over the terminal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use annulus_config::tokens::space;
use annulus_platform::window::Waker;
use annulus_session::{HostKeyQuestion, Program, Session, SessionEvent};
use annulus_ssh::Trust;
use annulus_term::alacritty_terminal::term::TermMode;
use annulus_term::keys::{self, Mods};
use annulus_term::{Palette, Size};
use annulus_ui::gfx::{Align, Font, Rect};
use annulus_ui::icons;
use annulus_ui::kit::{Id, Key, Modifiers, Role, Typed, Ui};
use annulus_ui::widgets::{
    BODY, BODY_STRONG, CONTROL, FieldStyle, HEADING, Kind, SMALL, button_width,
};
use zeroize::Zeroizing;

use super::terminal;
use crate::terminal_view::TabStatus;

/// A question the session is waiting on.
enum Ask {
    HostKey(HostKeyQuestion),
    Password {
        user: String,
        host: String,
        secret: Zeroizing<String>,
    },
}

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
        let response = ui.zone(id, inner, Role::Terminal, "Terminal");
        if response.focused && self.ask.is_none() {
            self.unread.store(false, Ordering::Relaxed);
            if let Some(session) = &self.session {
                let mode = session.mode();
                for typed in ui.take_typed() {
                    if let Some(bytes) = encode(&typed, mode) {
                        session.write(bytes);
                    }
                }
            }
        }
        if let Some(session) = &self.session {
            let screen = session.screen();
            terminal::draw(ui, inner, &screen, font, response.focused);
        }
        self.ask_card(ui, rect);
        self.ended_banner(ui, rect);
    }

    #[allow(clippy::too_many_lines)] // Both questions' cards, each top to bottom.
    fn ask_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let Some(ask) = self.ask.as_mut() else { return };
        let Some(session) = &self.session else { return };
        let t = ui.theme;
        let card = rect.centered(rect.w.min(460.0), 236.0);
        ui.gfx.fill_rect(rect, 0.0, t.scrim);
        ui.zone(Id::new("live.ask.backdrop"), rect, Role::Backdrop, "");
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        let mut answered = false;
        match ask {
            Ask::HostKey(question) => {
                ui.strict_text(
                    column.take_top(24.0),
                    &format!("First connection to {}", question.host),
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::XS);
                ui.gfx.text(
                    column.take_top(20.0),
                    "Check this key with the server's owner before trusting it.",
                    SMALL,
                    t.text2,
                    Align::Start,
                );
                column.take_top(space::M);
                let key = column.take_top(52.0);
                ui.gfx.fill_rect(key, 8.0, t.field);
                let mut lines = key.inset(space::M, space::S);
                ui.gfx.text(
                    lines.take_top(18.0),
                    &question.algorithm,
                    SMALL,
                    t.text2,
                    Align::Start,
                );
                ui.gfx.text(
                    lines.take_top(18.0),
                    &question.fingerprint,
                    Font::mono(12.0),
                    t.text,
                    Align::Start,
                );
                column.take_top(space::L);
                let mut row = column.take_top(CONTROL);
                let trust_w = button_width(ui.gfx, "Trust and connect", None);
                let once_w = button_width(ui.gfx, "Connect once", None);
                let cancel_w = button_width(ui.gfx, "Cancel", None);
                let cancel = row.take_left(cancel_w);
                let trust = row.take_right(trust_w);
                row.take_right(space::S);
                let once = row.take_right(once_w);
                if ui
                    .button(Id::new("live.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE)
                {
                    session.answers().host_key_answer(Trust::Reject);
                    answered = true;
                }
                if ui
                    .button(
                        Id::new("live.once"),
                        once,
                        "Connect once",
                        None,
                        Kind::Secondary,
                    )
                    .clicked
                {
                    session.answers().host_key_answer(Trust::Once);
                    answered = true;
                }
                if ui
                    .button(
                        Id::new("live.trust"),
                        trust,
                        "Trust and connect",
                        None,
                        Kind::Primary,
                    )
                    .clicked
                {
                    session.answers().host_key_answer(Trust::Save);
                    answered = true;
                }
            }
            Ask::Password { user, host, secret } => {
                ui.strict_text(
                    column.take_top(24.0),
                    &format!("Password for {user}@{host}"),
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::XS);
                ui.gfx.text(
                    column.take_top(20.0),
                    "Used for this connection only; it is not saved.",
                    SMALL,
                    t.text2,
                    Align::Start,
                );
                column.take_top(space::L);
                let field = ui.text_field(
                    Id::new("live.password"),
                    column.take_top(CONTROL),
                    secret,
                    FieldStyle {
                        placeholder: "Password",
                        icon: Some(icons::KEY),
                        ..FieldStyle::default()
                    },
                );
                if ui.state.focus().is_none() {
                    ui.state.set_focus(Some(Id::new("live.password")), false);
                }
                column.take_top(space::XL);
                let mut row = column.take_top(CONTROL);
                let connect = row.take_right(button_width(ui.gfx, "Connect", None).max(96.0));
                row.take_right(space::S);
                let cancel = row.take_right(button_width(ui.gfx, "Cancel", None));
                if ui
                    .button(Id::new("live.cancel"), cancel, "Cancel", None, Kind::Ghost)
                    .clicked
                    || ui.key(Key::Escape, Modifiers::NONE)
                {
                    session.answers().password(None);
                    answered = true;
                }
                if ui
                    .button(
                        Id::new("live.connect"),
                        connect,
                        "Connect",
                        None,
                        Kind::Primary,
                    )
                    .clicked
                    || field.submitted
                {
                    session.answers().password(Some(std::mem::replace(
                        secret,
                        Zeroizing::new(String::new()),
                    )));
                    answered = true;
                }
            }
        }
        if answered {
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
