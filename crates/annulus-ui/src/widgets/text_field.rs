//! One-line text fields: plain, password and search. Editing is the shared
//! [`TextEditor`]; this file adds keys, drawing and the caret.

use unicode_segmentation::UnicodeSegmentation;
use annulus_config::tokens::{metrics, radius, space, text};

use super::{centered_baseline, color, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Rect, Size};
use crate::icons::SEARCH;
use crate::input::{Button, Event, Handled, Key, Modifiers};
use crate::text_edit::{FieldKind, TextEditor};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// Room for the search icon before the text.
const ICON_SPACE: f32 = 24.0;

/// Narrowest a field gets from its content.
const MIN_WIDTH: f32 = 160.0;

/// Which kind of field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldStyle {
    /// Ordinary text.
    Text,
    /// A password or passphrase: shown as bullets, kept in zeroizing memory, never copied.
    Password,
    /// A search box: a magnifier, and Escape clears it.
    Search,
}

/// A one-line text field. Every change reports [`Action::Edit`]; Enter reports
/// [`Action::Activate`] (submit).
#[derive(Debug)]
pub struct TextField {
    editor: TextEditor,
    style: FieldStyle,
    placeholder: String,
}

impl TextField {
    /// An empty field. `placeholder` shows while it is empty and is its accessible name.
    #[must_use]
    pub fn new(style: FieldStyle, placeholder: impl Into<String>) -> Self {
        let kind = if style == FieldStyle::Password {
            FieldKind::Secret
        } else {
            FieldKind::Plain
        };
        Self {
            editor: TextEditor::new(kind),
            style,
            placeholder: placeholder.into(),
        }
    }

    /// The text (for a password field, the secret itself: hand it only to the code that
    /// needs it).
    #[must_use]
    pub fn text(&self) -> &str {
        self.editor.text()
    }

    /// The editing state, for the clipboard commands of the app.
    pub fn editor_mut(&mut self) -> &mut TextEditor {
        &mut self.editor
    }

    fn text_left(&self, bounds: Rect) -> f32 {
        let icon = if self.style == FieldStyle::Search {
            ICON_SPACE
        } else {
            0.0
        };
        bounds.origin.x + space::S + icon
    }

    /// Applies an editing key. Returns whether the key was used and whether the text changed.
    fn edit_key(&mut self, key: Key, modifiers: Modifiers) -> (bool, bool) {
        let extend = modifiers.shift;
        let command = modifiers.ctrl || modifiers.meta;
        let before = self.editor.text().len();
        let editor = &mut self.editor;
        match key {
            Key::Backspace => editor.backspace(),
            Key::Delete => editor.delete(),
            Key::ArrowLeft if command || modifiers.alt => editor.move_word_left(extend),
            Key::ArrowRight if command || modifiers.alt => editor.move_word_right(extend),
            Key::ArrowLeft => editor.move_left(extend),
            Key::ArrowRight => editor.move_right(extend),
            Key::Home => editor.move_home(extend),
            Key::End => editor.move_end(extend),
            Key::Character('a') if command => editor.select_all(),
            Key::Character('z') if command && modifiers.shift => return (true, editor.redo()),
            Key::Character('z') if command => return (true, editor.undo()),
            Key::Character('y') if command => return (true, editor.redo()),
            _ => return (false, false),
        }
        let changed =
            matches!(key, Key::Backspace | Key::Delete) && self.editor.text().len() != before;
        (true, changed)
    }
}

