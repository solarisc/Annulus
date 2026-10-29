//! Our widgets, drawn from the theme (UI-SPEC §8): buttons, text fields with our own editor,
//! check boxes, switches, drop-down menus, scroll views, chips and section labels. Each takes
//! the rectangle it fills; views lay rectangles out with [`Rect`]'s cutting helpers and the
//! `*_width` measurements here.

use zeroize::Zeroizing;

use crate::gfx::{Align, Canvas, Color, Font, Point, Rect, Weight};
use crate::icons::{self, IconId};
use crate::kit::{Cursor, Id, Key, Modifiers, Response, Role, Typed, Ui};
use annulus_config::tokens::{metrics, radius, space, text};

/// Body text.
pub const BODY: Font = Font::ui(text::BODY);
/// Body text, medium weight: buttons, list titles.
pub const BODY_MEDIUM: Font = Font::ui(text::BODY).weight(Weight::Medium);
/// Body text, semibold: emphasis.
pub const BODY_STRONG: Font = Font::ui(text::BODY).weight(Weight::Semibold);
/// Secondary text.
pub const SMALL: Font = Font::ui(text::SECONDARY);
/// Section labels above groups of rows.
pub const LABEL: Font = Font::ui(11.0).weight(Weight::Semibold);
/// Section headings.
pub const HEADING: Font = Font::ui(text::HEADING).weight(Weight::Semibold);
/// Page titles.
pub const TITLE: Font = Font::ui(20.0).weight(Weight::Semibold);

/// Height of buttons and fields.
pub const CONTROL: f32 = metrics::CONTROL_HEIGHT;
/// Height of one menu entry.
const MENU_ITEM: f32 = 30.0;

/// How a button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The one main action of a screen: filled with the accent.
    Primary,
    /// Other actions: a quiet outlined button.
    Secondary,
    /// Toolbar actions: no background until hovered.
    Ghost,
    /// Destructive actions.
    Danger,
}

/// The width a button needs for `label` and an optional leading icon.
pub fn button_width(canvas: &mut dyn Canvas, label: &str, icon: Option<IconId>) -> f32 {
    let label_width = if label.is_empty() {
        0.0
    } else {
        canvas.measure(label, BODY_MEDIUM).width
    };
    let icon_width = match (icon, label.is_empty()) {
        (Some(_), true) => 16.0,
        (Some(_), false) => 16.0 + 6.0,
        (None, _) => 0.0,
    };
    (label_width + icon_width + 2.0 * space::M).ceil()
}

/// The persistent state of one text field's editor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditState {
    /// The caret, a byte offset on a character boundary.
    pub cursor: usize,
    /// The other end of the selection; equal to `cursor` when nothing is selected.
    pub anchor: usize,
    /// Horizontal scroll of the text inside the field.
    pub scroll: f32,
    /// A mouse button is held after pressing in the field: moving extends the selection.
    pub dragging: bool,
}

impl EditState {
    /// Puts the caret and anchor inside `text` on character boundaries. A field's state
    /// outlives its text (the same field shows a new folder name, then a rename), so an old
    /// caret can be past the end or in the middle of a character.
    fn fit(&mut self, text: &str) {
        let fit = |at: usize| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        self.cursor = fit(self.cursor);
        self.anchor = fit(self.anchor);
    }
}

/// Text a field can edit: a plain `String`, or a `Zeroizing<String>` for secrets.
pub trait TextBuffer {
    /// The text.
    fn as_str(&self) -> &str;
    /// Replaces `range` with `text`.
    fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str);
    /// Secrets are drawn as dots and never copied out.
    fn is_secret(&self) -> bool {
        false
    }
}

impl TextBuffer for String {
    fn as_str(&self) -> &str {
        self
    }
    fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        String::replace_range(self, range, text);
    }
}

impl TextBuffer for Zeroizing<String> {
    fn as_str(&self) -> &str {
        self
    }
    fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        // Growing a String can move it and leave the old bytes behind: reserve first so
        // the edit happens in place, then wipe nothing because nothing was copied.
        let needed = self.len() - (range.end - range.start) + text.len();
        if needed > self.capacity() {
            let mut grown = Zeroizing::new(String::with_capacity(needed.max(32) * 2));
            grown.push_str(self);
            std::mem::swap(self, &mut grown);
        }
        String::replace_range(self, range, text);
    }
    fn is_secret(&self) -> bool {
        true
    }
}

/// What a text field did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldResponse {
    /// The zone's response.
    pub response: Response,
    /// The text changed.
    pub changed: bool,
    /// Enter was pressed in the field.
    pub submitted: bool,
}

/// Options for [`Ui::text_field`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldStyle<'a> {
    /// Shown while the field is empty.
    pub placeholder: &'a str,
    /// An icon drawn at the field's left edge.
    pub icon: Option<IconId>,
    /// A hint drawn at the right edge while the field is empty (a shortcut, a unit).
    pub hint: &'a str,
    /// Draw the text in the monospace font.
    pub mono: bool,
    /// Show the field in the error color.
    pub error: bool,
}

/// A popup menu open below a widget.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Popup {
    pub(crate) owner: Id,
    anchor: Rect,
    items: Vec<String>,
    selected: Option<usize>,
    width: f32,
    /// The menu was drawn once (it scrolls to the current choice when it first opens).
    shown: bool,
}

