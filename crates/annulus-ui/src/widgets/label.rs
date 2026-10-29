//! Static text: labels and tooltips.

use annulus_config::tokens::{radius, space, text};

use super::color;
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Size};
use crate::widget::{PaintCx, Role, TextMeasure, Widget};

/// A container with no look of its own, for grouping and laying out children.
#[derive(Debug, Default)]
pub struct Group;

impl Widget for Group {
    fn name(&self) -> &'static str {
        "group"
    }

    fn paint(&self, _cx: &mut PaintCx<'_>, _list: &mut DisplayList) {}
}

/// One line of text.
#[derive(Debug)]
pub struct Label {
    text: String,
    size: f32,
    secondary: bool,
}

impl Label {
    /// Body text.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            size: text::BODY,
            secondary: false,
        }
    }

    /// Smaller, dimmer text for hints and metadata.
    #[must_use]
    pub fn secondary(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            size: text::SECONDARY,
            secondary: true,
        }
    }

    /// A heading at `size` (use the `tokens::text` sizes).
    #[must_use]
    pub fn heading(text: impl Into<String>, size: f32) -> Self {
        Self {
            text: text.into(),
            size,
            secondary: false,
        }
    }
}

impl Widget for Label {
    fn name(&self) -> &'static str {
        "label"
    }

    fn role(&self) -> Role {
        Role::Label
    }

    fn label(&self) -> Option<&str> {
        Some(&self.text)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        measure.measure(&self.text, self.size, None)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let ink = if self.secondary {
            cx.palette.text_secondary
        } else {
            cx.palette.text_primary
        };
        // A line box is about 1.25 font sizes high; the baseline sits one font size down.
        let baseline = cx.bounds.origin.y + self.size;
        list.push(Op::TextRun {
            origin: Point::new(cx.bounds.origin.x, baseline),
            text: self.text.clone(),
            size: self.size,
            color: color(ink),
        });
    }
}

/// A short description in a raised box, shown as an overlay near the control it explains.
#[derive(Debug)]
pub struct Tooltip {
    text: String,
}

impl Tooltip {
    /// A tooltip saying `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Widget for Tooltip {
    fn name(&self) -> &'static str {
        "tooltip"
    }

    fn role(&self) -> Role {
        Role::Tooltip
    }

    fn label(&self) -> Option<&str> {
        Some(&self.text)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let size = measure.measure(&self.text, text::SECONDARY, None);
        Size::new(size.width + 2.0 * space::S, size.height + 2.0 * space::XS)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::Shadow {
            rect: bounds,
            radius: radius::SMALL,
            offset: Point::new(0.0, 2.0),
            blur: 6.0,
            color: crate::geometry::Color::rgba(0, 0, 0, 60),
        });
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::SMALL,
            color: color(palette.bg_raised),
        });
        list.push(Op::Border {
            rect: bounds,
            radius: radius::SMALL,
            width: 1.0,
            color: color(palette.border_subtle),
        });
        list.push(Op::TextRun {
            origin: Point::new(
                bounds.origin.x + space::S,
                bounds.origin.y + space::XS + text::SECONDARY,
            ),
            text: self.text.clone(),
            size: text::SECONDARY,
            color: color(palette.text_primary),
        });
    }
}
