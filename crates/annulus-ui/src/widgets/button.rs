//! Push buttons: text buttons in four kinds, icon-only buttons and split buttons.

use annulus_config::tokens::{metrics, radius, space, text};

use super::{centered_baseline, color, is_activation_key, is_click, text_on};
use crate::display_list::{DisplayList, IconId, Op};
use crate::geometry::{Color, Point, Rect, Size};
use crate::input::{Event, Handled, Key, Modifiers};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// How prominent a button is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action of a view: filled with the accent.
    Primary,
    /// A normal action: outlined surface.
    Secondary,
    /// A low-key action: no fill until hovered.
    Ghost,
    /// A destructive action: filled with the error color.
    Danger,
}

/// The icon drawn on a split button's menu part.
pub const CHEVRON_DOWN: IconId = IconId(1);

/// Width of a split button's menu part, in DIPs.
const SPLIT_MENU_WIDTH: f32 = 28.0;

/// A text button. Click, Enter or Space reports [`Action::Activate`].
#[derive(Debug)]
pub struct Button {
    label: String,
    kind: ButtonKind,
}

impl Button {
    /// A button showing `label`.
    #[must_use]
    pub fn new(label: impl Into<String>, kind: ButtonKind) -> Self {
        Self {
            label: label.into(),
            kind,
        }
    }
}

impl Widget for Button {
    fn name(&self) -> &'static str {
        "button"
    }

    fn role(&self) -> Role {
        Role::Button
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let label = measure.measure(&self.label, text::BODY, None);
        Size::new(
            label.width + 2.0 * space::M,
            metrics::CONTROL_HEIGHT.max(label.height),
        )
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let ink = paint_frame(cx, self.kind, list);
        let label = cx.text.measure(&self.label, text::BODY, None);
        let x = cx.bounds.origin.x + (cx.bounds.size.width - label.width) / 2.0;
        list.push(Op::TextRun {
            origin: Point::new(x, centered_baseline(cx.bounds, text::BODY)),
            text: self.label.clone(),
            size: text::BODY,
            color: ink,
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        activate_on_click_or_key(event, cx)
    }
}

/// A square button showing only an icon. `label` is its accessible name and tooltip text.
#[derive(Debug)]
pub struct IconButton {
    icon: IconId,
    label: String,
}

impl IconButton {
    /// An icon button named `label` for screen readers.
    #[must_use]
    pub fn new(icon: IconId, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
        }
    }
}