impl Ui<'_> {
    /// Whether the menu of the select or menu button `id` is open.
    #[must_use]
    pub fn menu_open(&self, id: Id) -> bool {
        self.state
            .popup
            .as_ref()
            .is_some_and(|popup| popup.owner == id)
    }

    /// A button. `label` may be empty for an icon-only button; `name` is then its accessible
    /// name.
    pub fn button(
        &mut self,
        id: Id,
        rect: Rect,
        label: &str,
        icon: Option<IconId>,
        kind: Kind,
    ) -> Response {
        let name = if label.is_empty() {
            icon.and_then(icons::icon).map_or("", |icon| icon.name)
        } else {
            label
        };
        let response = self.zone(id, rect, Role::Button, name);
        let t = self.theme;
        let hover = self.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let (fill, border, ink) = match kind {
            Kind::Primary => (
                t.accent.mix(t.accent_hover, hover),
                Color::TRANSPARENT,
                t.on_accent,
            ),
            Kind::Secondary => (t.surface.mix(t.hover, hover), t.line_strong, t.text),
            Kind::Ghost => (t.hover.alpha(hover), Color::TRANSPARENT, t.text),
            Kind::Danger => (
                t.error.alpha(0.12 + 0.1 * hover),
                t.error.alpha(0.5),
                t.error,
            ),
        };
        let fill = if response.pressed {
            fill.mix(t.press, 0.4)
        } else {
            fill
        };
        self.gfx.fill_rect(rect, radius::SMALL + 2.0, fill);
        if border.a > 0 {
            self.gfx.stroke_rect(rect, radius::SMALL + 2.0, 1.0, border);
        }
        if response.focus_ring {
            self.focus_ring(rect, radius::SMALL + 2.0);
        }
        let content = button_width(self.gfx, label, icon) - 2.0 * space::M;
        let mut inner = rect.centered(content.min(rect.w - 2.0 * space::S), rect.h);
        if let Some(icon) = icon {
            let slot = inner.take_left(16.0);
            self.gfx.icon(icon, slot.centered(16.0, 16.0), ink);
            inner.take_left(6.0);
        }
        if !label.is_empty() {
            self.strict_text(inner, label, BODY_MEDIUM, ink, Align::Start);
        }
        if response.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        response
    }

    /// A square ghost button holding only an icon, accent-colored when `active`.
    pub fn icon_button(
        &mut self,
        id: Id,
        rect: Rect,
        icon: IconId,
        name: &str,
        active: bool,
    ) -> Response {
        let response = self.zone(id, rect, Role::Button, name);
        let t = self.theme;
        let hover = self.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let fill = if active {
            t.selected
        } else {
            t.hover.alpha(hover)
        };
        self.gfx.fill_rect(rect, radius::SMALL + 2.0, fill);
        if response.focus_ring {
            self.focus_ring(rect, radius::SMALL + 2.0);
        }
        let ink = if active {
            t.accent
        } else {
            t.text2.mix(t.text, hover)
        };
        self.gfx.icon(icon, rect.centered(16.0, 16.0), ink);
        self.tooltip(id, rect, name);
        response
    }

    /// A single-line text field editing `buffer`.
    #[allow(clippy::too_many_lines)] // Input, scrolling and drawing of one field, in order.
    pub fn text_field(
        &mut self,
        id: Id,
        rect: Rect,
        buffer: &mut dyn TextBuffer,
        style: FieldStyle<'_>,
    ) -> FieldResponse {
        let response = self.zone(id, rect, Role::TextInput, style.placeholder);
        let t = self.theme;
        let font = if style.mono {
            Font::mono(text::BODY)
        } else {
            BODY
        };
        let hover = self.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let focus = self.ease(id.with("focus"), if response.focused { 1.0 } else { 0.0 });
        let border = if style.error {
            t.error
        } else {
            t.line_strong.mix(t.text3, hover * 0.6).mix(t.accent, focus)
        };
        if focus > 0.0 {
            self.gfx.fill_rect(
                rect.outset(3.0),
                radius::SMALL + 5.0,
                t.accent.alpha(0.22 * focus),
            );
        }
        self.gfx.fill_rect(rect, radius::SMALL + 2.0, t.field);
        self.gfx.stroke_rect(rect, radius::SMALL + 2.0, 1.0, border);
        if response.hovered {
            self.set_cursor(Cursor::Text);
        }

        let mut inner = rect.inset(space::M - 2.0, 0.0);
        if let Some(icon) = style.icon {
            let slot = inner.take_left(16.0);
            let ink = t.text3.mix(t.text2, focus);
            self.gfx.icon(icon, slot.centered(16.0, 16.0), ink);
            inner.take_left(space::S);
        }

        let mut state = self.state.edit(id);
        state.fit(buffer.as_str());
        let mut changed = false;
        let mut submitted = false;
        if response.focused {
            let mut leftover = Vec::new();
            for typed in self.take_typed() {
                let (key, modifiers) = match typed {
                    Typed::Key(key, modifiers) => (key, modifiers),
                    Typed::Text(text) | Typed::Paste(text) => {
                        // A pasted line break becomes a space: fields are one line.
                        let text: String = text
                            .chars()
                            .map(|c| if c == '\n' { ' ' } else { c })
                            .filter(|c| !c.is_control())
                            .collect();
                        if !text.is_empty() {
                            let (from, to) = state.range();
                            buffer.replace_range(from..to, &text);
                            state.cursor = from + text.len();
                            state.anchor = state.cursor;
                            changed = true;
                        }
                        continue;
                    }
                };
                match edit_key(buffer, &mut state, key, modifiers) {
                    KeyOutcome::Changed => changed = true,
                    KeyOutcome::Moved => {}
                    KeyOutcome::Submit => submitted = true,
                    KeyOutcome::Copy(text) => self.state.clipboard_out = Some(text),
                    KeyOutcome::Ignored => leftover.push((key, modifiers)),
                }
            }
            self.state.return_keys(leftover);
        }
        let content = buffer.as_str();
        state.fit(content);
        let shown: String = if buffer.is_secret() {
            "\u{2022}".repeat(content.chars().count())
        } else {
            content.to_owned()
        };
        // Byte offsets in `content` map to offsets in `shown` by character count.
        let to_shown = |offset: usize| -> usize {
            if buffer.is_secret() {
                content[..offset].chars().count() * '\u{2022}'.len_utf8()
            } else {
                offset
            }
        };
        if (response.pressed || response.clicked)
            && let Some(pointer) = self.pointer()
        {
            let x = pointer.x - inner.x + state.scroll;
            let at = offset_at(self.gfx, &shown, font, x);
            let at = from_shown(content, buffer.is_secret(), at);
            state.cursor = at;
            if !state.dragging {
                state.anchor = at;
            }
            state.dragging = response.pressed;
        } else {
            state.dragging = false;
        }
        if response.double_clicked && !buffer.is_secret() {
            let (start, end) = word_at(content, state.cursor);
            state.anchor = start;
            state.cursor = end;
        }

        let caret_x = self
            .gfx
            .measure(&shown[..to_shown(state.cursor)], font)
            .width;
        if caret_x - state.scroll > inner.w - 2.0 {
            state.scroll = caret_x - inner.w + 2.0;
        }
        if caret_x < state.scroll {
            state.scroll = caret_x;
        }
        let full = self.gfx.measure(&shown, font).width;
        state.scroll = state.scroll.clamp(0.0, (full - inner.w + 2.0).max(0.0));

        self.push_clip(inner);
        let line = self.gfx.measure("Ag", font).height;
        let top = inner.y + (inner.h - line) / 2.0;
        if response.focused && state.cursor != state.anchor {
            let (from, to) = state.range();
            let x0 = self.gfx.measure(&shown[..to_shown(from)], font).width;
            let x1 = self.gfx.measure(&shown[..to_shown(to)], font).width;
            self.gfx.fill_rect(
                Rect::new(inner.x + x0 - state.scroll, top, x1 - x0, line),
                2.0,
                t.accent.alpha(0.35),
            );
        }
        if shown.is_empty() {
            self.gfx
                .text(inner, style.placeholder, font, t.text3, Align::Start);
        } else {
            self.gfx.draw_text(
                Point::new(inner.x - state.scroll, top),
                &shown,
                font,
                t.text,
            );
        }
        if response.focused {
            self.gfx.fill_rect(
                Rect::new(inner.x + caret_x - state.scroll, top + 1.0, 1.5, line - 2.0),
                0.0,
                t.text,
            );
        }
        self.pop_clip();
        let room = inner.w - self.gfx.measure(style.placeholder, font).width;
        let hint = self.gfx.measure(style.hint, SMALL).width + 2.0 * space::S;
        if shown.is_empty() && !style.hint.is_empty() && !response.focused && room > hint + space::L
        {
            let chip = Rect::new(inner.r() - hint, inner.center().y - 10.0, hint, 20.0);
            self.gfx
                .stroke_rect(chip, radius::SMALL, 1.0, t.line_strong);
            self.gfx
                .text(chip, style.hint, SMALL, t.text3, Align::Center);
        }
        self.state.set_edit(id, state);
        if buffer.is_secret() {
            self.set_secret(id);
        } else {
            self.set_value(id, buffer.as_str());
        }
        FieldResponse {
            response,
            changed,
            submitted,
        }
    }

    /// A multi-line text field (scripts): Enter starts a new line, Up and Down move between
    /// lines, Home and End go to the ends of the line, and a paste keeps its line breaks.
    #[allow(clippy::too_many_lines)] // Input, scrolling and drawing of one area, in order.
    pub fn text_area(
        &mut self,
        id: Id,
        rect: Rect,
        buffer: &mut dyn TextBuffer,
        style: FieldStyle<'_>,
    ) -> FieldResponse {
        let response = self.zone(id, rect, Role::TextInput, style.placeholder);
        let t = self.theme;
        let font = if style.mono {
            Font::mono(text::BODY)
        } else {
            BODY
        };
        let focus = self.ease(id.with("focus"), if response.focused { 1.0 } else { 0.0 });
        if focus > 0.0 {
            self.gfx.fill_rect(
                rect.outset(3.0),
                radius::SMALL + 5.0,
                t.accent.alpha(0.22 * focus),
            );
        }
        self.gfx.fill_rect(rect, radius::SMALL + 2.0, t.field);
        let border = t.line_strong.mix(t.accent, focus);
        self.gfx.stroke_rect(rect, radius::SMALL + 2.0, 1.0, border);
        if response.hovered {
            self.set_cursor(Cursor::Text);
        }
        let inner = rect.inset(space::M - 2.0, space::S);
        let line_h = self.gfx.measure("Ag", font).height;
        let mut state = self.state.edit(id);
        state.fit(buffer.as_str());
        let mut changed = false;
        if response.focused {
            let mut leftover = Vec::new();
            for typed in self.take_typed() {
                match typed {
                    Typed::Text(text) | Typed::Paste(text) => {
                        let text: String = text
                            .replace("\r\n", "\n")
                            .chars()
                            .filter(|c| *c == '\n' || !c.is_control())
                            .collect();
                        if !text.is_empty() {
                            let (from, to) = state.range();
                            buffer.replace_range(from..to, &text);
                            state.cursor = from + text.len();
                            state.anchor = state.cursor;
                            changed = true;
                        }
                    }
                    Typed::Key(Key::Enter, Modifiers::NONE) => {
                        let (from, to) = state.range();
                        buffer.replace_range(from..to, "\n");
                        state.cursor = from + 1;
                        state.anchor = state.cursor;
                        changed = true;
                    }
                    Typed::Key(key @ (Key::Up | Key::Down), modifiers) => {
                        let up = key == Key::Up;
                        state.cursor = line_step(buffer.as_str(), state.cursor, up);
                        if !modifiers.shift {
                            state.anchor = state.cursor;
                        }
                    }
                    Typed::Key(key @ (Key::Home | Key::End), modifiers) if !modifiers.ctrl => {
                        let (start, end) = line_bounds(buffer.as_str(), state.cursor);
                        state.cursor = if key == Key::Home { start } else { end };
                        if !modifiers.shift {
                            state.anchor = state.cursor;
                        }
                    }
                    Typed::Key(key, modifiers) => {
                        match edit_key(buffer, &mut state, key, modifiers) {
                            KeyOutcome::Changed => changed = true,
                            KeyOutcome::Copy(text) => self.state.clipboard_out = Some(text),
                            KeyOutcome::Ignored => leftover.push((key, modifiers)),
                            KeyOutcome::Moved | KeyOutcome::Submit => {}
                        }
                    }
                }
            }
            self.state.return_keys(leftover);
        }
        let secret = buffer.is_secret();
        let text = buffer.as_str();
        state.fit(text);
        // The caret's line stays in view.
        let caret_row = text[..state.cursor].matches('\n').count();
        #[allow(clippy::cast_precision_loss)] // Scripts are short.
        let caret_y = caret_row as f32 * line_h;
        if caret_y - state.scroll > inner.h - line_h {
            state.scroll = caret_y - inner.h + line_h;
        }
        if caret_y < state.scroll {
            state.scroll = caret_y;
        }
        #[allow(clippy::cast_precision_loss)] // Scripts are short.
        let content = (text.matches('\n').count() + 1) as f32 * line_h;
        state.scroll -= self.take_wheel(inner);
        state.scroll = state.scroll.clamp(0.0, (content - inner.h).max(0.0));
        if (response.pressed || response.clicked)
            && let Some(pointer) = self.pointer()
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let row = ((pointer.y - inner.y + state.scroll) / line_h).max(0.0) as usize;
            let mut start = 0;
            for (index, line) in text.split('\n').enumerate() {
                if index == row {
                    break;
                }
                start += line.len() + 1;
            }
            let start = start.min(text.len());
            let line_end = text[start..].find('\n').map_or(text.len(), |at| start + at);
            let line = &text[start..line_end];
            let shown = if secret {
                "\u{2022}".repeat(line.chars().count())
            } else {
                line.to_owned()
            };
            let at = start
                + from_shown(
                    line,
                    secret,
                    offset_at(self.gfx, &shown, font, pointer.x - inner.x),
                );
            state.cursor = at;
            if !state.dragging {
                state.anchor = at;
            }
            state.dragging = response.pressed;
        } else {
            state.dragging = false;
        }

        self.push_clip(inner);
        if text.is_empty() {
            self.gfx.draw_text(
                Point::new(inner.x, inner.y),
                style.placeholder,
                font,
                t.text3,
            );
        }
        let (from, to) = state.range();
        let mut start = 0;
        for (row, line) in text.split('\n').enumerate() {
            #[allow(clippy::cast_precision_loss)] // Scripts are short.
            let y = inner.y + row as f32 * line_h - state.scroll;
            let end = start + line.len();
            // A secret is drawn as one dot per character; byte offsets map by count.
            let masked = if secret {
                "\u{2022}".repeat(line.chars().count())
            } else {
                String::new()
            };
            let to_shown = |offset: usize| {
                if secret {
                    line[..offset].chars().count() * '\u{2022}'.len_utf8()
                } else {
                    offset
                }
            };
            let line_shown: &str = if secret { &masked } else { line };
            if y + line_h >= inner.y && y <= inner.b() {
                if response.focused && from < to && from <= end && to >= start {
                    let a = from.max(start) - start;
                    let b = to.min(end) - start;
                    let x0 = self.gfx.measure(&line_shown[..to_shown(a)], font).width;
                    let mut x1 = self.gfx.measure(&line_shown[..to_shown(b)], font).width;
                    if to > end {
                        x1 += 6.0;
                    }
                    self.gfx.fill_rect(
                        Rect::new(inner.x + x0, y, x1 - x0, line_h),
                        2.0,
                        t.accent.alpha(0.35),
                    );
                }
                self.gfx
                    .draw_text(Point::new(inner.x, y), line_shown, font, t.text);
                if response.focused
                    && (start..=end).contains(&state.cursor)
                    && state.cursor >= start
                {
                    let x = self
                        .gfx
                        .measure(&line_shown[..to_shown(state.cursor - start)], font)
                        .width;
                    if row == caret_row {
                        self.gfx.fill_rect(
                            Rect::new(inner.x + x, y + 1.0, 1.5, line_h - 2.0),
                            0.0,
                            t.text,
                        );
                    }
                }
            }
            start = end + 1;
        }
        self.pop_clip();
        self.state.set_edit(id, state);
        if buffer.is_secret() {
            self.set_secret(id);
        } else {
            self.set_value(id, buffer.as_str());
        }
        FieldResponse {
            response,
            changed,
            submitted: false,
        }
    }

    /// A check box with `label` to its right; the whole row toggles it.
    pub fn checkbox(&mut self, id: Id, rect: Rect, value: &mut bool, label: &str) -> Response {
        let response = self.zone(id, rect, Role::CheckBox, label);
        if response.clicked {
            *value = !*value;
        }
        self.set_toggled(id, *value);
        let t = self.theme;
        let hover = self.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        let on = self.ease(id.with("on"), if *value { 1.0 } else { 0.0 });
        let mut row = rect;
        let box_rect = row.take_left(16.0).centered(16.0, 16.0);
        let fill = t.field.mix(t.accent, on);
        self.gfx.fill_rect(box_rect, radius::SMALL, fill);
        let border = t.line_strong.mix(t.text3, hover).mix(t.accent, on);
        self.gfx.stroke_rect(box_rect, radius::SMALL, 1.0, border);
        if on > 0.0 {
            self.gfx.icon(
                icons::CHECK,
                box_rect.inset(1.5, 1.5),
                t.on_accent.alpha(on),
            );
        }
        if response.focus_ring {
            self.focus_ring(box_rect, radius::SMALL);
        }
        row.take_left(space::S + 2.0);
        self.gfx.text(row, label, BODY, t.text, Align::Start);
        if response.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        response
    }

    /// An on/off switch, 34 x 20, centered in `rect`.
    pub fn switch(&mut self, id: Id, rect: Rect, value: &mut bool, name: &str) -> Response {
        let track = rect.centered(34.0, 20.0);
        let response = self.zone(id, track, Role::CheckBox, name);
        if response.clicked {
            *value = !*value;
        }
        self.set_toggled(id, *value);
        let t = self.theme;
        let on = self.ease(id.with("on"), if *value { 1.0 } else { 0.0 });
        self.gfx
            .fill_rect(track, 10.0, t.line_strong.mix(t.accent, on));
        let knob = Point::new(track.x + 10.0 + 14.0 * on, track.center().y);
        self.gfx
            .fill_circle(knob, 7.0, t.on_accent.mix(Color::WHITE, 0.4));
        if response.focus_ring {
            self.focus_ring(track, 10.0);
        }
        response
    }

    /// A drop-down: the current choice in a field-like box; clicking opens the list.
    /// Returns the index chosen this frame.
    pub fn select(
        &mut self,
        id: Id,
        rect: Rect,
        items: &[String],
        selected: Option<usize>,
        placeholder: &str,
    ) -> Option<usize> {
        let current = selected
            .and_then(|index| items.get(index))
            .map_or(placeholder, String::as_str);
        let response = self.zone(id, rect, Role::ComboBox, current);
        self.set_value(id, current);
        let t = self.theme;
        let open = self.state.popup.as_ref().is_some_and(|p| p.owner == id);
        let hover = self.ease(id.with("hover"), if response.hovered { 1.0 } else { 0.0 });
        self.gfx
            .fill_rect(rect, radius::SMALL + 2.0, t.field.mix(t.hover, hover * 0.5));
        let border = if open || response.focused {
            t.accent
        } else {
            t.line_strong.mix(t.text3, hover * 0.6)
        };
        self.gfx.stroke_rect(rect, radius::SMALL + 2.0, 1.0, border);
        if response.focus_ring {
            self.focus_ring(rect, radius::SMALL + 2.0);
        }
        let mut inner = rect.inset(space::M - 2.0, 0.0);
        let chevron = inner.take_right(16.0);
        self.gfx
            .icon(icons::CHEVRON_DOWN, chevron.centered(14.0, 14.0), t.text2);
        let ink = if selected.is_some() { t.text } else { t.text3 };
        self.gfx.text(inner, current, BODY, ink, Align::Start);
        if response.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        self.popup_toggle(id, &response, rect, items, selected, rect.w)
    }

    /// A button that opens a menu of `items` below it. Returns the index chosen this frame.
    pub fn menu_button(
        &mut self,
        id: Id,
        rect: Rect,
        label: &str,
        icon: Option<IconId>,
        kind: Kind,
        items: &[String],
    ) -> Option<usize> {
        let response = self.button(id, rect, label, icon, kind);
        let width = items
            .iter()
            .map(|item| self.gfx.measure(item, BODY).width + 2.0 * space::L)
            .fold(rect.w.max(180.0), f32::max);
        self.popup_toggle(id, &response, rect, items, None, width)
    }

    fn popup_toggle(
        &mut self,
        id: Id,
        response: &Response,
        anchor: Rect,
        items: &[String],
        selected: Option<usize>,
        width: f32,
    ) -> Option<usize> {
        let chosen =
            (0..items.len()).find(|index| self.state.was_clicked(id.with(("item", *index))));
        if chosen.is_some() {
            self.state.popup = None;
            return chosen;
        }
        if response.clicked {
            let open = self.state.popup.as_ref().is_some_and(|p| p.owner == id);
            self.state.popup = (!open).then(|| Popup {
                owner: id,
                anchor,
                items: items.to_vec(),
                selected,
                width,
                shown: false,
            });
        }
        None
    }

    /// Draws what floats above the frame: the open menu. Call after everything else.
    pub fn overlays(&mut self) {
        let Some(popup) = self.state.popup.clone() else {
            self.draw_tooltip();
            return;
        };
        if self.key(Key::Escape, Modifiers::NONE) {
            self.state.popup = None;
            return;
        }
        let t = self.theme;
        // Clicking anywhere outside the menu closes it.
        let backdrop = self.zone(
            popup.owner.with("backdrop"),
            self.viewport,
            Role::Backdrop,
            "",
        );
        if backdrop.clicked || backdrop.secondary_clicked {
            self.state.popup = None;
            return;
        }
        #[allow(clippy::cast_precision_loss)] // Menus hold dozens of items at most.
        let content = popup.items.len() as f32 * MENU_ITEM + 8.0;
        // Below the widget when it fits, else above; when neither side holds it all, the side
        // with more room, and the menu scrolls.
        let below = popup.anchor.b() + 4.0;
        let room_below = self.viewport.b() - 8.0 - below;
        let room_above = popup.anchor.y - 4.0 - (self.viewport.y + 8.0);
        let (y, height) = if content <= room_below {
            (below, content)
        } else if content <= room_above {
            (popup.anchor.y - 4.0 - content, content)
        } else if room_below >= room_above {
            (below, room_below.max(MENU_ITEM + 8.0))
        } else {
            (popup.anchor.y - 4.0 - room_above, room_above)
        };
        let x = popup
            .anchor
            .x
            .min(self.viewport.r() - popup.width - 8.0)
            .max(8.0);
        let menu = Rect::new(x, y, popup.width, height);
        self.gfx.shadow(menu, radius::MEDIUM, 16.0, 6.0, t.shadow);
        self.gfx.fill_rect(menu, radius::MEDIUM, t.raised);
        self.gfx.stroke_rect(menu, radius::MEDIUM, 1.0, t.line);
        let list = menu.inset(4.0, 4.0);
        let scroll_id = popup.owner.with("menu.scroll");
        let offset = if content > height {
            // In view: the item the keyboard is on, or the current choice as the menu opens.
            let focused = (0..popup.items.len())
                .find(|index| self.state.focus() == Some(popup.owner.with(("item", *index))));
            let keep = focused.or(popup.selected.filter(|_| !popup.shown));
            if let Some(index) = keep {
                #[allow(clippy::cast_precision_loss)] // As above.
                let top = index as f32 * MENU_ITEM;
                let now = self.scroll_offset(scroll_id);
                self.set_scroll_offset(scroll_id, now.clamp(top + MENU_ITEM - list.h, top));
            }
            self.scroll_view(scroll_id, list, content - 8.0)
        } else {
            0.0
        };
        if let Some(open) = self.state.popup.as_mut() {
            open.shown = true;
        }
        self.push_clip(list);
        for (index, item) in popup.items.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let row = Rect::new(
                list.x,
                list.y - offset + index as f32 * MENU_ITEM,
                list.w,
                MENU_ITEM,
            );
            if row.b() < list.y || row.y > list.b() {
                continue;
            }
            let id = popup.owner.with(("item", index));
            let response = self.zone(id, row, Role::MenuItem, item);
            if response.hovered || response.focus_ring {
                self.gfx.fill_rect(row, radius::SMALL, t.hover);
            }
            let mut inner = row.inset(space::S + 2.0, 0.0);
            let check = inner.take_right(16.0);
            if popup.selected == Some(index) {
                self.gfx
                    .icon(icons::CHECK, check.centered(14.0, 14.0), t.accent);
            }
            self.gfx.text(inner, item, BODY, t.text, Align::Start);
            if response.hovered {
                self.set_cursor(Cursor::Pointer);
            }
        }
        self.pop_clip();
    }

    /// The tooltip asked for this frame: under its control, or above it at the window's
    /// bottom, inside the window.
    fn draw_tooltip(&mut self) {
        let Some((text, anchor)) = self.state.tooltip.take() else {
            return;
        };
        let t = self.theme;
        let size = self.gfx.measure(&text, SMALL);
        let (width, height) = (size.width + 2.0 * space::S, size.height + 8.0);
        let below = anchor.b() + 6.0;
        let top = if below + height > self.viewport.b() - 4.0 {
            anchor.y - 6.0 - height
        } else {
            below
        };
        let left = (anchor.center().x - width / 2.0)
            .min(self.viewport.r() - width - 4.0)
            .max(self.viewport.x + 4.0);
        let tip = Rect::new(left, top, width, height);
        self.gfx.shadow(tip, radius::SMALL, 10.0, 3.0, t.shadow);
        self.gfx.fill_rect(tip, radius::SMALL, t.raised);
        self.gfx.stroke_rect(tip, radius::SMALL, 1.0, t.line_strong);
        self.gfx.text(tip, &text, SMALL, t.text, Align::Center);
    }

    /// A small uppercase label above a group of rows.
    pub fn section_label(&mut self, rect: Rect, label: &str) {
        let t = self.theme;
        self.gfx.text(
            rect,
            &label.to_uppercase(),
            LABEL,
            t.text3.mix(t.text2, 0.4),
            Align::Start,
        );
    }

    /// A rounded tag chip; returns its width.
    pub fn chip(&mut self, at: Point, label: &str, color: Color) -> f32 {
        let width = self.gfx.measure(label, SMALL).width + 2.0 * space::S;
        let rect = Rect::new(at.x, at.y, width, 20.0);
        self.gfx.fill_rect(rect, 10.0, color.alpha(0.14));
        self.gfx.text(
            rect,
            label,
            SMALL,
            color.mix(self.theme.text, 0.35),
            Align::Center,
        );
        width
    }

    /// A vertical scroll view: handles the wheel, clips to `rect` and draws a thin scroll bar
    /// while the content is taller than the view. Returns the content offset to draw at.
    pub fn scroll_view(&mut self, id: Id, rect: Rect, content_height: f32) -> f32 {
        let max = (content_height - rect.h).max(0.0);
        let mut offset = self.scroll_offset(id) - self.take_wheel(rect);
        offset = offset.clamp(0.0, max);
        self.set_scroll_offset(id, offset);
        if max > 0.0 {
            let t = self.theme;
            let track = Rect::new(rect.r() - 6.0, rect.y + 2.0, 4.0, rect.h - 4.0);
            let thumb_h = (rect.h / content_height * track.h).max(24.0);
            let thumb_y = track.y + (track.h - thumb_h) * (offset / max);
            self.gfx.fill_rect(
                Rect::new(track.x, thumb_y, track.w, thumb_h),
                2.0,
                t.text2.alpha(0.4),
            );
        }
        offset
    }
}

