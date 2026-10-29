//! Notices: inline banners and short-lived toasts, plus the queue that decides which toasts
//! show. Errors never time out, so a failure can't vanish before the user has seen it.

use std::collections::VecDeque;

use annulus_config::tokens::{radius, space, text};

use super::{centered_baseline, color, is_activation_key, is_click, is_key};
use crate::display_list::{DisplayList, Op};
use crate::geometry::{Color, Point, Rect, Size};
use crate::icons::CLOSE;
use crate::input::{Button, Event, Handled, Key};
use crate::widget::{Action, EventCx, PaintCx, Role, TextMeasure, Widget};

/// How serious a notice is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Neutral information.
    Info,
    /// Something worked.
    Success,
    /// Something needs attention.
    Warning,
    /// Something failed.
    Error,
}

/// Width of the close button area at a banner's right edge.
const CLOSE_AREA: f32 = 32.0;

/// A full-width message above content, with a close button that reports
/// [`Action::Dismiss`] (as does Escape while it has focus).
#[derive(Debug)]
pub struct Banner {
    message: String,
    severity: Severity,
}

impl Banner {
    /// A banner saying `message`.
    #[must_use]
    pub fn new(message: impl Into<String>, severity: Severity) -> Self {
        Self {
            message: message.into(),
            severity,
        }
    }
}

impl Widget for Banner {
    fn name(&self) -> &'static str {
        "banner"
    }

    fn role(&self) -> Role {
        Role::Alert
    }

    fn label(&self) -> Option<&str> {
        Some(&self.message)
    }

    fn content_size(&self, _available: Size, measure: &mut dyn TextMeasure) -> Size {
        let message = measure.measure(&self.message, text::BODY, None);
        Size::new(
            message.width + 2.0 * space::M + CLOSE_AREA,
            message.height + 2.0 * space::S,
        )
    }

    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList) {
        let palette = cx.palette;
        let bounds = cx.bounds;
        let tone = tone(self.severity, palette);
        list.push(Op::RoundedRect {
            rect: bounds,
            radius: radius::MEDIUM,
            color: Color {
                a: 40,
                ..color(tone)
            },
        });
        list.push(Op::Rect {
            rect: Rect::new(bounds.origin.x, bounds.origin.y, 3.0, bounds.size.height),
            color: color(tone),
        });
        list.push(Op::TextRun {
            origin: Point::new(
                bounds.origin.x + space::M,
                centered_baseline(bounds, text::BODY),
            ),
            text: self.message.clone(),
            size: text::BODY,
            color: color(palette.text_primary),
        });
        let close = close_area(bounds);
        let icon = Rect::new(
            close.origin.x + 10.0,
            close.origin.y + (close.size.height - 12.0) / 2.0,
            12.0,
            12.0,
        );
        list.push(Op::Icon {
            icon: CLOSE,
            rect: icon,
            color: color(palette.text_secondary),
        });
    }

    fn focusable(&self) -> bool {
        true
    }

    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        if matches!(
            event,
            Event::PointerDown {
                button: Button::Primary,
                ..
            }
        ) {
            return Handled::Yes;
        }
        if is_click(event, close_area(cx.bounds))
            || is_key(event, Key::Escape)
            || is_activation_key(event)
        {
            cx.emit(Action::Dismiss);
            return Handled::Yes;
        }
        Handled::No
    }
}

fn close_area(bounds: Rect) -> Rect {
    Rect::new(
        bounds.max_x() - CLOSE_AREA,
        bounds.origin.y,
        CLOSE_AREA,
        bounds.size.height,
    )
}

fn tone(
    severity: Severity,
    palette: &annulus_config::tokens::Palette,
) -> annulus_config::tokens::Rgb {
    match severity {
        Severity::Info => palette.accent,
        Severity::Success => palette.status_ok,
        Severity::Warning => palette.status_warn,
        Severity::Error => palette.status_error,
    }
}

/// One queued toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// Identifies it for dismissal.
    pub id: u64,
    /// What it says.
    pub message: String,
    /// How serious it is.
    pub severity: Severity,
    /// When it disappears on its own, in ms; `None` for errors, which stay until dismissed.
    expires_ms: Option<u64>,
}

/// Which toasts are on screen. Time is passed in (ms from any origin), so tests are exact.
#[derive(Debug, Default)]
pub struct ToastQueue {
    toasts: VecDeque<Toast>,
    next_id: u64,
}

impl ToastQueue {
    /// At most this many toasts show; older ones make room.
    pub const MAX_VISIBLE: usize = 3;
    /// How long a non-error toast stays.
    pub const LIFETIME_MS: u64 = 5000;

    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `message` at `now_ms` and returns its id. When the stack is full the oldest
    /// toast that can expire makes room; errors are only pushed out by newer errors.
    pub fn push(&mut self, message: impl Into<String>, severity: Severity, now_ms: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let expires_ms =
            (severity != Severity::Error).then(|| now_ms.saturating_add(Self::LIFETIME_MS));
        self.toasts.push_back(Toast {
            id,
            message: message.into(),
            severity,
            expires_ms,
        });
        while self.toasts.len() > Self::MAX_VISIBLE {
            let oldest_transient = self
                .toasts
                .iter()
                .position(|toast| toast.expires_ms.is_some());
            self.toasts.remove(oldest_transient.unwrap_or(0));
        }
        id
    }

    /// Removes toast `id`, as when the user closes it.
    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    /// Drops expired toasts and returns the ones to show, oldest first.
    pub fn visible(&mut self, now_ms: u64) -> Vec<Toast> {
        self.toasts
            .retain(|toast| toast.expires_ms.is_none_or(|expires| now_ms < expires));
        self.toasts.iter().cloned().collect()
    }

    /// When the next toast expires, so the app can wake up then instead of polling.
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
        self.toasts
            .iter()
            .filter_map(|toast| toast.expires_ms)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_toasts_expire_and_errors_stay() {
        let mut queue = ToastQueue::new();
        queue.push("Copied", Severity::Success, 0);
        queue.push("Connection lost", Severity::Error, 0);
        assert_eq!(queue.next_deadline(), Some(ToastQueue::LIFETIME_MS));
        let later: Vec<_> = queue
            .visible(ToastQueue::LIFETIME_MS)
            .into_iter()
            .map(|t| t.message)
            .collect();
        assert_eq!(later, ["Connection lost"]);
        assert_eq!(queue.next_deadline(), None, "nothing left to wake up for");
    }

    #[test]
    fn a_full_stack_drops_the_oldest_transient_toast_not_an_error() {
        let mut queue = ToastQueue::new();
        queue.push("Sync failed", Severity::Error, 0);
        queue.push("Copied", Severity::Success, 1);
        queue.push("Saved", Severity::Success, 2);
        queue.push("Key generated", Severity::Info, 3);
        let shown: Vec<_> = queue.visible(4).into_iter().map(|t| t.message).collect();
        assert_eq!(shown, ["Sync failed", "Saved", "Key generated"]);
    }

    #[test]
    fn dismissed_toast_is_gone() {
        let mut queue = ToastQueue::new();
        let id = queue.push("Sync failed", Severity::Error, 0);
        queue.dismiss(id);
        assert_eq!(queue.visible(1), []);
    }
}
