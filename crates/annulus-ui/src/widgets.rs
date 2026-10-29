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
    owner: Id,
    anchor: Rect,
    items: Vec<String>,
    selected: Option<usize>,
    width: f32,
}

impl Ui<'_> {
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
        response
    }

    /// A single-line text field editing `buffer`.
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
        let mut changed = false;
        let mut submitted = false;
        if response.focused {
            let mut leftover = Vec::new();
            for typed in self.take_typed() {
                let (key, modifiers) = match typed {
                    Typed::Key(key, modifiers) => (key, modifiers),
                    Typed::Text(text) => {
                        let text: String = text.chars().filter(|c| !c.is_control()).collect();
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
        state.cursor = state.cursor.min(content.len());
        state.anchor = state.anchor.min(content.len());
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
        FieldResponse {
            response,
            changed,
            submitted,
        }
    }

    /// A check box with `label` to its right; the whole row toggles it.
    pub fn checkbox(&mut self, id: Id, rect: Rect, value: &mut bool, label: &str) -> Response {
        let response = self.zone(id, rect, Role::CheckBox, label);
        if response.clicked {
            *value = !*value;
        }
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
            });
        }
        None
    }

    /// Draws what floats above the frame: the open menu. Call after everything else.
    pub fn overlays(&mut self) {
        let Some(popup) = self.state.popup.clone() else {
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
        const ITEM: f32 = 30.0;
        #[allow(clippy::cast_precision_loss)] // Menus are short.
        let height = popup.items.len() as f32 * ITEM + 8.0;
        let below = popup.anchor.b() + 4.0;
        let y = if below + height > self.viewport.b() - 8.0 {
            (popup.anchor.y - 4.0 - height).max(8.0)
        } else {
            below
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
        let mut rows = menu.inset(4.0, 4.0);
        for (index, item) in popup.items.iter().enumerate() {
            let row = rows.take_top(ITEM);
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
    use crate::kit::{Event, Theme, UiState};
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