/// What a key did inside a text field.
enum KeyOutcome {
    Changed,
    Moved,
    Submit,
    Copy(String),
    Ignored,
}

impl EditState {
    /// The selection as an ordered byte range.
    #[must_use]
    pub fn range(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }
}

fn edit_key(
    buffer: &mut dyn TextBuffer,
    state: &mut EditState,
    key: Key,
    modifiers: Modifiers,
) -> KeyOutcome {
    let text = buffer.as_str();
    let word = modifiers.ctrl;
    let extend = modifiers.shift;
    let move_to = |state: &mut EditState, to: usize| {
        state.cursor = to;
        if !extend {
            state.anchor = to;
        }
        KeyOutcome::Moved
    };
    match key {
        Key::Left => {
            let (from, to) = state.range();
            if from != to && !extend {
                return move_to(state, from);
            }
            let to = if word {
                word_left(text, state.cursor)
            } else {
                prev_char(text, state.cursor)
            };
            move_to(state, to)
        }
        Key::Right => {
            let (from, to) = state.range();
            if from != to && !extend {
                return move_to(state, to);
            }
            let to = if word {
                word_right(text, state.cursor)
            } else {
                next_char(text, state.cursor)
            };
            move_to(state, to)
        }
        Key::Home => move_to(state, 0),
        Key::End => move_to(state, text.len()),
        Key::Backspace | Key::Delete => {
            let (mut from, mut to) = state.range();
            if from == to {
                if key == Key::Backspace {
                    from = if word {
                        word_left(text, from)
                    } else {
                        prev_char(text, from)
                    };
                } else {
                    to = if word {
                        word_right(text, to)
                    } else {
                        next_char(text, to)
                    };
                }
            }
            if from == to {
                return KeyOutcome::Moved;
            }
            buffer.replace_range(from..to, "");
            state.cursor = from;
            state.anchor = from;
            KeyOutcome::Changed
        }
        Key::Char('a') if modifiers == Modifiers::CTRL => {
            state.anchor = 0;
            state.cursor = text.len();
            KeyOutcome::Moved
        }
        Key::Char(c @ ('c' | 'x')) if modifiers == Modifiers::CTRL => {
            let (from, to) = state.range();
            if buffer.is_secret() || from == to {
                return KeyOutcome::Moved;
            }
            let copied = text[from..to].to_owned();
            if c == 'x' {
                buffer.replace_range(from..to, "");
                state.cursor = from;
                state.anchor = from;
            }
            KeyOutcome::Copy(copied)
        }
        Key::Enter if modifiers == Modifiers::NONE => KeyOutcome::Submit,
        _ => KeyOutcome::Ignored,
    }
}

