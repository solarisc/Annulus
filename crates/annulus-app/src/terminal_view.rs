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
use annulus_term::select::SelectionKind;
use annulus_term::{Screen, Size};

use crate::ssh_dialogs::SshDialogs;

/// Default font size of terminal text, in points.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Smallest and largest terminal font size.
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=32.0;

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
        }
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
                SessionEvent::Exited => self.exited = true,
                // A bell asks for attention (the taskbar flashes) instead of making a sound.
                SessionEvent::Bell => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Informational,
                    ));
                }
                SessionEvent::HostKey(_) | SessionEvent::Password { .. } => {
                    self.dialogs.take(event);
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

        if self.session.is_none() && self.failed.is_none() {
            let ctx = ui.ctx().clone();
            match Session::spawn(&self.program, size, cell_w, cell_h, move || {
                ctx.request_repaint();
            }) {
                Ok(session) => {
                    self.session = Some(session);
                    self.size = Some((size, cell_w, cell_h));
                }
                Err(error) => self.failed = Some(format!("Could not start the session: {error}")),
            }
        }
        if let Some(message) = &self.failed {
            ui.colored_label(Color32::from_rgb(0xE8, 0x64, 0x5D), message);
            return;
        }
        let Some(session) = &self.session else { return };

        if self.size != Some((size, cell_w, cell_h)) {
            session.resize(size, cell_w, cell_h);
            self.size = Some((size, cell_w, cell_h));
        }
        self.drain_events(ui.ctx());
        if let Some(message) = &self.failed {
            ui.colored_label(Color32::from_rgb(0xE8, 0x64, 0x5D), message);
            return;
        }
        let prompting = self.prompts(ui.ctx());
        let Some(session) = &self.session else { return };

        let id = ui.scope_id().with("terminal");
        let response = ui.interact(area, id, Sense::click_and_drag());
        handle_mouse(&response, session, area.min, cell);
        // A new tab takes the keyboard, so typing works without a click first.
        if response.clicked() || !self.focused_once {
            response.request_focus();
            self.focused_once = true;
        }
        if response.has_focus() {
            // Tab, arrows and Escape belong to the program, not to egui's focus navigation.
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
            if !prompting {
                handle_input(ui, session);
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
            #[allow(clippy::cast_precision_loss)] // Row counts are small.
            let at = area.min + Vec2::new(4.0, cell.y * (size.rows.saturating_sub(1)) as f32);
            ui.painter().text(
                at,
                egui::Align2::LEFT_TOP,
                "[session ended]",
                font,
                Color32::GRAY,
            );
        }
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

/// Sends this frame's keyboard and paste input to the program.
fn handle_input(ui: &Ui, session: &Session) {
    let term_mode = session.mode();
    let events = ui.input(|input| input.events.clone());
    for event in events {
        match event {
            egui::Event::Text(text) => {
                session.scroll_to_bottom();
                session.write(text.into_bytes());
            }
            egui::Event::Paste(text) => {
                session.scroll_to_bottom();
                session.write(paste_bytes(&text, term_mode));
            }
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
                    session.scroll_to_bottom();
                    session.write(bytes);
                }
            }
            _ => {}
        }
    }
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
