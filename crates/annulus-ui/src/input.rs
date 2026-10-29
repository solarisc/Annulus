//! Input events as the toolkit sees them, independent of the windowing library. The platform
//! layer translates OS events into these.

use crate::geometry::Point;

/// Which pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Primary (usually left).
    Primary,
    /// Secondary (usually right): context menus.
    Secondary,
    /// Middle: closes tabs.
    Middle,
}

/// Modifier keys held during an event.
#[allow(clippy::struct_excessive_bools)] // One flag per physical modifier key, as the OS reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// Windows key or Command.
    pub meta: bool,
}

/// A key, by meaning rather than by physical position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Tab.
    Tab,
    /// Enter or Return.
    Enter,
    /// Escape.
    Escape,
    /// Space bar.
    Space,
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Arrow up.
    ArrowUp,
    /// Arrow down.
    ArrowDown,
    /// Arrow left.
    ArrowLeft,
    /// Arrow right.
    ArrowRight,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// A key that produces this character without modifiers, for shortcuts.
    Character(char),
}

/// One input event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A button went down at a point.
    PointerDown {
        /// Where, in DIPs.
        position: Point,
        /// Which button.
        button: Button,
    },
    /// A button went up at a point.
    PointerUp {
        /// Where, in DIPs.
        position: Point,
        /// Which button.
        button: Button,
    },
    /// The pointer moved.
    PointerMove {
        /// Where, in DIPs.
        position: Point,
    },
    /// The wheel or touchpad scrolled. Positive `delta` scrolls the content up (shows
    /// what is further down or right).
    Wheel {
        /// Where the pointer is.
        position: Point,
        /// Distance in DIPs along the scrolling container's axis.
        delta: f32,
    },
    /// A key went down (or repeats).
    KeyDown {
        /// Which key.
        key: Key,
        /// Modifiers held.
        modifiers: Modifiers,
    },
    /// Committed text input (after IME composition).
    Text(String),
}

impl Event {
    /// The pointer position, for pointer and wheel events.
    #[must_use]
    pub fn position(&self) -> Option<Point> {
        match self {
            Event::PointerDown { position, .. }
            | Event::PointerUp { position, .. }
            | Event::PointerMove { position }
            | Event::Wheel { position, .. } => Some(*position),
            Event::KeyDown { .. } | Event::Text(_) => None,
        }
    }
}

/// Whether a widget used an event. Unhandled events bubble to the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handled {
    /// The widget used it; stop here.
    Yes,
    /// Pass it on to the parent.
    No,
}
