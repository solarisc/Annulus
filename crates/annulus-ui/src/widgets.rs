//! Our widgets, drawn from the theme (UI-SPEC §8): buttons, text fields with our own editor,
//! check boxes, switches, drop-down menus, scroll views, chips and section labels. Each takes
//! the rectangle it fills; views lay rectangles out with [`Rect`]'s cutting helpers and the
//! `*_width` measurements here.

use std::sync::Arc;

use zeroize::Zeroizing;

use crate::gfx::{Align, Canvas, Color, Font, Point, Rect, Weight};
use crate::icons::{self, IconId};
use crate::kit::{Cursor, Id, Key, Modifiers, Response, Role, Typed, Ui};
use crate::motion::{Layer, Motion};
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

/// Text on keycaps.
const KEYCAP: Font = Font::ui(11.0).weight(Weight::Medium);
/// Height of a keycap.
pub const KEYCAP_H: f32 = 20.0;
/// Space between the keycaps of one chord.
const KEYCAP_GAP: f32 = 4.0;

/// The keys of `chord`, split at its `+` signs ("Ctrl+=" is Ctrl and =; "Ctrl++" is Ctrl and
/// +).
#[must_use]
pub fn chord_keys(chord: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut rest = chord;
    while !rest.is_empty() {
        // A key is at least one character, so a "+" right at the start is the key itself.
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let end = rest[first..].find('+').map_or(rest.len(), |at| at + first);
        keys.push(&rest[..end]);
        rest = rest[end..].strip_prefix('+').unwrap_or("");
    }
    keys
}

fn keycap_width(canvas: &mut dyn Canvas, key: &str) -> f32 {
    (canvas.measure(key, KEYCAP).width + 12.0)
        .max(KEYCAP_H)
        .ceil()
}

/// The width [`Ui::keys`] takes for `chord`.
pub fn keys_width(canvas: &mut dyn Canvas, chord: &str) -> f32 {
    let keys = chord_keys(chord);
    #[allow(clippy::cast_precision_loss)] // A chord has a handful of keys.
    let gaps = keys.len().saturating_sub(1) as f32 * KEYCAP_GAP;
    keys.iter()
        .map(|key| keycap_width(canvas, key))
        .sum::<f32>()
        + gaps
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

/// Width of the menu half of a [`Ui::split_button`].
pub const SPLIT_MENU_W: f32 = 30.0;

/// The width a [`Ui::split_button`] needs to show `label` in full.
pub fn split_button_width(canvas: &mut dyn Canvas, label: &str, icon: IconId) -> f32 {
    button_width(canvas, label, Some(icon)) + SPLIT_MENU_W
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

/// The rows a wrapping [`Ui::text_area`] broke its text into, kept while they still fit:
/// `fit` is the hash of that text and the width's bits.
#[derive(Debug)]
pub(crate) struct Wrapped {
    fit: (u64, u32),
    rows: Vec<(usize, usize)>,
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

/// What a split button did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SplitResponse {
    /// The main action's half.
    pub main: Response,
    /// The menu item chosen this frame.
    pub chosen: Option<usize>,
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
    /// A [`Ui::text_area`] breaks long lines at its width instead of running past it (prose,
    /// such as notes; scripts keep their lines whole).
    pub wrap: bool,
}

/// What a toast is about, for its color and mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something happened.
    Info,
    /// Something finished well.
    Done,
    /// Something went wrong.
    Problem,
}

/// How long a toast stays, in seconds, coming and going included.
pub const TOAST_LIFE: f64 = 4.0;

/// One toast on screen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Toast {
    text: String,
    tone: Tone,
    /// When it was shown, on the window's clock.
    at: f64,
}

/// A popup menu open below a widget.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Popup {
    pub(crate) owner: Id,
    anchor: Rect,
    items: Arc<[String]>,
    /// A few colors per item, drawn as dots beside its name (a color scheme's); may be empty.
    swatches: Arc<[Vec<Color>]>,
    selected: Option<usize>,
    width: f32,
    /// The menu was drawn once (it scrolls to the current choice when it first opens).
    shown: bool,
    /// What the search at the top of a long menu holds.
    query: String,
    /// The item under the pointer, else the one the keyboard is on, as last drawn.
    hot: Option<usize>,
    /// The item Enter in the search picked, handed to the owner next frame.
    picked: Option<usize>,
}

impl Popup {
    fn new(owner: Id, anchor: Rect, items: &[String], selected: Option<usize>, width: f32) -> Self {
        Popup {
            owner,
            anchor,
            items: items.into(),
            swatches: Arc::new([]),
            selected,
            width,
            shown: false,
            query: String::new(),
            hot: None,
            picked: None,
        }
    }

    /// Whether the menu has a search: it is too long to read through.
    fn searchable(&self) -> bool {
        self.items.len() > SEARCH_FROM
    }

    /// The items the search keeps, in order: those whose name holds every word typed, in any
    /// case.
    fn matches(&self) -> Vec<usize> {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        (0..self.items.len())
            .filter(|index| {
                let name = self.items[*index].to_lowercase();
                words.iter().all(|word| name.contains(word.as_str()))
            })
            .collect()
    }
}

/// The height of a menu's search row.
const SEARCH_H: f32 = CONTROL + 4.0;

/// Menus longer than this get a search at the top.
const SEARCH_FROM: usize = 12;

/// The width of the dots [`swatch`] draws for `colors`.
fn swatch_width(colors: &[Color]) -> f32 {
    #[allow(clippy::cast_precision_loss)] // A handful of dots.
    let dots = colors.len().saturating_sub(1) as f32;
    dots * 11.0 + 7.0
}

/// A color scheme in small: a pill in its first color holding a dot of each of the others.
fn swatch(canvas: &mut dyn Canvas, rect: Rect, colors: &[Color], line: Color) {
    let Some((&back, dots)) = colors.split_first() else {
        return;
    };
    let pill = rect.centered(swatch_width(colors), 16.0);
    canvas.fill_rect(pill, 8.0, back);
    canvas.stroke_rect(pill, 8.0, 1.0, line);
    for (index, dot) in dots.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // As above.
        let x = pill.x + 8.0 + index as f32 * 11.0;
        canvas.fill_circle(Point::new(x, pill.center().y), 3.5, *dot);
    }
}