/// The byte range of the line holding `at` (without its line break).
fn line_bounds(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    (start, end)
}

/// The offset one line up or down from `at`, keeping the column (in characters) where the
/// line is long enough; the ends of the text on the first and last line.
fn line_step(text: &str, at: usize, up: bool) -> usize {
    let (start, end) = line_bounds(text, at);
    let column = text[start..at].chars().count();
    let (line_start, line_end) = if up {
        if start == 0 {
            return 0;
        }
        line_bounds(text, start - 1)
    } else {
        if end == text.len() {
            return text.len();
        }
        line_bounds(text, end + 1)
    };
    text[line_start..line_end]
        .char_indices()
        .nth(column)
        .map_or(line_end, |(offset, _)| line_start + offset)
}

fn prev_char(text: &str, at: usize) -> usize {
    text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map_or(text.len(), |c| at + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_left(text: &str, at: usize) -> usize {
    let mut chars = text[..at].char_indices().rev().peekable();
    while chars.next_if(|(_, c)| !is_word(*c)).is_some() {}
    let mut start = chars.peek().map_or(0, |(i, _)| *i);
    while let Some((i, _)) = chars.next_if(|(_, c)| is_word(*c)) {
        start = i;
    }
    if text[..at].chars().all(|c| !is_word(c)) {
        0
    } else {
        start
    }
}

fn word_right(text: &str, at: usize) -> usize {
    let mut end = at;
    let mut chars = text[at..].chars().peekable();
    while let Some(c) = chars.next_if(|c| !is_word(*c)) {
        end += c.len_utf8();
    }
    while let Some(c) = chars.next_if(|c| is_word(*c)) {
        end += c.len_utf8();
    }
    end
}

fn word_at(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = at
        + text[at..]
            .chars()
            .take_while(|c| is_word(*c))
            .map(char::len_utf8)
            .sum::<usize>();
    (start, end)
}

/// The byte offset in `shown` nearest to `x` DIPs from its start.
fn offset_at(canvas: &mut dyn Canvas, shown: &str, font: Font, x: f32) -> usize {
    let mut best = (0, f32::MAX);
    for (offset, _) in shown
        .char_indices()
        .chain(std::iter::once((shown.len(), ' ')))
    {
        let distance = (canvas.measure(&shown[..offset], font).width - x).abs();
        if distance < best.1 {
            best = (offset, distance);
        }
    }
    best.0
}

/// Maps an offset in the shown text back to the real text (secrets show one dot per
/// character).
fn from_shown(content: &str, secret: bool, offset: usize) -> usize {
    if !secret {
        return offset;
    }
    let chars = offset / '\u{2022}'.len_utf8();
    content
        .char_indices()
        .nth(chars)
        .map_or(content.len(), |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::Recorder;
    use crate::kit::{Event, Theme, UiState, Zone};
    use annulus_config::tokens::Appearance;

    fn frame(state: &mut UiState, events: &[Event], draw: impl FnOnce(&mut Ui<'_>)) {
        let mut canvas = Recorder::default();
        state.begin_frame(events, 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            state,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        draw(&mut ui);
        ui.overlays();
        state.end_frame();
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key(key, modifiers)
    }

    const FIELD: Rect = Rect {
        x: 10.0,
        y: 10.0,
        w: 200.0,
        h: 32.0,
    };

    fn type_into(buffer: &mut dyn TextBuffer, events: &[Event]) -> FieldResponse {
        let mut state = UiState::default();
        let id = Id::new("field");
        frame(&mut state, &[], |ui| {
            ui.text_field(id, FIELD, buffer, FieldStyle::default());
        });
        state.set_focus(Some(id), true);
        let mut out = FieldResponse::default();
        frame(&mut state, events, |ui| {
            out = ui.text_field(id, FIELD, buffer, FieldStyle::default());
        });
        out
    }

    #[test]
    fn typing_inserts_at_the_caret_and_editing_keys_work() {
        let mut text = String::from("hello world");
        let out = type_into(
            &mut text,
            &[
                key(Key::End, Modifiers::NONE),
                key(Key::Backspace, Modifiers::CTRL),
                Event::Text("there".into()),
                key(Key::Home, Modifiers::NONE),
                key(Key::Delete, Modifiers::NONE),
                Event::Text("H".into()),
            ],
        );
        assert_eq!(text, "Hello there");
        assert!(out.changed && !out.submitted);
    }

    #[test]
    fn selection_is_replaced_and_enter_submits() {
        let mut text = String::from("root@db-1");
        let out = type_into(
            &mut text,
            &[
                key(Key::End, Modifiers::NONE),
                key(
                    Key::Left,
                    Modifiers {
                        ctrl: true,
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
                Event::Text("web".into()),
                key(Key::Enter, Modifiers::NONE),
            ],
        );
        // Ctrl+Shift+Left selects the word "1"; typing replaces it.
        assert_eq!(text, "root@db-web");
        assert!(out.submitted);
    }

    #[test]
    fn select_all_and_copy_hand_text_out_but_never_from_a_secret() {
        let mut state = UiState::default();
        let id = Id::new("field");
        let mut plain = String::from("visible");
        frame(&mut state, &[], |ui| {
            ui.text_field(id, FIELD, &mut plain, FieldStyle::default());
        });
        state.set_focus(Some(id), true);
        let copy = [
            key(Key::Char('a'), Modifiers::CTRL),
            key(Key::Char('c'), Modifiers::CTRL),
        ];
        frame(&mut state, &copy, |ui| {
            ui.text_field(id, FIELD, &mut plain, FieldStyle::default());
        });
        assert_eq!(state.take_clipboard().as_deref(), Some("visible"));
        let mut secret = Zeroizing::new(String::from("hunter2"));
        frame(&mut state, &copy, |ui| {
            ui.text_field(id, FIELD, &mut secret, FieldStyle::default());
        });
        assert_eq!(state.take_clipboard(), None);
    }

    #[test]
    fn a_secret_field_draws_dots_never_its_text() {
        let mut state = UiState::default();
        let mut canvas = Recorder::default();
        let mut secret = Zeroizing::new(String::from("hunter2"));
        state.begin_frame(&[], 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        ui.text_field(Id::new("pw"), FIELD, &mut secret, FieldStyle::default());
        assert!(
            canvas
                .texts
                .iter()
                .all(|(_, text)| !text.contains("hunter"))
        );
        assert!(
            canvas
                .texts
                .iter()
                .any(|(_, text)| text == &"\u{2022}".repeat(7))
        );
    }

    #[test]
    fn up_and_down_keep_the_column_and_stop_at_the_ends() {
        let text = "cd /srv\nls\nsudo systemctl restart app";
        // From "ls" column 1 up to "cd /srv" column 1.
        assert_eq!(line_step(text, 9, true), 1);
        // From "cd /srv" column 5 down to "ls", which is shorter: its end.
        assert_eq!(line_step(text, 5, false), 10);
        assert_eq!(line_step(text, 3, true), 0);
        assert_eq!(line_step(text, text.len() - 2, false), text.len());
        assert_eq!(line_bounds(text, 9), (8, 10));
    }

    #[test]
    fn a_text_area_keeps_line_breaks_from_enter_and_paste() {
        let mut state = UiState::default();
        let id = Id::new("script");
        let mut script = String::from("echo a");
        frame(&mut state, &[], |ui| {
            ui.text_area(id, FIELD, &mut script, FieldStyle::default());
        });
        state.set_focus(Some(id), true);
        frame(
            &mut state,
            &[
                key(Key::End, Modifiers::NONE),
                key(Key::Enter, Modifiers::NONE),
                Event::Paste("echo b\r\necho c".into()),
                key(Key::Up, Modifiers::NONE),
                key(Key::Home, Modifiers::NONE),
                Event::Text("# ".into()),
            ],
            |ui| {
                ui.text_area(id, FIELD, &mut script, FieldStyle::default());
            },
        );
        assert_eq!(script, "echo a\n# echo b\necho c");
    }

    #[test]
    fn a_secret_text_area_takes_a_pasted_key_and_draws_only_dots() {
        let mut state = UiState::default();
        let id = Id::new("key");
        let mut key_text = Zeroizing::new(String::new());
        frame(&mut state, &[], |ui| {
            ui.text_area(id, FIELD, &mut key_text, FieldStyle::default());
        });
        state.set_focus(Some(id), true);
        let paste = [Event::Paste("BEGIN KEY\r\nb3BlbnNzaA\r\nEND KEY".into())];
        let mut canvas = Recorder::default();
        state.begin_frame(&paste, 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        ui.text_area(id, FIELD, &mut key_text, FieldStyle::default());
        assert_eq!(key_text.as_str(), "BEGIN KEY\nb3BlbnNzaA\nEND KEY");
        assert!(
            canvas
                .texts
                .iter()
                .all(|(_, text)| !text.contains("KEY") && !text.contains("b3Bl"))
        );
        assert!(
            canvas
                .texts
                .iter()
                .any(|(_, text)| text == &"\u{2022}".repeat(10))
        );
    }

    #[test]
    fn a_caret_left_from_other_text_lands_on_a_character_boundary() {
        let mut state = UiState::default();
        let id = Id::new("field");
        let mut short = String::from("ab");
        frame(&mut state, &[], |ui| {
            ui.text_field(id, FIELD, &mut short, FieldStyle::default());
        });
        state.set_focus(Some(id), true);
        // The caret after "a", byte 1.
        frame(
            &mut state,
            &[
                key(Key::Home, Modifiers::NONE),
                key(Key::Right, Modifiers::NONE),
            ],
            |ui| {
                ui.text_field(id, FIELD, &mut short, FieldStyle::default());
            },
        );
        // The same field now shows text whose byte 1 is inside "\u{c4}"; typing must not panic.
        let mut name = String::from("\u{c4}rger.txt");
        frame(&mut state, &[Event::Text("x".into())], |ui| {
            ui.text_field(id, FIELD, &mut name, FieldStyle::default());
        });
        assert_eq!(name, "x\u{c4}rger.txt");
        let mut area = String::from("\u{c4}\n\u{c4}");
        frame(&mut state, &[Event::Text("y".into())], |ui| {
            ui.text_area(id, FIELD, &mut area, FieldStyle::default());
        });
        assert!(area.contains('y'));
    }

    #[test]
    fn a_menu_too_long_for_the_window_scrolls_inside_it_from_the_current_choice() {
        let mut state = UiState::default();
        let id = Id::new("host");
        let items: Vec<String> = (1..=30).map(|n| format!("host-{n}")).collect();
        let field = Rect::new(10.0, 120.0, 200.0, 32.0);
        let open = [
            Event::PointerMoved(field.center()),
            Event::PointerDown(crate::kit::Button::Primary),
            Event::PointerUp(crate::kit::Button::Primary),
        ];
        frame(&mut state, &[], |ui| {
            ui.select(id, field, &items, Some(24), "Host");
        });
        frame(&mut state, &open, |ui| {
            ui.select(id, field, &items, Some(24), "Host");
        });
        frame(&mut state, &[], |ui| {
            ui.select(id, field, &items, Some(24), "Host");
        });
        let zones = state.zones();
        let window = Rect::new(0.0, 0.0, 400.0, 300.0);
        let menu: Vec<&Zone> = zones
            .iter()
            .filter(|zone| zone.role == Role::MenuItem)
            .collect();
        assert!(
            !menu.is_empty() && menu.len() < items.len(),
            "only what fits is drawn"
        );
        assert!(
            menu.iter()
                .all(|zone| zone.rect.b() <= window.b() && zone.rect.y >= window.y),
            "every item inside the window"
        );
        assert!(
            menu.iter().any(|zone| zone.label == "host-25"),
            "opened at the choice"
        );
    }

    #[test]
    fn an_icon_button_says_what_it_does_after_a_rest_or_on_keyboard_focus() {
        let mut state = UiState::default();
        let id = Id::new("edit");
        let rect = Rect::new(20.0, 20.0, 30.0, 30.0);
        let drawn = |state: &mut UiState, events: &[Event], time: f64| {
            let mut canvas = Recorder::default();
            state.begin_frame(events, time);
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                state,
                Rect::new(0.0, 0.0, 400.0, 300.0),
            );
            ui.icon_button(id, rect, crate::icons::EDIT, "Edit host", false);
            ui.overlays();
            state.end_frame();
            canvas.texts.iter().any(|(_, text)| text == "Edit host")
        };
        drawn(&mut state, &[], 0.0);
        assert!(
            !drawn(&mut state, &[Event::PointerMoved(rect.center())], 1.0),
            "not at once"
        );
        assert!(drawn(&mut state, &[], 1.7), "after a rest");
        assert!(
            !drawn(&mut state, &[Event::PointerLeft], 2.0),
            "gone when the pointer goes"
        );
        state.set_focus(Some(id), true);
        assert!(drawn(&mut state, &[], 2.1), "on keyboard focus at once");
    }

    #[test]
    fn words_are_found_for_ctrl_arrows() {
        let text = "ssh  root@db-1";
        assert_eq!(word_left(text, text.len()), 13);
        assert_eq!(word_left(text, 9), 5);
        assert_eq!(word_left(text, 5), 0);
        assert_eq!(word_right(text, 0), 3);
        assert_eq!(word_right(text, 3), 9);
        assert_eq!(word_at(text, 7), (5, 9));
    }

    #[test]
    fn escape_closes_an_open_menu_before_anything_else_sees_it() {
        let mut state = UiState::default();
        let id = Id::new("group");
        let items = vec!["Top level".to_owned(), "Production".to_owned()];
        let rect = Rect::new(10.0, 10.0, 200.0, 32.0);
        let click = [
            Event::PointerMoved(Point::new(20.0, 20.0)),
            Event::PointerDown(crate::kit::Button::Primary),
            Event::PointerUp(crate::kit::Button::Primary),
        ];
        frame(&mut state, &[], |ui| {
            ui.select(id, rect, &items, Some(0), "");
        });
        frame(&mut state, &click, |ui| {
            ui.select(id, rect, &items, Some(0), "");
        });
        assert!(state.popup.is_some());
        let mut escape_seen = true;
        frame(&mut state, &[key(Key::Escape, Modifiers::NONE)], |ui| {
            ui.select(id, rect, &items, Some(0), "");
            escape_seen = ui.key(Key::Escape, Modifiers::NONE);
        });
        assert!(state.popup.is_none());
        assert!(
            !escape_seen,
            "the panel under the menu must not see the Escape"
        );
        // A menu whose owner stops drawing closes too.
        frame(&mut state, &click, |ui| {
            ui.select(id, rect, &items, Some(0), "");
        });
        assert!(state.popup.is_some());
        frame(&mut state, &[], |_| {});
        assert!(state.popup.is_none());
    }

    #[test]
    fn a_select_opens_its_menu_and_reports_the_choice() {
        let mut state = UiState::default();
        let id = Id::new("group");
        let items = vec!["Top level".to_owned(), "Production".to_owned()];
        let rect = Rect::new(10.0, 10.0, 200.0, 32.0);
        let mut chosen = None;
        let click_at = |x: f32, y: f32| {
            vec![
                Event::PointerMoved(Point::new(x, y)),
                Event::PointerDown(crate::kit::Button::Primary),
                Event::PointerUp(crate::kit::Button::Primary),
            ]
        };
        frame(&mut state, &[], |ui| {
            chosen = ui.select(id, rect, &items, Some(0), "");
        });
        frame(&mut state, &click_at(20.0, 20.0), |ui| {
            chosen = ui.select(id, rect, &items, Some(0), "");
        });
        assert!(chosen.is_none());
        // Draw the open menu once so its rows are on screen, then click the second row:
        // the menu starts 4 below the field, rows are 30 tall after a 4 inset.
        frame(&mut state, &[], |ui| {
            chosen = ui.select(id, rect, &items, Some(0), "");
        });
        frame(&mut state, &click_at(30.0, 42.0 + 4.0 + 4.0 + 45.0), |ui| {
            chosen = ui.select(id, rect, &items, Some(0), "");
        });
        assert_eq!(chosen, Some(1));
    }
}