impl Widget for TextField {
    fn name(&self) -> &'static str {
        "text-field"
    }

    fn role(&self) -> Role {
        match self.style {
            FieldStyle::Text => Role::TextInput,
            FieldStyle::Password => Role::PasswordInput,
            FieldStyle::Search => Role::SearchInput,
        }
    }

    fn label(&self) -> Option<&str> {
        Some(&self.placeholder)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let shown = measure.measure(&self.editor.display_text(), text::BODY, None);
        let icon = if self.style == FieldStyle::Search {
            ICON_SPACE
        } else {
            0.0
        };
        Size::new(
            (shown.width + icon + 2.0 * space::S).max(MIN_WIDTH),
            metrics::CONTROL_HEIGHT,
        )
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::SMALL,
            color: color(palette.bg_surface),
        });
        let (border, width) = if cx.state.focused {
            (palette.accent, 1.5)
        } else {
            (palette.border_subtle, 1.0)
        };
        list.push(Op::Border {
            rect: bounds,
            radius: radius::SMALL,
            width,
            color: color(border),
        });
        if self.style == FieldStyle::Search {
            let icon = Rect::new(
                bounds.origin.x + space::S,
                bounds.origin.y + (bounds.size.height - 16.0) / 2.0,
                16.0,
                16.0,
            );
            list.push(Op::Icon {
                icon: SEARCH,
                rect: icon,
                color: color(palette.text_secondary),
            });
        }

        let left = self.text_left(bounds);
        let baseline = centered_baseline(bounds, text::BODY);
        let shown = self.editor.display_text();
        list.clip(bounds, |list| {
            if shown.is_empty() {
                list.push(Op::TextRun {
                    origin: Point::new(left, baseline),
                    text: self.placeholder.clone(),
                    size: text::BODY,
                    color: color(palette.text_secondary),
                });
            } else {
                let selection = self.editor.selection();
                if !selection.is_empty() && cx.state.focused {
                    let start = left
                        + cx.text
                            .measure(&self.shown_prefix(selection.start), text::BODY, None)
                            .width;
                    let end = left
                        + cx.text
                            .measure(&self.shown_prefix(selection.end), text::BODY, None)
                            .width;
                    let height = bounds.size.height - 12.0;
                    list.push(Op::Rect {
                        rect: Rect::new(start, bounds.origin.y + 6.0, end - start, height),
                        color: color(palette.bg_selected),
                    });
                }
                list.push(Op::TextRun {
                    origin: Point::new(left, baseline),
                    text: shown,
                    size: text::BODY,
                    color: color(palette.text_primary),
                });
            }
            if cx.state.focused {
                let caret_x = left
                    + cx.text
                        .measure(&self.shown_prefix(self.editor.cursor()), text::BODY, None)
                        .width;
                list.push(Op::Rect {
                    rect: Rect::new(
                        caret_x,
                        bounds.origin.y + 7.0,
                        1.5,
                        bounds.size.height - 14.0,
                    ),
                    color: color(palette.text_primary),
                });
            }
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        match event {
            Event::PointerDown {
                button: Button::Primary,
                ..
            } => Handled::Yes,
            Event::Text(typed) => {
                self.editor.insert(typed);
                if !typed.is_empty() {
                    cx.emit(Action::Edit);
                }
                Handled::Yes
            }
            _ if is_key(event, Key::Enter) => {
                cx.emit(Action::Activate);
                Handled::Yes
            }
            _ if self.style == FieldStyle::Search
                && is_key(event, Key::Escape)
                && !self.editor.text().is_empty() =>
            {
                self.editor.clear();
                cx.emit(Action::Edit);
                Handled::Yes
            }
            Event::KeyDown { key, modifiers } => {
                let (used, changed) = self.edit_key(*key, *modifiers);
                if changed {
                    cx.emit(Action::Edit);
                }
                if used { Handled::Yes } else { Handled::No }
            }
            _ => Handled::No,
        }
    }
}

impl TextField {
    /// What the field shows before byte offset `end` of the text: the text itself, or one
    /// bullet per grapheme for a password.
    fn shown_prefix(&self, end: usize) -> String {
        let prefix = self.editor.text().get(..end).unwrap_or_default();
        match self.style {
            FieldStyle::Password => "\u{2022}".repeat(prefix.graphemes(true).count()),
            FieldStyle::Text | FieldStyle::Search => prefix.to_owned(),
        }
    }
}