impl Ui<'_> {
    /// The item of the open menu of `id` under the pointer, else the one the keyboard is on:
    /// what a choice would look like can be shown before it is made.
    #[must_use]
    pub fn menu_hot(&self, id: Id) -> Option<usize> {
        self.state
            .popup
            .as_ref()
            .filter(|popup| popup.owner == id)
            .and_then(|popup| popup.hot)
    }

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
        // Held down, it gives a little, like a key.
        let press = self.ease(id.with("press"), if response.pressed { 1.0 } else { 0.0 });
        self.push_layer(Layer::NONE.scaled(rect.center(), 1.0 - 0.035 * press));
        self.gfx.fill_rect(rect, radius::SMALL + 2.0, fill);
        if border.a > 0 {
            self.gfx.stroke_rect(rect, radius::SMALL + 2.0, 1.0, border);
        }
        if response.focus_ring {
            self.focus_ring(rect, radius::SMALL + 2.0);
        }
        let content = button_width(&mut self.gfx, label, icon) - 2.0 * space::M;
        let mut inner = rect.centered(content.min(rect.w - 2.0 * space::S), rect.h);
        if let Some(icon) = icon {
            let slot = inner.take_left(16.0);
            self.gfx.icon(icon, slot.centered(16.0, 16.0), ink);
            inner.take_left(6.0);
        }
        if !label.is_empty() {
            self.strict_text(inner, label, BODY_MEDIUM, ink, Align::Start);
        }
        self.pop_layer();
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
        let press = self.ease(id.with("press"), if response.pressed { 1.0 } else { 0.0 });
        self.push_layer(Layer::NONE.scaled(rect.center(), 1.0 - 0.06 * press));
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
        self.pop_layer();
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
                    KeyOutcome::Copy(text) => self.state.clipboard_out = Some((text, false)),
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
            let at = offset_at(&mut self.gfx, &shown, font, x);
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

    /// A multi-line text field (scripts, notes): Enter starts a new line, Up and Down move
    /// between lines, Home and End go to the ends of the line, and a paste keeps its line
    /// breaks. With [`FieldStyle::wrap`] long lines break at its width, and the keys move
    /// along the rows shown.
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
        let secret = buffer.is_secret();
        // The width rows break at; a secret never wraps, as its dots would break elsewhere
        // than its text.
        let wrap = (style.wrap && !secret).then_some(inner.w - 2.0);
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
                        let text = buffer.as_str();
                        state.cursor = match wrap {
                            Some(width) => {
                                let rows = visual_rows(&mut self.gfx, text, font, Some(width));
                                row_step(&mut self.gfx, text, &rows, (state.cursor, up), font)
                            }
                            None => line_step(text, state.cursor, up),
                        };
                        if !modifiers.shift {
                            state.anchor = state.cursor;
                        }
                    }
                    Typed::Key(key @ (Key::Home | Key::End), modifiers) if !modifiers.ctrl => {
                        let text = buffer.as_str();
                        let (start, end) = match wrap {
                            Some(width) => {
                                let rows = visual_rows(&mut self.gfx, text, font, Some(width));
                                let row = row_of(&rows, state.cursor);
                                (rows[row].0, soft_end(text, &rows, row))
                            }
                            None => line_bounds(text, state.cursor),
                        };
                        state.cursor = if key == Key::Home { start } else { end };
                        if !modifiers.shift {
                            state.anchor = state.cursor;
                        }
                    }
                    Typed::Key(key, modifiers) => {
                        match edit_key(buffer, &mut state, key, modifiers) {
                            KeyOutcome::Changed => changed = true,
                            KeyOutcome::Copy(text) => {
                                self.state.clipboard_out = Some((text, false));
                            }
                            KeyOutcome::Ignored => leftover.push((key, modifiers)),
                            KeyOutcome::Moved | KeyOutcome::Submit => {}
                        }
                    }
                }
            }
            self.state.return_keys(leftover);
        }
        let text = buffer.as_str();
        state.fit(text);
        let rows = self.text_rows(id, text, font, wrap);
        // The caret's row stays in view.
        let caret_row = row_of(&rows, state.cursor);
        #[allow(clippy::cast_precision_loss)] // Scripts and notes are short.
        let caret_y = caret_row as f32 * line_h;
        if caret_y - state.scroll > inner.h - line_h {
            state.scroll = caret_y - inner.h + line_h;
        }
        if caret_y < state.scroll {
            state.scroll = caret_y;
        }
        #[allow(clippy::cast_precision_loss)] // As above.
        let content = rows.len() as f32 * line_h;
        state.scroll -= self.take_wheel(inner);
        state.scroll = state.scroll.clamp(0.0, (content - inner.h).max(0.0));
        if (response.pressed || response.clicked)
            && let Some(pointer) = self.pointer()
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // On screen.
            let row = (((pointer.y - inner.y + state.scroll) / line_h).max(0.0) as usize)
                .min(rows.len() - 1);
            let start = rows[row].0;
            let line = &text[start..soft_end(text, &rows, row)];
            let shown = if secret {
                "\u{2022}".repeat(line.chars().count())
            } else {
                line.to_owned()
            };
            let at = start
                + from_shown(
                    line,
                    secret,
                    offset_at(&mut self.gfx, &shown, font, pointer.x - inner.x),
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
        // A click may have moved the caret to another row.
        let caret_row = row_of(&rows, state.cursor);
        for (row, &(start, end)) in rows.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // Scripts and notes are short.
            let y = inner.y + row as f32 * line_h - state.scroll;
            let line = &text[start..end];
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
                if response.focused && row == caret_row && (start..=end).contains(&state.cursor) {
                    let x = self
                        .gfx
                        .measure(&line_shown[..to_shown(state.cursor - start)], font)
                        .width;
                    self.gfx.fill_rect(
                        Rect::new(inner.x + x, y + 1.0, 1.5, line_h - 2.0),
                        0.0,
                        t.text,
                    );
                }
            }
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

    /// The rows text area `id` shows `text` in ([`visual_rows`]). Wrapped rows are kept
    /// until the text or width changes: measuring every word each frame would be slow.
    fn text_rows(
        &mut self,
        id: Id,
        text: &str,
        font: Font,
        wrap: Option<f32>,
    ) -> Vec<(usize, usize)> {
        use std::hash::{Hash, Hasher};
        let Some(width) = wrap else {
            return visual_rows(&mut self.gfx, text, font, None);
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let fit = (hasher.finish(), width.to_bits());
        if let Some(known) = self.state.wraps.get(&id)
            && known.fit == fit
        {
            return known.rows.clone();
        }
        let rows = visual_rows(&mut self.gfx, text, font, Some(width));
        self.state.wraps.insert(
            id,
            Wrapped {
                fit,
                rows: rows.clone(),
            },
        );
        rows
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
        // Off, the track is dark enough to read as a control on a white card too.
        let off = t.line_strong.mix(t.text3, 0.55);
        self.gfx.fill_rect(track, 10.0, off.mix(t.accent, on));
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
        self.select_with_swatches(id, rect, items, &[], selected, placeholder)
    }

    /// A [`Self::select`] whose items each show `swatches[index]`: a pill in the first color
    /// holding a dot of each of the others (a color scheme in small). The current choice
    /// shows its swatch in the box too.
    pub fn select_with_swatches(
        &mut self,
        id: Id,
        rect: Rect,
        items: &[String],
        swatches: &[Vec<Color>],
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
        if let Some(colors) = selected.and_then(|index| swatches.get(index)) {
            let dots = inner.take_right(swatch_width(colors) + space::S);
            swatch(&mut self.gfx, dots, colors, t.line_strong);
        }
        let ink = if selected.is_some() { t.text } else { t.text3 };
        self.gfx.text(inner, current, BODY, ink, Align::Start);
        if response.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        // Wide enough for the longest name beside its swatch; measured only as it opens.
        let width = if response.clicked && !swatches.is_empty() {
            let dots = swatches.iter().map(|c| swatch_width(c)).fold(0.0, f32::max);
            items
                .iter()
                .map(|item| self.gfx.measure(item, BODY).width)
                .fold(0.0, f32::max)
                + dots
                + 2.0 * space::S
                + 2.0 * space::M
                + 24.0
        } else {
            0.0
        };
        let chosen = self.popup_toggle(id, &response, rect, items, selected, rect.w.max(width));
        if let Some(popup) = self
            .state
            .popup
            .as_mut()
            .filter(|p| p.owner == id && !p.shown)
        {
            popup.swatches = swatches.into();
        }
        chosen
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
            // The item's text, its row's insets and the check mark's column.
            .map(|item| self.gfx.measure(item, BODY).width + 2.0 * space::L + 24.0)
            .fold(rect.w.max(180.0), f32::max);
        self.popup_toggle(id, &response, rect, items, None, width)
    }

    /// The screen's main action and a menu of related ones, joined into one accent-filled
    /// control: `label` on the left does the action, a chevron on the right named
    /// `menu_name` opens `items` under the whole control. Narrower than
    /// [`split_button_width`], the left half shows only its icon and keeps its name for
    /// screen readers and a tooltip.
    pub fn split_button(
        &mut self,
        id: Id,
        rect: Rect,
        label: &str,
        icon: IconId,
        menu_name: &str,
        items: &[String],
    ) -> SplitResponse {
        let t = self.theme;
        let more_id = id.with("menu");
        let mut main = rect;
        let more = main.take_right(SPLIT_MENU_W);
        // Two zones, drawn left to right: Tab reaches the action, then its menu.
        let response = self.zone(id, main, Role::Button, label);
        let toggle = self.zone(more_id, more, Role::Button, menu_name);
        let open = self.menu_open(more_id);
        let corner = radius::SMALL + 2.0;
        let held = response.pressed || toggle.pressed;
        let press = self.ease(id.with("press"), if held { 1.0 } else { 0.0 });
        self.push_layer(Layer::NONE.scaled(rect.center(), 1.0 - 0.035 * press));
        // One shape; each half lights up on its own, clipped so only the outer corners round.
        self.gfx.fill_rect(rect, corner, t.accent);
        for (half, half_id, zone, lit) in [
            (main, id, response, response.hovered),
            (more, more_id, toggle, toggle.hovered || open),
        ] {
            let hover = self.ease(half_id.with("hover"), if lit { 1.0 } else { 0.0 });
            if hover > 0.0 || zone.pressed {
                let fill = t.accent.mix(t.accent_hover, hover);
                let fill = if zone.pressed {
                    fill.mix(t.press, 0.4)
                } else {
                    fill
                };
                self.push_clip(half);
                self.gfx.fill_rect(rect, corner, fill);
                self.pop_clip();
            }
        }
        let divider = Rect::new(more.x, rect.y + 8.0, 1.0, rect.h - 16.0);
        self.gfx.fill_rect(divider, 0.0, t.on_accent.alpha(0.25));
        let needed = button_width(&mut self.gfx, label, Some(icon));
        let full = main.w >= needed;
        if full {
            let mut inner = main.centered(needed - 2.0 * space::M, main.h);
            let slot = inner.take_left(16.0);
            self.gfx.icon(icon, slot.centered(16.0, 16.0), t.on_accent);
            inner.take_left(6.0);
            self.strict_text(inner, label, BODY_MEDIUM, t.on_accent, Align::Start);
        } else {
            self.gfx.icon(icon, main.centered(16.0, 16.0), t.on_accent);
        }
        self.gfx
            .icon(icons::CHEVRON_DOWN, more.centered(16.0, 16.0), t.on_accent);
        if response.focus_ring {
            self.focus_ring(main, corner);
        }
        if toggle.focus_ring {
            self.focus_ring(more, corner);
        }
        self.pop_layer();
        if response.hovered || toggle.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        if !full {
            self.tooltip(id, main, label);
        }
        self.tooltip(more_id, more, menu_name);
        let width = items
            .iter()
            // As in a menu button: the item's text, its row's insets and the check mark's column.
            .map(|item| self.gfx.measure(item, BODY).width + 2.0 * space::L + 24.0)
            .fold(rect.w.max(180.0), f32::max);
        let chosen = self.popup_toggle(more_id, &toggle, rect, items, None, width);
        SplitResponse {
            main: response,
            chosen,
        }
    }

    /// The context menu of zone `owner` (a card, a row) whose `response` this frame is: it opens
    /// at the pointer on a right click, or under the zone on Shift+F10 while the zone has focus.
    /// Returns the index chosen this frame.
    pub fn context_menu(
        &mut self,
        owner: Id,
        response: &Response,
        items: &[String],
    ) -> Option<usize> {
        let chosen =
            (0..items.len()).find(|index| self.state.was_clicked(owner.with(("item", *index))));
        if chosen.is_some() {
            self.state.popup = None;
            self.state.move_focus_to(owner);
            return chosen;
        }
        let keyboard = response.focused && self.key(Key::F(10), Modifiers::SHIFT);
        let anchor = if response.secondary_clicked {
            // Already on screen: the pointer is never inside a layer.
            self.pointer().map(|at| Rect::new(at.x, at.y, 1.0, 1.0))
        } else if keyboard {
            let below = Rect::new(
                response.rect.x + space::M,
                response.rect.b() - 6.0,
                1.0,
                1.0,
            );
            Some(self.gfx.to_screen(below))
        } else {
            None
        };
        if let Some(anchor) = anchor {
            let width = items
                .iter()
                .map(|item| self.gfx.measure(item, BODY).width + 2.0 * space::L + 16.0)
                .fold(200.0, f32::max);
            self.state.popup = Some(Popup::new(owner, anchor, items, None, width));
        }
        None
    }

    /// The menu of `items` that a click on control `id` (its `response` this frame) opens
    /// under `anchor`, `width` wide, with `selected` checked; a control drawn its own way (an
    /// icon button) uses it as [`Self::menu_button`] does. Returns the index chosen this frame.
    pub fn menu_for(
        &mut self,
        id: Id,
        response: &Response,
        anchor: Rect,
        items: &[String],
        selected: Option<usize>,
        width: f32,
    ) -> Option<usize> {
        self.popup_toggle(id, response, anchor, items, selected, width)
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
        let picked = self
            .state
            .popup
            .as_ref()
            .filter(|p| p.owner == id)
            .and_then(|p| p.picked);
        let chosen = picked.or_else(|| {
            (0..items.len()).find(|index| self.state.was_clicked(id.with(("item", *index))))
        });
        if chosen.is_some() {
            self.state.popup = None;
            // Back on the control the menu came from, as the keyboard left it.
            self.state.move_focus_to(id);
            return chosen;
        }
        if response.clicked {
            let open = self.state.popup.as_ref().is_some_and(|p| p.owner == id);
            let anchor = self.gfx.to_screen(anchor);
            self.state.popup = (!open).then(|| Popup::new(id, anchor, items, selected, width));
        }
        None
    }

    /// Draws what floats above the frame: the open menu. Call after everything else.
    pub fn overlays(&mut self) {
        self.draw_toasts();
        let Some(popup) = self.state.popup.clone() else {
            self.draw_tooltip();
            return;
        };
        if self.key(Key::Escape, Modifiers::NONE) {
            self.state.popup = None;
            self.state.move_focus_to(popup.owner);
            return;
        }
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
        // Sized for every item, so the menu holds still while the search narrows it.
        #[allow(clippy::cast_precision_loss)] // Menus hold hundreds of items at most.
        let content = popup.items.len() as f32 * MENU_ITEM
            + 8.0
            + if popup.searchable() { SEARCH_H } else { 0.0 };
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
            (below, room_below.max(MENU_ITEM + 8.0 + SEARCH_H))
        } else {
            (popup.anchor.y - 4.0 - room_above, room_above)
        };
        let x = popup
            .anchor
            .x
            .min(self.viewport.r() - popup.width - 8.0)
            .max(8.0);
        let menu = Rect::new(x, y, popup.width, height);
        // It grows out of the widget that opened it, from the edge nearest to it.
        let open = self.entrance(
            popup.owner.with("menu.open"),
            Motion::new(180, crate::motion::Curve::Out),
        );
        let below = y >= popup.anchor.y;
        let origin = Point::new(
            popup.anchor.center().x.clamp(menu.x, menu.r()),
            if below { menu.y } else { menu.b() },
        );
        self.push_layer(Layer::appear(
            open,
            origin,
            0.9,
            if below { -6.0 } else { 6.0 },
        ));
        self.draw_menu(&popup, menu);
        self.pop_layer();
    }

    /// The search at the top of the open menu `popup`, in `rect`: it has the keyboard as the
    /// menu opens, and Enter in it picks the first item it keeps.
    fn menu_search(&mut self, popup: &Popup, rect: Rect, first: Option<usize>) {
        let id = popup.owner.with("menu.search");
        if !popup.shown {
            self.state.move_focus_to(id);
        }
        let mut query = popup.query.clone();
        let field = self.text_field(
            id,
            rect,
            &mut query,
            FieldStyle {
                placeholder: "Search",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        let Some(open) = self.state.popup.as_mut() else {
            return;
        };
        let submitted = field.submitted.then_some(first).flatten();
        if field.changed {
            open.query = query;
        }
        if submitted.is_some() {
            open.picked = submitted;
        }
        if field.changed {
            // A new list starts at its top.
            self.set_scroll_offset(popup.owner.with("menu.scroll"), 0.0);
        }
        if submitted.is_some() {
            self.request_repaint();
        }
    }

    /// Item `index` of the open menu `popup` in `row`: its name, its swatch, and a check mark
    /// when it is the current choice.
    fn menu_row(&mut self, popup: &Popup, index: usize, row: Rect) -> Response {
        let t = self.theme;
        let item = &popup.items[index];
        let response = self.zone(popup.owner.with(("item", index)), row, Role::MenuItem, item);
        if response.hovered || response.focus_ring {
            self.gfx.fill_rect(row, radius::SMALL, t.hover);
        }
        let mut inner = row.inset(space::S + 2.0, 0.0);
        let check = inner.take_right(16.0);
        if popup.selected == Some(index) {
            self.gfx
                .icon(icons::CHECK, check.centered(14.0, 14.0), t.accent);
        }
        if let Some(colors) = popup.swatches.get(index) {
            let dots = inner.take_right(swatch_width(colors) + space::S);
            swatch(&mut self.gfx, dots, colors, t.line_strong);
        }
        self.gfx.text(inner, item, BODY, t.text, Align::Start);
        if response.hovered {
            self.set_cursor(Cursor::Pointer);
        }
        response
    }

    /// The open menu `popup`'s surface, its search when it is long, and the items the search
    /// keeps, in `menu`.
    fn draw_menu(&mut self, popup: &Popup, menu: Rect) {
        let t = self.theme;
        // A click on the menu between its items (its edge, its scroll bar) stays in the menu
        // rather than reaching the backdrop and closing it.
        self.zone(popup.owner.with("menu.surface"), menu, Role::Backdrop, "");
        self.gfx.shadow(menu, radius::MEDIUM, 16.0, 6.0, t.shadow);
        self.gfx.fill_rect(menu, radius::MEDIUM, t.raised);
        self.gfx.stroke_rect(menu, radius::MEDIUM, 1.0, t.line);
        let mut list = menu.inset(4.0, 4.0);
        let shown = popup.matches();
        if popup.searchable() {
            let search = list.take_top(SEARCH_H).inset(2.0, 0.0);
            let search = Rect::new(search.x, search.y + 2.0, search.w, CONTROL - 4.0);
            self.menu_search(popup, search, shown.first().copied());
        }
        #[allow(clippy::cast_precision_loss)] // As above.
        let content = shown.len() as f32 * MENU_ITEM;
        let scroll_id = popup.owner.with("menu.scroll");
        // The wheel over the menu is the menu's, whatever scrolls under it.
        self.state.wheel.1 = std::mem::take(&mut self.state.menu_wheel);
        let offset = if content > list.h {
            // In view: the item the keyboard is on, or the current choice as the menu opens.
            let focused = shown
                .iter()
                .position(|index| self.state.focus() == Some(popup.owner.with(("item", *index))));
            let current = popup
                .selected
                .filter(|_| !popup.shown)
                .and_then(|selected| shown.iter().position(|index| *index == selected));
            if let Some(at) = focused.or(current) {
                #[allow(clippy::cast_precision_loss)] // As above.
                let top = at as f32 * MENU_ITEM;
                // A row either side stays in view too, so Tab always has a drawn item to go to
                // next and the menu scrolls ahead of it.
                let highest = (top - MENU_ITEM).max(0.0);
                let lowest = (top + 2.0 * MENU_ITEM - list.h).min(highest);
                let now = self.scroll_offset(scroll_id);
                self.set_scroll_offset(scroll_id, now.clamp(lowest, highest));
            }
            self.scroll_view(scroll_id, list, content)
        } else {
            0.0
        };
        if let Some(open) = self.state.popup.as_mut() {
            open.shown = true;
        }
        self.push_clip(list);
        let (mut hovered, mut focused) = (None, None);
        // Rows stop short of the scroll bar, so it never runs over a swatch or a highlight.
        let row_w = if content > list.h {
            list.w - 8.0
        } else {
            list.w
        };
        for (at, &index) in shown.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let row = Rect::new(
                list.x,
                list.y - offset + at as f32 * MENU_ITEM,
                row_w,
                MENU_ITEM,
            );
            if row.b() < list.y || row.y > list.b() {
                continue;
            }
            let response = self.menu_row(popup, index, row);
            if response.hovered {
                hovered = Some(index);
            }
            if response.focused {
                focused = Some(index);
            }
        }
        if shown.is_empty() {
            let note = Rect::new(list.x, list.y, list.w, MENU_ITEM);
            self.gfx.text(
                note.inset(space::S + 2.0, 0.0),
                "No matches",
                BODY,
                t.text2,
                Align::Start,
            );
        }
        self.pop_clip();
        // A new hot item changes what the owner previews: draw it at once.
        let hot = hovered.or(focused);
        if let Some(open) = self.state.popup.as_mut()
            && open.hot != hot
        {
            open.hot = hot;
            self.request_repaint();
        }
    }

    /// Shows `text` in a toast at the window's bottom right for a few seconds (something done
    /// that needs no answer: copied, saved, moved).
    pub fn toast(&mut self, text: impl Into<String>, tone: Tone) {
        // A frame in already, so the frame that shows it is not blank.
        let at = self.time() - crate::motion::FRAME;
        self.state.toasts.push(Toast {
            text: text.into(),
            tone,
            at,
        });
        // A few at most: the oldest go first.
        let extra = self.state.toasts.len().saturating_sub(3);
        self.state.toasts.drain(..extra);
    }

    /// The toasts, newest at the bottom: each slides in from the right, stays, then slides
    /// out; frames are asked for only while one moves.
    fn draw_toasts(&mut self) {
        const IN: f64 = 0.22;
        let now = self.time();
        self.state
            .toasts
            .retain(|toast| now - toast.at < TOAST_LIFE);
        if self.state.toasts.is_empty() {
            return;
        }
        let t = self.theme;
        let toasts = self.state.toasts.clone();
        let mut bottom = self.viewport.b() - 20.0;
        let mut wake: f64 = f64::MAX;
        for toast in toasts.iter().rev() {
            let age = now - toast.at;
            let coming = (age / IN).clamp(0.0, 1.0);
            let going = ((TOAST_LIFE - age) / IN).clamp(0.0, 1.0);
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let shown = crate::motion::Curve::Out.at(coming.min(going) as f32);
            // The next moment it moves: now while it slides, else when it starts to leave.
            wake = wake.min(if coming < 1.0 || going < 1.0 {
                0.016
            } else {
                TOAST_LIFE - IN - age
            });
            let width = (self.gfx.measure(&toast.text, BODY).width + 64.0)
                .min(self.viewport.w - 40.0)
                .max(200.0);
            let rect = Rect::new(self.viewport.r() - 20.0 - width, bottom - 44.0, width, 44.0);
            bottom -= 52.0 * shown;
            self.push_layer(Layer::fade(shown).shifted(28.0 * (1.0 - shown), 0.0));
            self.gfx.shadow(rect, 12.0, 20.0, 6.0, t.shadow);
            self.gfx.fill_rect(rect, 12.0, t.raised);
            self.gfx.stroke_rect(rect, 12.0, 1.0, t.line_strong);
            let color = match toast.tone {
                Tone::Info => t.accent,
                Tone::Done => t.ok,
                Tone::Problem => t.error,
            };
            let mut inner = rect.inset(space::M, 0.0);
            let mark = inner.take_left(20.0).centered(20.0, 20.0);
            self.gfx.fill_circle(mark.center(), 10.0, color.alpha(0.18));
            let glyph = match toast.tone {
                Tone::Problem => icons::WARNING,
                _ => icons::CHECK,
            };
            self.gfx.icon(glyph, mark.centered(13.0, 13.0), color);
            inner.take_left(space::S + 2.0);
            self.gfx
                .text(inner, &toast.text, BODY, t.text, Align::Start);
            self.pop_layer();
        }
        if wake < f64::MAX {
            self.request_repaint_after(std::time::Duration::from_secs_f64(wake.max(0.016)));
        }
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
        // It fades in, rising a touch toward its control.
        let shown = self.entrance(crate::kit::Id::new(("tooltip", &text)), Motion::FAST);
        let toward = if top > anchor.y { 3.0 } else { -3.0 };
        self.push_layer(Layer::appear(shown, tip.center(), 0.97, toward));
        self.gfx.shadow(tip, radius::SMALL, 10.0, 3.0, t.shadow);
        self.gfx.fill_rect(tip, radius::SMALL, t.raised);
        self.gfx.stroke_rect(tip, radius::SMALL, 1.0, t.line_strong);
        self.gfx.text(tip, &text, SMALL, t.text, Align::Center);
        self.pop_layer();
    }

    /// A small uppercase label above a group of rows.
    pub fn section_label(&mut self, rect: Rect, label: &str) {
        let t = self.theme;
        // Quieter than body text, yet readable on the light theme too.
        self.gfx.text(
            rect,
            &label.to_uppercase(),
            LABEL,
            t.text3.mix(t.text2, 0.75),
            Align::Start,
        );
    }

    /// Draws the key chord `chord` ("Ctrl+Shift+K") as keycaps, one per key, from `at` (their
    /// top-left corner), [`KEYCAP_H`] tall; returns the width taken. See [`keys_width`].
    pub fn keys(&mut self, at: Point, chord: &str) -> f32 {
        let t = self.theme;
        let mut x = at.x;
        for (index, key) in chord_keys(chord).iter().enumerate() {
            if index > 0 {
                x += KEYCAP_GAP;
            }
            let width = keycap_width(&mut self.gfx, key);
            let cap = Rect::new(x, at.y, width, KEYCAP_H);
            // A key: its face, and a darker lip along the bottom.
            self.gfx
                .fill_rect(cap.offset(0.0, 1.5), 5.0, t.line_strong.alpha(0.9));
            self.gfx.fill_rect(cap, 5.0, t.surface.mix(t.hover, 0.6));
            self.gfx.stroke_rect(cap, 5.0, 1.0, t.line_strong);
            self.gfx.text(cap, key, KEYCAP, t.text, Align::Center);
            x += width;
        }
        x - at.x
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
    /// while the content is taller than the view. The bar's thumb drags, and a press on its
    /// track brings the thumb under the pointer. Returns the content offset to draw at.
    pub fn scroll_view(&mut self, id: Id, rect: Rect, content_height: f32) -> f32 {
        let max = (content_height - rect.h).max(0.0);
        let mut target = (self.scroll_offset(id) - self.take_wheel(rect)).clamp(0.0, max);
        let track = Rect::new(rect.r() - 6.0, rect.y + 2.0, 4.0, rect.h - 4.0);
        let thumb_h = (rect.h / content_height * track.h).max(24.0).min(track.h);
        let bar = id.with("bar");
        let grab = id.with("grab");
        let (mut held, mut hot) = (false, false);
        if max > 0.0 {
            // Wider than it is drawn, so it is easy to hit.
            let response = self.zone(
                bar,
                Rect::new(rect.r() - 8.0, rect.y, 8.0, rect.h),
                Role::ScrollView,
                "",
            );
            held = self.dragging(bar);
            if let (true, Some(pointer)) = (held, self.pointer()) {
                let screen = self.gfx.to_screen(track);
                let scale = screen.h / track.h;
                // Where on the thumb it was taken: kept for the whole drag, so the thumb does
                // not jump under the pointer; a press on the track takes it by its middle.
                let taken = if let Some(taken) = self.state.scroll.get(&grab) {
                    *taken
                } else {
                    let thumb_y = screen.y + (screen.h - thumb_h * scale) * (target / max);
                    let at = self.pressed_at().map_or(pointer.y, |at| at.y);
                    let on_thumb = (thumb_y..=thumb_y + thumb_h * scale).contains(&at);
                    let taken = if on_thumb {
                        (at - thumb_y) / scale
                    } else {
                        thumb_h / 2.0
                    };
                    self.state.scroll.insert(grab, taken);
                    taken
                };
                let room = track.h - thumb_h;
                if room > 0.0 {
                    let top = (pointer.y - screen.y) / scale - taken;
                    target = (top / room * max).clamp(0.0, max);
                }
            }
            hot = response.hovered || held;
        }
        if !held {
            self.state.scroll.remove(&grab);
        }
        self.set_scroll_offset(id, target);
        // The content glides to where the wheel or the keyboard sent it, and follows a
        // dragged thumb at once.
        if held {
            self.spring_set(id.with("glide"), target);
        }
        let offset = self.spring(id.with("glide"), target).clamp(0.0, max);
        if max > 0.0 {
            let t = self.theme;
            let thumb_y = track.y + (track.h - thumb_h) * (offset / max);
            self.gfx.fill_rect(
                Rect::new(track.x, thumb_y, track.w, thumb_h),
                2.0,
                t.text2.alpha(if hot { 0.7 } else { 0.4 }),
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

/// The rows a text area shows `text` in, as byte ranges without their line breaks: one per
/// line, or with `width`, each line broken after the last word that fits.
fn visual_rows(
    canvas: &mut dyn Canvas,
    text: &str,
    font: Font,
    width: Option<f32>,
) -> Vec<(usize, usize)> {
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        match width {
            Some(width) => wrap_line(canvas, text, (start, end), (font, width), &mut rows),
            None => rows.push((start, end)),
        }
        start = end + 1;
    }
    rows
}

/// Breaks the line `text[start..end]` into rows no wider than `width`. Spaces at a break
/// stay at the end of the row before it; a word wider than a whole row breaks between
/// characters.
fn wrap_line(
    canvas: &mut dyn Canvas,
    text: &str,
    (start, end): (usize, usize),
    (font, width): (Font, f32),
    rows: &mut Vec<(usize, usize)>,
) {
    let mut row = start;
    // The width of `text[row..at]`.
    let mut x = 0.0;
    let mut at = start;
    for word in text[start..end].split_inclusive(char::is_whitespace) {
        let ink = word.trim_end();
        let ink_w = canvas.measure(ink, font).width;
        if at > row && x + ink_w > width {
            rows.push((row, at));
            row = at;
            x = 0.0;
        }
        if ink_w > width {
            for (offset, c) in ink.char_indices() {
                let next = at + offset + c.len_utf8();
                if at + offset > row && canvas.measure(&text[row..next], font).width > width {
                    rows.push((row, at + offset));
                    row = at + offset;
                }
            }
            at += word.len();
            x = canvas.measure(&text[row..at], font).width;
        } else {
            x += canvas.measure(word, font).width;
            at += word.len();
        }
    }
    rows.push((row, end));
}

/// The row of `rows` the caret at `at` shows on: at a soft break, the row it starts.
fn row_of(rows: &[(usize, usize)], at: usize) -> usize {
    rows.iter()
        .rposition(|(start, _)| *start <= at)
        .unwrap_or(0)
}

/// The furthest the caret goes on row `row` and still shows on it: before the space a soft
/// break leaves at its end, else the row's end.
fn soft_end(text: &str, rows: &[(usize, usize)], row: usize) -> usize {
    let end = rows[row].1;
    if rows.get(row + 1).is_some_and(|next| next.0 == end) {
        prev_char(text, end)
    } else {
        end
    }
}

/// The offset one row up or down from `at`, at the same distance from the left edge; the
/// ends of the text past the first and last row.
fn row_step(
    canvas: &mut dyn Canvas,
    text: &str,
    rows: &[(usize, usize)],
    (at, up): (usize, bool),
    font: Font,
) -> usize {
    let row = row_of(rows, at);
    let x = canvas.measure(&text[rows[row].0..at], font).width;
    let target = if up {
        let Some(above) = row.checked_sub(1) else {
            return 0;
        };
        above
    } else {
        if row + 1 >= rows.len() {
            return text.len();
        }
        row + 1
    };
    let start = rows[target].0;
    start + offset_at(canvas, &text[start..soft_end(text, rows, target)], font, x)
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
    fn a_wrapping_text_area_breaks_lines_at_its_width_and_draws_every_character() {
        let mut state = UiState::default();
        let mut notes = String::from(
            "nginx front end, restart it with care after a deploy; certs at \
             https://example.com/a-long-path-without-any-spaces-in-it\nsecond line",
        );
        let area = Rect::new(10.0, 10.0, 200.0, 280.0);
        let mut canvas = Recorder::default();
        state.begin_frame(&[], 0.0);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        let style = FieldStyle {
            wrap: true,
            ..FieldStyle::default()
        };
        ui.text_area(Id::new("notes"), area, &mut notes, style);
        // The recorder's characters are 7 wide; spaces at a break may hang past the edge.
        let inside = area.w - 2.0 * (space::M - 2.0);
        let rows: Vec<&str> = canvas.texts.iter().map(|(_, row)| row.as_str()).collect();
        #[allow(clippy::cast_precision_loss)] // Short rows.
        let widest = rows
            .iter()
            .map(|row| row.trim_end().chars().count() as f32 * 7.0)
            .fold(0.0, f32::max);
        assert!(widest <= inside, "{rows:?}");
        assert_eq!(rows.concat(), notes.replace('\n', ""));
        assert!(rows.len() > 4, "{rows:?}");
    }

    #[test]
    fn end_up_and_down_keep_to_wrapped_rows() {
        let mut state = UiState::default();
        let id = Id::new("notes");
        let style = FieldStyle {
            wrap: true,
            ..FieldStyle::default()
        };
        // Five words of four letters fill a row: "aaaa bbbb cccc dddd eeee " is 25 long.
        let mut notes = String::from("aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj");
        frame(&mut state, &[], |ui| {
            ui.text_area(id, FIELD, &mut notes, style);
        });
        state.set_focus(Some(id), true);
        let mut press = |state: &mut UiState, pressed: Key| {
            frame(state, &[key(pressed, Modifiers::NONE)], |ui| {
                ui.text_area(id, FIELD, &mut notes, style);
            });
            state.edit(id).cursor
        };
        // End stops before the space at the break, so the caret stays on the first row.
        assert_eq!(press(&mut state, Key::End), 24);
        assert_eq!(
            press(&mut state, Key::Down),
            49,
            "the same place on the next row"
        );
        assert_eq!(press(&mut state, Key::Up), 24);
        assert_eq!(press(&mut state, Key::Home), 0);
        assert_eq!(press(&mut state, Key::Up), 0);
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
    fn a_split_buttons_halves_act_apart_and_its_menu_opens_under_all_of_it() {
        let mut state = UiState::default();
        let id = Id::new("new");
        let items = vec!["New group".to_owned(), "Import".to_owned()];
        let rect = Rect::new(20.0, 40.0, 140.0, 32.0);
        // Time moves, so the menu has finished opening when an item is clicked.
        let mut time = 0.0;
        let mut split = |state: &mut UiState, events: &[Event]| {
            time += 0.5;
            let mut canvas = Recorder::default();
            state.begin_frame(events, time);
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                state,
                Rect::new(0.0, 0.0, 400.0, 300.0),
            );
            let out = ui.split_button(id, rect, "New host", icons::PLUS, "More", &items);
            ui.overlays();
            state.end_frame();
            out
        };
        let click = |at: Point| {
            [
                Event::PointerMoved(at),
                Event::PointerDown(crate::kit::Button::Primary),
                Event::PointerUp(crate::kit::Button::Primary),
            ]
        };
        let menu = |state: &UiState| -> Vec<Zone> {
            state
                .zones()
                .iter()
                .filter(|zone| zone.role == Role::MenuItem)
                .cloned()
                .collect()
        };
        split(&mut state, &[]);
        let action = split(
            &mut state,
            &click(Point::new(rect.x + 20.0, rect.center().y)),
        );
        split(&mut state, &[]);
        assert!(action.main.clicked && action.chosen.is_none());
        assert!(menu(&state).is_empty(), "the action opens no menu");

        let chevron = Point::new(rect.r() - SPLIT_MENU_W / 2.0, rect.center().y);
        assert!(!split(&mut state, &click(chevron)).main.clicked);
        split(&mut state, &[]);
        let items_shown = menu(&state);
        assert_eq!(items_shown.len(), 2);
        assert!(
            items_shown
                .iter()
                .all(|zone| zone.rect.x < rect.r() - SPLIT_MENU_W && zone.rect.y > rect.center().y),
            "under the whole control, from its left edge: {items_shown:?}"
        );
        let chosen = split(&mut state, &click(items_shown[1].rect.center()));
        assert_eq!(chosen.chosen, Some(1));
        assert!(!chosen.main.clicked);
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
    fn the_keyboard_reaches_every_item_of_a_long_menu_and_enter_picks_it() {
        let mut state = UiState::default();
        let id = Id::new("scheme");
        let items: Vec<String> = (0..500).map(|n| format!("scheme-{n}")).collect();
        let field = Rect::new(10.0, 40.0, 200.0, 32.0);
        let mut chosen = None;
        // Time moves, so the menu's scrolling glide arrives as it would in a window.
        let mut time = 0.0;
        let mut draw = |state: &mut UiState, events: &[Event]| {
            time += 0.5;
            let mut canvas = Recorder::default();
            state.begin_frame(events, time);
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                state,
                Rect::new(0.0, 0.0, 400.0, 300.0),
            );
            if let Some(index) = ui.select(id, field, &items, Some(0), "Scheme") {
                chosen = Some(index);
            }
            ui.overlays();
            state.end_frame();
        };
        draw(&mut state, &[]);
        state.set_focus(Some(id), true);
        draw(&mut state, &[key(Key::Enter, Modifiers::NONE)]);
        draw(&mut state, &[]);
        let target = id.with(("item", 40_usize));
        for _ in 0..60 {
            if state.focus() == Some(target) {
                break;
            }
            draw(&mut state, &[key(Key::Tab, Modifiers::NONE)]);
            draw(&mut state, &[]);
        }
        let on = state
            .zones()
            .iter()
            .find(|zone| Some(zone.id) == state.focus())
            .map(|zone| zone.label.clone());
        assert_eq!(
            state.focus(),
            Some(target),
            "Tab walks past the first screen: on {on:?}"
        );
        let window = Rect::new(0.0, 0.0, 400.0, 300.0);
        let zone = state
            .zones()
            .iter()
            .find(|zone| zone.id == target)
            .expect("the focused item is drawn");
        assert!(
            zone.rect.y >= window.y && zone.rect.b() <= window.b(),
            "and in view"
        );
        draw(&mut state, &[key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(chosen, Some(40));
    }

    #[test]
    fn the_wheel_scrolls_an_open_menu_and_not_the_page_under_it() {
        let mut state = UiState::default();
        let (id, page) = (Id::new("scheme"), Id::new("page"));
        let items: Vec<String> = (0..500).map(|n| format!("scheme-{n}")).collect();
        let field = Rect::new(10.0, 40.0, 200.0, 32.0);
        let draw = |state: &mut UiState, events: &[Event]| {
            frame(state, events, |ui| {
                // A page taller than the window, scrolling under the menu.
                ui.scroll_view(page, Rect::new(0.0, 0.0, 400.0, 300.0), 2000.0);
                ui.select(id, field, &items, Some(0), "Scheme");
            });
        };
        draw(&mut state, &[]);
        draw(
            &mut state,
            &[
                Event::PointerMoved(field.center()),
                Event::PointerDown(crate::kit::Button::Primary),
                Event::PointerUp(crate::kit::Button::Primary),
            ],
        );
        draw(&mut state, &[]);
        let over_menu = Point::new(field.center().x, field.b() + 120.0);
        draw(
            &mut state,
            &[
                Event::PointerMoved(over_menu),
                Event::Wheel { x: 0.0, y: -90.0 },
            ],
        );
        let scrolled = |state: &UiState, id: Id| state.scroll.get(&id).copied().unwrap_or(0.0);
        assert!((scrolled(&state, id.with("menu.scroll")) - 90.0).abs() < 0.5);
        assert!(scrolled(&state, page).abs() < 0.5, "the page stays put");
        // Closed, the wheel is the page's again.
        draw(&mut state, &[key(Key::Escape, Modifiers::NONE)]);
        draw(&mut state, &[Event::Wheel { x: 0.0, y: -90.0 }]);
        assert!((scrolled(&state, page) - 90.0).abs() < 0.5);
    }

    #[test]
    fn a_menus_scroll_bar_drags_and_jumps_and_keeps_the_menu_open() {
        let mut state = UiState::default();
        let id = Id::new("scheme");
        let items: Vec<String> = (0..500).map(|n| format!("scheme-{n}")).collect();
        let field = Rect::new(10.0, 40.0, 200.0, 32.0);
        let draw = |state: &mut UiState, events: &[Event]| {
            frame(state, events, |ui| {
                ui.select(id, field, &items, Some(0), "Scheme");
            });
        };
        draw(&mut state, &[]);
        draw(
            &mut state,
            &[
                Event::PointerMoved(field.center()),
                Event::PointerDown(crate::kit::Button::Primary),
                Event::PointerUp(crate::kit::Button::Primary),
            ],
        );
        draw(&mut state, &[]);
        let scroll = id.with("menu.scroll");
        let bar = state
            .zones()
            .iter()
            .find(|zone| zone.id == scroll.with("bar"))
            .map(|zone| zone.rect)
            .expect("a long menu's scroll bar takes the pointer");
        let scrolled = |state: &UiState| state.scroll.get(&scroll).copied().unwrap_or(0.0);
        let open = |state: &UiState| state.popup.is_some();
        // The thumb, at the top, dragged to the bottom: the menu reaches its last items.
        let thumb = Point::new(bar.center().x, bar.y + 6.0);
        let bottom = Point::new(bar.center().x, bar.b() + 40.0);
        draw(
            &mut state,
            &[
                Event::PointerMoved(thumb),
                Event::PointerDown(crate::kit::Button::Primary),
            ],
        );
        draw(&mut state, &[Event::PointerMoved(bottom)]);
        draw(&mut state, &[]);
        assert!(
            state.zones().iter().any(|zone| zone.label == "scheme-499"),
            "dragged to the end: {}",
            scrolled(&state)
        );
        let max = scrolled(&state);
        draw(&mut state, &[Event::PointerUp(crate::kit::Button::Primary)]);
        assert!(open(&state), "a drag on the bar keeps the menu open");
        // A click on the track halfway down brings the thumb there.
        draw(&mut state, &[Event::PointerMoved(bar.center())]);
        draw(
            &mut state,
            &[Event::PointerDown(crate::kit::Button::Primary)],
        );
        draw(&mut state, &[Event::PointerUp(crate::kit::Button::Primary)]);
        let half = scrolled(&state) / max;
        assert!((0.4..0.6).contains(&half), "halfway: {half}");
        assert!(open(&state), "a click on the bar keeps the menu open");
        // A click on the menu's own edge, between its items, keeps it open too.
        let edge = Point::new(bar.x - 100.0, bar.b() + 2.0);
        draw(&mut state, &[Event::PointerMoved(edge)]);
        draw(
            &mut state,
            &[Event::PointerDown(crate::kit::Button::Primary)],
        );
        draw(&mut state, &[Event::PointerUp(crate::kit::Button::Primary)]);
        assert!(open(&state), "a click on the menu's edge keeps it open");
    }

    #[test]
    fn a_long_menu_searches_as_you_type_enter_picks_and_hover_is_reported() {
        let mut state = UiState::default();
        let id = Id::new("scheme");
        let items: Vec<String> = (0..500).map(|n| format!("Scheme {n}")).collect();
        let swatches: Vec<Vec<Color>> = items
            .iter()
            .map(|_| vec![Color::BLACK, Color::WHITE])
            .collect();
        let field = Rect::new(10.0, 40.0, 200.0, 32.0);
        let chosen = std::cell::Cell::new(None);
        let hot = std::cell::Cell::new(None);
        let draw = |state: &mut UiState, events: &[Event]| {
            frame(state, events, |ui| {
                hot.set(ui.menu_hot(id));
                if let Some(index) =
                    ui.select_with_swatches(id, field, &items, &swatches, Some(0), "Scheme")
                {
                    chosen.set(Some(index));
                }
            });
        };
        draw(&mut state, &[]);
        state.set_focus(Some(id), true);
        draw(&mut state, &[key(Key::Enter, Modifiers::NONE)]);
        draw(&mut state, &[]);
        assert_eq!(
            state.focus(),
            Some(id.with("menu.search")),
            "the search has the keyboard"
        );
        // Case and word order do not matter.
        draw(&mut state, &[Event::Text("EME 17".into())]);
        draw(&mut state, &[]);
        let drawn: Vec<String> = state
            .zones()
            .iter()
            .filter(|zone| zone.role == Role::MenuItem)
            .map(|zone| zone.label.clone())
            .collect();
        assert_eq!(drawn.first().map(String::as_str), Some("Scheme 17"));
        assert!(drawn.iter().all(|name| name.contains("17")), "{drawn:?}");
        // The pointer on an item makes it the one to preview.
        let second = state
            .zones()
            .iter()
            .find(|zone| zone.label == "Scheme 117")
            .map(|zone| zone.rect.center())
            .expect("drawn");
        draw(&mut state, &[Event::PointerMoved(second)]);
        draw(&mut state, &[]);
        assert_eq!(hot.get(), Some(117));
        draw(&mut state, &[Event::PointerLeft]);
        draw(&mut state, &[key(Key::Enter, Modifiers::NONE)]);
        draw(&mut state, &[]);
        assert_eq!(chosen.get(), Some(17), "Enter picks the first match");
        assert_eq!(state.focus(), Some(id), "back on the select");
        assert_eq!(hot.get(), None, "nothing to preview once closed");
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
    fn a_toast_slides_in_stays_without_asking_for_frames_then_goes() {
        let mut state = UiState::default();
        let at = |state: &mut UiState, time: f64, show: bool| {
            let mut canvas = Recorder::default();
            state.begin_frame(&[], time);
            let mut ui = Ui::new(
                &mut canvas,
                Theme::new(Appearance::Dark),
                state,
                Rect::new(0.0, 0.0, 400.0, 300.0),
            );
            if show {
                ui.toast("Copied the ssh command", Tone::Done);
            }
            ui.overlays();
            let output = state.end_frame();
            let drawn = canvas.texts.iter().any(|(_, text)| text.contains("Copied"));
            (drawn, output.repaint_after)
        };
        let (drawn, next) = at(&mut state, 1.0, true);
        assert!(drawn);
        assert!(
            next.is_some_and(|after| after.as_secs_f64() < 0.05),
            "sliding in"
        );
        // Resting: the next frame it needs is when it starts to leave.
        let (drawn, next) = at(&mut state, 2.0, false);
        assert!(drawn);
        assert!(
            next.is_some_and(|after| after.as_secs_f64() > 1.0),
            "{next:?}"
        );
        let (drawn, next) = at(&mut state, 1.0 + TOAST_LIFE + 0.1, false);
        assert!(!drawn && next.is_none(), "gone, and nothing more to draw");
    }

    #[test]
    fn a_chord_splits_into_its_keys_even_when_a_key_is_plus() {
        assert_eq!(chord_keys("Ctrl+Shift+K"), ["Ctrl", "Shift", "K"]);
        assert_eq!(chord_keys("Ctrl+="), ["Ctrl", "="]);
        assert_eq!(chord_keys("Ctrl++"), ["Ctrl", "+"]);
        assert_eq!(chord_keys("+"), ["+"]);
        assert_eq!(chord_keys("Enter"), ["Enter"]);
        assert_eq!(chord_keys(""), Vec::<&str>::new());
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