impl Widget for IconButton {
    fn name(&self) -> &'static str {
        "icon-button"
    }

    fn role(&self) -> Role {
        Role::Button
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, _measure: &mut dyn TextMeasure) -> Size {
        Size::new(metrics::CONTROL_HEIGHT, metrics::CONTROL_HEIGHT)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let ink = paint_frame(cx, ButtonKind::Ghost, list);
        list.push(Op::Icon {
            icon: self.icon,
            rect: centered_square(cx.bounds, 16.0),
            color: ink,
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        activate_on_click_or_key(event, cx)
    }
}

/// A button with a main part that reports [`Action::Activate`] and a menu part (the chevron
/// on the right, or Alt+Down) that reports [`Action::Open`].
#[derive(Debug)]
pub struct SplitButton {
    label: String,
    kind: ButtonKind,
}

impl SplitButton {
    /// A split button showing `label`.
    #[must_use]
    pub fn new(label: impl Into<String>, kind: ButtonKind) -> Self {
        Self {
            label: label.into(),
            kind,
        }
    }
}

impl Widget for SplitButton {
    fn name(&self) -> &'static str {
        "split-button"
    }

    fn role(&self) -> Role {
        Role::Button
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let label = measure.measure(&self.label, text::BODY, None);
        Size::new(
            label.width + 2.0 * space::M + SPLIT_MENU_WIDTH,
            metrics::CONTROL_HEIGHT,
        )
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let ink = paint_frame(cx, self.kind, list);
        let bounds = cx.bounds;
        let main_width = (bounds.size.width - SPLIT_MENU_WIDTH).max(0.0);
        let label = cx.text.measure(&self.label, text::BODY, None);
        list.push(Op::TextRun {
            origin: Point::new(
                bounds.origin.x + (main_width - label.width) / 2.0,
                centered_baseline(bounds, text::BODY),
            ),
            text: self.label.clone(),
            size: text::BODY,
            color: ink,
        });
        let divider_x = bounds.origin.x + main_width;
        list.push(Op::Rect {
            rect: Rect::new(
                divider_x,
                bounds.origin.y + 6.0,
                1.0,
                bounds.size.height - 12.0,
            ),
            color: Color { a: 90, ..ink },
        });
        let menu = Rect::new(
            divider_x,
            bounds.origin.y,
            SPLIT_MENU_WIDTH,
            bounds.size.height,
        );
        list.push(Op::Icon {
            icon: CHEVRON_DOWN,
            rect: centered_square(menu, 12.0),
            color: ink,
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let opens_menu = matches!(
            event,
            Event::KeyDown {
                key: Key::ArrowDown,
                modifiers: Modifiers { alt: true, .. }
            }
        );
        let menu = Rect::new(
            cx.bounds.max_x() - SPLIT_MENU_WIDTH,
            cx.bounds.origin.y,
            SPLIT_MENU_WIDTH,
            cx.bounds.size.height,
        );
        if opens_menu || is_click(event, menu) {
            cx.emit(Action::Open);
            return Handled::Yes;
        }
        activate_on_click_or_key(event, cx)
    }
}

/// Shared input behavior of buttons: a press is taken (so the release comes back here), a
/// release inside or Enter/Space activates.
fn activate_on_click_or_key(event: &Event, cx: &mut EventCx) -> Handled {
    if matches!(
        event,
        Event::PointerDown {
            button: crate::input::Button::Primary,
            ..
        }
    ) {
        return Handled::Yes;
    }
    if is_click(event, cx.bounds) || is_activation_key(event) {
        cx.emit(Action::Activate);
        return Handled::Yes;
    }
    Handled::No
}

/// Draws a button's background and border for its kind and state; returns the text color.
fn paint_frame(cx: &PaintCx<'_>, kind: ButtonKind, list: &mut DisplayList) -> Color {
    let palette = cx.palette;
    let state = cx.state;
    let (fill, border) = match kind {
        ButtonKind::Primary => (Some(palette.accent), None),
        ButtonKind::Danger => (Some(palette.status_error), None),
        ButtonKind::Secondary => (
            Some(if state.hovered || state.pressed {
                palette.bg_hover
            } else {
                palette.bg_surface
            }),
            Some(palette.border_subtle),
        ),
        ButtonKind::Ghost => (
            (state.hovered || state.pressed).then_some(palette.bg_hover),
            None,
        ),
    };
    let bounds = cx.bounds;
    if let Some(fill) = fill {
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::MEDIUM,
            color: color(fill),
        });
    }
    if let Some(border) = border {
        list.push(Op::Border {
            rect: bounds,
            radius: radius::MEDIUM,
            width: 1.0,
            color: color(border),
        });
    }
    // Filled kinds show hover and press as a translucent veil, so they keep their hue.
    if matches!(kind, ButtonKind::Primary | ButtonKind::Danger) && (state.hovered || state.pressed)
    {
        let alpha = if state.pressed { 48 } else { 24 };
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::MEDIUM,
            color: Color::rgba(0, 0, 0, alpha),
        });
    }
    match fill {
        Some(fill) if matches!(kind, ButtonKind::Primary | ButtonKind::Danger) => {
            color(text_on(fill, palette))
        }
        _ => color(palette.text_primary),
    }
}

fn centered_square(bounds: Rect, side: f32) -> Rect {
    Rect::new(
        bounds.origin.x + (bounds.size.width - side) / 2.0,
        bounds.origin.y + (bounds.size.height - side) / 2.0,
        side,
        side,
    )
}
