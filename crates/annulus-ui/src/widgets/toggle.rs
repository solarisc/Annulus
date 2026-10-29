//! Two-state controls: check boxes, switches and radio buttons. Each shows its label to the
//! right of the control, and the whole row is the click target.

use annulus_config::tokens::{radius, space, text};

use super::{centered_baseline, color, is_activation_key, is_click, text_on};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Rect, Size};
use crate::icons::CHECK;
use crate::input::{Button, Event, Handled};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

const BOX: f32 = 16.0;
const TRACK: Size = Size::new(32.0, 18.0);

/// A check box. Click or Space flips it and reports [`Action::Toggle`] with the new value.
#[derive(Debug)]
pub struct Checkbox {
    label: String,
    checked: bool,
}

impl Checkbox {
    /// A check box labeled `label`.
    #[must_use]
    pub fn new(label: impl Into<String>, checked: bool) -> Self {
        Self {
            label: label.into(),
            checked,
        }
    }

    /// Whether it is checked.
    #[must_use]
    pub fn checked(&self) -> bool {
        self.checked
    }
}

impl Widget for Checkbox {
    fn name(&self) -> &'static str {
        "checkbox"
    }

    fn role(&self) -> Role {
        Role::CheckBox
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        row_size(BOX, &self.label, measure)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let control = control_rect(cx.bounds, Size::new(BOX, BOX));
        if self.checked {
            list.push(Op::RoundedRect {
                rect: control,
                radius: radius::SMALL,
                color: color(palette.accent),
            });
            let ink = color(text_on(palette.accent, palette));
            list.push(Op::Icon {
                icon: CHECK,
                rect: inset(control, 2.0),
                color: ink,
            });
        } else {
            let fill = if cx.state.hovered {
                palette.bg_hover
            } else {
                palette.bg_surface
            };
            list.push(Op::RoundedRect {
                rect: control,
                radius: radius::SMALL,
                color: color(fill),
            });
            list.push(Op::Border {
                rect: control,
                radius: radius::SMALL,
                width: 1.5,
                color: color(palette.text_secondary),
            });
        }
        paint_label(cx, &self.label, BOX, list);
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        on_toggle(event, cx, || {
            self.checked = !self.checked;
            Some(Action::Toggle(self.checked))
        })
    }
}

/// An on/off switch, for settings that apply at once. Click, Space or Enter flips it and
/// reports [`Action::Toggle`].
#[derive(Debug)]
pub struct Switch {
    label: String,
    on: bool,
}

impl Switch {
    /// A switch labeled `label`.
    #[must_use]
    pub fn new(label: impl Into<String>, on: bool) -> Self {
        Self {
            label: label.into(),
            on,
        }
    }

    /// Whether it is on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on
    }
}

impl Widget for Switch {
    fn name(&self) -> &'static str {
        "switch"
    }

    fn role(&self) -> Role {
        Role::Switch
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        row_size(TRACK.width, &self.label, measure)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let track = control_rect(cx.bounds, TRACK);
        let fill = if self.on {
            palette.accent
        } else {
            palette.text_secondary
        };
        list.push(Op::RoundedRect {
            rect: track,
            radius: TRACK.height / 2.0,
            color: color(fill),
        });
        let knob_side = TRACK.height - 4.0;
        let knob_x = if self.on {
            track.max_x() - knob_side - 2.0
        } else {
            track.origin.x + 2.0
        };
        list.push(Op::RoundedRect {
            rect: Rect::new(knob_x, track.origin.y + 2.0, knob_side, knob_side),
            radius: knob_side / 2.0,
            color: color(palette.bg_surface),
        });
        paint_label(cx, &self.label, TRACK.width, list);
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        on_toggle(event, cx, || {
            self.on = !self.on;
            Some(Action::Toggle(self.on))
        })
    }
}

/// One option of a radio group. Choosing an unselected option reports
/// [`Action::Select`] with its index; the app then updates the whole group.
#[derive(Debug)]
pub struct Radio {
    label: String,
    index: usize,
    selected: bool,
}

impl Radio {
    /// Option number `index` of its group, labeled `label`.
    #[must_use]
    pub fn new(label: impl Into<String>, index: usize, selected: bool) -> Self {
        Self {
            label: label.into(),
            index,
            selected,
        }
    }

    /// Marks this option as the group's choice, or not.
    pub fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }
}

impl Widget for Radio {
    fn name(&self) -> &'static str {
        "radio"
    }

    fn role(&self) -> Role {
        Role::RadioButton
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        row_size(BOX, &self.label, measure)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let control = control_rect(cx.bounds, Size::new(BOX, BOX));
        let ring = if self.selected {
            palette.accent
        } else {
            palette.text_secondary
        };
        list.push(Op::Border {
            rect: control,
            radius: BOX / 2.0,
            width: 1.5,
            color: color(ring),
        });
        if self.selected {
            list.push(Op::RoundedRect {
                rect: inset(control, 4.0),
                radius: BOX / 2.0,
                color: color(palette.accent),
            });
        }
        paint_label(cx, &self.label, BOX, list);
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let (selected, index) = (self.selected, self.index);
        on_toggle(event, cx, || (!selected).then_some(Action::Select(index)))
    }
}

/// Shared input of toggles: take the press, and on a click inside or Space/Enter run `flip`
/// and report what it returns.
fn on_toggle(event: &Event, cx: &mut EventCx, flip: impl FnOnce() -> Option<Action>) -> Handled {
    if matches!(
        event,
        Event::PointerDown {
            button: Button::Primary,
            ..
        }
    ) {
        return Handled::Yes;
    }
    if is_click(event, cx.bounds) || is_activation_key(event) {
        if let Some(action) = flip() {
            cx.emit(action);
        }
        return Handled::Yes;
    }
    Handled::No
}

/// A control followed by its label: control width, gap, label width; at least the control's
/// height.
fn row_size(control_width: f32, label: &str, measure: &mut dyn TextMeasure) -> Size {
    let label = measure.measure(label, text::BODY, None);
    Size::new(
        control_width + space::S + label.width,
        label.height.max(TRACK.height),
    )
}

/// The control's rectangle: at the left edge, centered vertically.
fn control_rect(bounds: Rect, size: Size) -> Rect {
    Rect::new(
        bounds.origin.x,
        bounds.origin.y + (bounds.size.height - size.height) / 2.0,
        size.width,
        size.height,
    )
}

fn paint_label(cx: &PaintCx<'_>, label: &str, control_width: f32, list: &mut DisplayList) {
    list.push(Op::TextRun {
        origin: Point::new(
            cx.bounds.origin.x + control_width + space::S,
            centered_baseline(cx.bounds, text::BODY),
        ),
        text: label.to_owned(),
        size: text::BODY,
        color: color(cx.palette.text_primary),
    });
}

fn inset(rect: Rect, by: f32) -> Rect {
    Rect::new(
        rect.origin.x + by,
        rect.origin.y + by,
        rect.size.width - 2.0 * by,
        rect.size.height - 2.0 * by,
    )
}
