//! Modal dialogs: a scrim that confines input, and the raised panel inside it.

use annulus_config::tokens::{radius, space, text};

use super::{color, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Color, Point};
use crate::input::{Event, Handled, Key};
use crate::style::{Align, Edges, Justify, Length, Position, Style};
use crate::widget::{Action, EventCx, PaintCx, Role, Widget};

/// A full-window overlay that dims the app and keeps focus and pointer input inside it.
/// Escape reports [`Action::Dismiss`]. A click on the dimmed area does nothing, so a stray
/// click never throws away a half-filled form.
#[derive(Debug)]
pub struct Modal {
    title: String,
}

impl Modal {
    /// A modal dialog titled `title` (its accessible name).
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    /// The style a modal needs: an overlay covering its parent, centering its panel.
    #[must_use]
    pub fn style() -> Style {
        Style {
            position: Position::Overlay { x: 0.0, y: 0.0 },
            width: Length::Fill(1.0),
            height: Length::Fill(1.0),
            justify: Justify::Center,
            align: Align::Center,
            ..Style::default()
        }
    }
}

impl Widget for Modal {
    fn name(&self) -> &'static str {
        "modal"
    }

    fn role(&self) -> Role {
        Role::Dialog
    }

    fn label(&self) -> Option<&str> {
        Some(&self.title)
    }

    fn modal(&self) -> bool {
        true
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        list.push(Op::Rect {
            rect: cx.bounds,
            color: Color::rgba(0, 0, 0, 110),
        });
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        if is_key(event, Key::Escape) {
            cx.emit(Action::Dismiss);
            return Handled::Yes;
        }
        // Pointer input outside the panel lands here; swallowing it keeps the app inert.
        match event {
            Event::PointerDown { .. } | Event::PointerUp { .. } | Event::Wheel { .. } => {
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// The raised panel of a dialog, with its title at the top. Its children (fields,
/// buttons) are laid out below the title.
#[derive(Debug)]
pub struct DialogPanel {
    title: String,
}

impl DialogPanel {
    /// A panel headed `title`.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    /// The style a panel needs: padded, with room for the title, `width` DIPs wide.
    #[must_use]
    pub fn style(width: f32) -> Style {
        let mut padding = Edges::all(space::XL);
        padding.top = space::XL + text::TITLE + space::L;
        Style {
            width: Length::Fixed(width),
            padding,
            gap: space::M,
            align: Align::Stretch,
            ..Style::default()
        }
    }
}

impl Widget for DialogPanel {
    fn name(&self) -> &'static str {
        "dialog-panel"
    }

    fn label(&self) -> Option<&str> {
        Some(&self.title)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::Shadow {
            rect: bounds,
            radius: radius::LARGE,
            offset: Point::new(0.0, 8.0),
            blur: 24.0,
            color: Color::rgba(0, 0, 0, 90),
        });
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::LARGE,
            color: color(palette.bg_raised),
        });
        list.push(Op::Border {
            rect: bounds,
            radius: radius::LARGE,
            width: 1.0,
            color: color(palette.border_subtle),
        });
        list.push(Op::TextRun {
            origin: Point::new(
                bounds.origin.x + space::XL,
                bounds.origin.y + space::XL + text::TITLE,
            ),
            text: self.title.clone(),
            size: text::TITLE,
            color: color(palette.text_primary),
        });
    }
}
