//! The widget trait and the contexts it works in: what a node measures, draws, how it reacts
//! to input, and how assistive technology sees it (roles map to AccessKit).

use annulus_config::tokens::Palette;

use crate::display_list::DisplayList;
use crate::geometry::{Rect, Size};
use crate::input::{Event, Handled};

/// Measures text for layout. The CPU backend's text engine and DirectWrite implement it;
/// layout tests use a fake with fixed advances (ADR-0012 §6).
pub trait TextMeasure {
    /// The size of `text` at font `size` (DIPs), wrapped at `max_width` if given.
    fn measure(&mut self, text: &str, size: f32, max_width: Option<f32>) -> Size;
}

/// How a node is exposed to screen readers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// A container with no meaning of its own.
    #[default]
    Group,
    /// Static text.
    Label,
    /// A push button.
    Button,
    /// A check box.
    CheckBox,
    /// An on/off switch.
    Switch,
    /// One option of a radio group.
    RadioButton,
    /// A one-line text input.
    TextInput,
    /// A text input whose content is hidden.
    PasswordInput,
    /// A text input for searching.
    SearchInput,
    /// A closed drop-down list.
    ComboBox,
    /// A row of mutually exclusive options.
    RadioGroup,
    /// A short description shown on hover or focus.
    Tooltip,
    /// A modal dialog.
    Dialog,
}

/// What the tree knows about a node when it is painted.
#[allow(clippy::struct_excessive_bools)] // Independent interaction states, each read on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WidgetState {
    /// The pointer is over it.
    pub hovered: bool,
    /// A press on it has not been released yet.
    pub pressed: bool,
    /// It has keyboard focus.
    pub focused: bool,
    /// The tree draws a focus ring around it (keyboard focus).
    pub focus_visible: bool,
}

/// Everything a widget needs to draw itself.
pub struct PaintCx<'a> {
    /// Where the node is.
    pub bounds: Rect,
    /// The active theme's colors.
    pub palette: &'a Palette,
    /// Hover, press and focus.
    pub state: WidgetState,
    /// Measures text, e.g. to place a caret.
    pub text: &'a mut dyn TextMeasure,
}

/// Something the user did to a widget, for the app's view models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A button was clicked or pressed with Enter or Space.
    Activate,
    /// A check box or switch was set to this value.
    Toggle(bool),
    /// The text of a field changed.
    Edit,
    /// The option at this index was chosen.
    Select(usize),
    /// The widget asks to open its list or menu (selects, split buttons).
    Open,
    /// A dialog, menu or banner asks to be closed (Escape, close button).
    Dismiss,
}

/// Lets a widget report actions while it handles an event.
#[derive(Debug, Default)]
pub struct EventCx {
    /// Where the node is, for pointer math.
    pub bounds: Rect,
    actions: Vec<Action>,
}

impl EventCx {
    /// A context for a node at `bounds`.
    #[must_use]
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            actions: Vec::new(),
        }
    }

    /// Reports `action` to the app.
    pub fn emit(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// The actions reported so far.
    #[must_use]
    pub fn into_actions(self) -> Vec<Action> {
        self.actions
    }
}

/// The behavior of one node in the tree. Widgets hold only their own visual state; the view
/// models in `annulus-app` own app state (DESIGN §7.2).
pub trait Widget {
    /// A short name for debugging and test output, e.g. `"button"`.
    fn name(&self) -> &'static str;

    /// How screen readers see it.
    fn role(&self) -> Role {
        Role::Group
    }

    /// The accessible name, usually the visible label.
    fn label(&self) -> Option<&str> {
        None
    }

    /// The checked state of a check box, switch or radio button; `None` for other widgets.
    fn checked(&self) -> Option<bool> {
        None
    }

    /// The current value screen readers announce (field text, chosen option). Password
    /// fields return `None`: their secret is never exposed.
    fn value(&self) -> Option<String> {
        None
    }

    /// The size this widget's own content needs (text, icon), not counting padding or
    /// children, when at most `available` is free (either side may be infinite). Containers
    /// keep the default of zero.
    fn content_size(&self, available: Size, text: &mut dyn TextMeasure) -> Size {
        let _ = (available, text);
        Size::default()
    }

    /// Appends this widget's own drawing (not its children's).
    fn paint(&self, cx: &mut PaintCx<'_>, list: &mut DisplayList);

    /// Whether this node is a modal overlay: while it is open, Tab stays inside it and
    /// pointer input outside it goes to it instead of the app underneath.
    fn modal(&self) -> bool {
        false
    }

    /// Whether Tab and clicks can focus this widget.
    fn focusable(&self) -> bool {
        false
    }

    /// Reacts to an event routed to this node. Unhandled events bubble to the parent.
    fn handle(&mut self, event: &Event, cx: &mut EventCx) -> Handled {
        let _ = (event, cx);
        Handled::No
    }
}
