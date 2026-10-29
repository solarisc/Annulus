//! Choosing one of a few options: a closed drop-down (select) and a segmented control.

use annulus_config::tokens::{metrics, radius, space, text};

use super::{centered_baseline, color, is_activation_key, is_key, text_on};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Point, Rect, Size};
use crate::icons::CHEVRON_DOWN;
use crate::input::{Button, Event, Handled, Key, Modifiers};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// A closed drop-down showing the chosen option. Click, Enter, Space or Alt+Down reports
/// [`Action::Open`] (the menu itself is an overlay). Up and Down change the choice
/// directly and report [`Action::Select`].
#[derive(Debug)]
pub struct Select {
    label: String,
    options: Vec<String>,
    selected: usize,
}

impl Select {
    /// A select named `label` with `options`, showing option `selected` (clamped).
    #[must_use]
    pub fn new(label: impl Into<String>, options: Vec<String>, selected: usize) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            label: label.into(),
            options,
            selected,
        }
    }

    /// The chosen option's index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }
}

impl Widget for Select {
    fn name(&self) -> &'static str {
        "select"
    }

    fn role(&self) -> Role {
        Role::ComboBox
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let widest = self
            .options
            .iter()
            .map(|option| measure.measure(option, text::BODY, None).width)
            .fold(0.0, f32::max);
        Size::new(widest + 2.0 * space::M + 16.0, metrics::CONTROL_HEIGHT)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        let fill = if cx.state.hovered {
            palette.bg_hover
        } else {
            palette.bg_surface
        };
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::SMALL,
            color: color(fill),
        });
        let border = if cx.state.focused {
            palette.accent
        } else {
            palette.border_subtle
        };
        list.push(Op::Border {
            rect: bounds,
            radius: radius::SMALL,
            width: 1.0,
            color: color(border),
        });
        if let Some(value) = self.options.get(self.selected) {
            list.push(Op::TextRun {
                origin: Point::new(
                    bounds.origin.x + space::M,
                    centered_baseline(bounds, text::BODY),
                ),
                text: value.clone(),
                size: text::BODY,
                color: color(palette.text_primary),
            });
        }
        let chevron = Rect::new(
            bounds.max_x() - space::M - 12.0,
            bounds.origin.y + (bounds.size.height - 12.0) / 2.0,
            12.0,
            12.0,
        );
        list.push(Op::Icon {
            icon: CHEVRON_DOWN,
            rect: chevron,
            color: color(palette.text_secondary),
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let alt_down = matches!(
            event,
            Event::KeyDown {
                key: Key::ArrowDown,
                modifiers: Modifiers { alt: true, .. }
            }
        );
        if matches!(
            event,
            Event::PointerDown {
                button: Button::Primary,
                ..
            }
        ) || alt_down
            || is_activation_key(event)
        {
            cx.emit(Action::Open);
            return Handled::Yes;
        }
        let step = if is_key(event, Key::ArrowDown) {
            Some(1)
        } else if is_key(event, Key::ArrowUp) {
            Some(-1)
        } else {
            None
        };
        if let Some(step) = step {
            if let Some(next) = moved(self.selected, step, self.options.len())
                && next != self.selected
            {
                self.selected = next;
                cx.emit(Action::Select(next));
            }
            return Handled::Yes;
        }
        Handled::No
    }
}

/// A row of equally wide options, one of them chosen. Clicking a segment or Left/Right
/// reports [`Action::Select`].
#[derive(Debug)]
pub struct Segmented {
    label: String,
    options: Vec<String>,
    selected: usize,
}

impl Segmented {
    /// A segmented control named `label`, with option `selected` (clamped) chosen.
    #[must_use]
    pub fn new(label: impl Into<String>, options: Vec<String>, selected: usize) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            label: label.into(),
            options,
            selected,
        }
    }

    /// The chosen option's index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[allow(clippy::cast_precision_loss)] // A handful of segments.
    fn segment_width(&self, bounds: Rect) -> f32 {
        bounds.size.width / self.options.len().max(1) as f32
    }

    fn choose(&mut self, index: usize, cx: &mut EventCx) {
        if index != self.selected && index < self.options.len() {
            self.selected = index;
            cx.emit(Action::Select(index));
        }
    }
}

impl Widget for Segmented {
    fn name(&self) -> &'static str {
        "segmented"
    }

    fn role(&self) -> Role {
        Role::RadioGroup
    }

    fn label(&self) -> Option<&str> {
        Some(&self.label)
    }

    #[allow(clippy::cast_precision_loss)] // A handful of segments.
    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let widest = self
            .options
            .iter()
            .map(|option| measure.measure(option, text::BODY, None).width)
            .fold(0.0, f32::max);
        Size::new(
            (widest + 2.0 * space::M) * self.options.len() as f32,
            metrics::CONTROL_HEIGHT,
        )
    }

    #[allow(clippy::cast_precision_loss)] // A handful of segments.
    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::MEDIUM,
            color: color(palette.bg_hover),
        });
        let width = self.segment_width(bounds);
        for (index, option) in self.options.iter().enumerate() {
            let segment = Rect::new(
                bounds.origin.x + width * index as f32,
                bounds.origin.y,
                width,
                bounds.size.height,
            );
            let ink = if index == self.selected {
                let pill = Rect::new(
                    segment.origin.x + 2.0,
                    segment.origin.y + 2.0,
                    segment.size.width - 4.0,
                    segment.size.height - 4.0,
                );
                list.push(Op::RoundedRect {
                    rect: pill,
                    radius: radius::MEDIUM - 2.0,
                    color: color(palette.accent),
                });
                text_on(palette.accent, palette)
            } else {
                palette.text_primary
            };
            let label = cx.text.measure(option, text::BODY, None);
            list.push(Op::TextRun {
                origin: Point::new(
                    segment.origin.x + (width - label.width) / 2.0,
                    centered_baseline(segment, text::BODY),
                ),
                text: option.clone(),
                size: text::BODY,
                color: color(ink),
            });
        }
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        match event {
            Event::PointerDown {
                position,
                button: Button::Primary,
            } => {
                let width = self.segment_width(cx.bounds);
                if width > 0.0 {
                    let offset = (position.x - cx.bounds.origin.x) / width;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // Floored, clamped below.
                    let index = offset.max(0.0).floor() as usize;
                    self.choose(index.min(self.options.len().saturating_sub(1)), cx);
                }
                Handled::Yes
            }
            _ if is_key(event, Key::ArrowLeft) || is_key(event, Key::ArrowRight) => {
                let step = if is_key(event, Key::ArrowLeft) { -1 } else { 1 };
                if let Some(next) = moved(self.selected, step, self.options.len()) {
                    self.choose(next, cx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// `index` moved by `step`, kept inside `0..len`; `None` when there are no options.
fn moved(index: usize, step: isize, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(index.saturating_add_signed(step).min(last))
}
