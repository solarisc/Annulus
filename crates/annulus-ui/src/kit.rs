//! The immediate-mode toolkit ([ADR-0017] decision 2). Each frame, views paint through a
//! [`Canvas`] and register interactive [`Zone`]s. Input is resolved at the start of the frame
//! against the zones of the frame on screen, so what is clicked is always what was drawn.
//!
//! [ADR-0017]: ../../docs/adr/0017-native-ui.md

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use annulus_config::tokens::{Appearance, MOTION_MS, Palette};

use crate::gfx::{Canvas, Color, Point, Rect};
use crate::widgets::{EditState, Popup};

/// Identifies a zone across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub u64);

impl Id {
    /// The id of `source`.
    #[must_use]
    pub fn new(source: impl Hash) -> Self {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        Id(hasher.finish())
    }

    /// A child id: this one combined with `part`.
    #[must_use]
    pub fn with(self, part: impl Hash) -> Self {
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        part.hash(&mut hasher);
        Id(hasher.finish())
    }
}

/// What a zone is, for screen readers and keyboard behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Activates something.
    Button,
    /// Minimize, maximize and close: clickable and named, but never in the Tab order, like
    /// the OS's own window buttons.
    WindowButton,
    /// One tab of a tab strip or navigation.
    Tab,
    /// Single-line text input.
    TextInput,
    /// A check box or switch.
    CheckBox,
    /// A drop-down choice.
    ComboBox,
    /// One row or card of a list.
    ListItem,
    /// One entry of a menu.
    MenuItem,
    /// A region that scrolls.
    ScrollView,
    /// A draggable divider.
    Splitter,
    /// Takes the pointer but is not interactive (the title bar's drag area, a backdrop).
    Backdrop,
    /// A terminal: focusable, and while focused every key goes to the program, Tab and
    /// Enter included.
    Terminal,
}

impl Role {
    /// Whether Tab can move focus to a zone with this role.
    #[must_use]
    pub fn focusable(self) -> bool {
        !matches!(
            self,
            Role::ScrollView | Role::Backdrop | Role::Splitter | Role::WindowButton
        )
    }
}

/// One interactive region of a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// Stable id.
    pub id: Id,
    /// Where it is on screen, already clipped.
    pub rect: Rect,
    /// What it is.
    pub role: Role,
    /// Its accessible name.
    pub label: String,
    /// A check box's or switch's state.
    pub toggled: Option<bool>,
    /// A field's text or a drop-down's choice, for assistive technology; never a secret's.
    pub value: Option<String>,
    /// A secret field: assistive technology is told so and never gets its text.
    pub secret: bool,
}

impl Zone {
    /// A zone with nothing more to tell assistive technology than its role and name.
    #[must_use]
    pub fn new(id: Id, rect: Rect, role: Role, label: &str) -> Self {
        Self {
            id,
            rect,
            role,
            label: label.to_owned(),
            toggled: None,
            value: None,
            secret: false,
        }
    }
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    /// Left, usually.
    Primary,
    /// Right, usually: context menus.
    Secondary,
    /// The wheel button: closes tabs.
    Middle,
}

/// A key the toolkit or the app reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Tab.
    Tab,
    /// Enter / Return.
    Enter,
    /// Escape.
    Escape,
    /// Space bar.
    Space,
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// A letter, digit or punctuation key, lowercase for letters.
    Char(char),
    /// F1 to F24.
    F(u8),
}

/// Modifier keys held during an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)] // One flag per modifier key.
pub struct Modifiers {
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt / Option.
    pub alt: bool,
    /// Windows key / Command.
    pub logo: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
        logo: false,
    };
    /// Control alone.
    pub const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    /// Shift alone.
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    /// Control and Shift: app shortcuts on Windows and Linux (UI-SPEC §7).
    pub const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };
}

/// One input event, in DIPs.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The pointer moved to a point.
    PointerMoved(Point),
    /// The pointer left the window.
    PointerLeft,
    /// A button went down.
    PointerDown(Button),
    /// A button came up.
    PointerUp(Button),
    /// Wheel or touchpad scroll, in DIPs; positive `y` scrolls content up (shows what is
    /// below).
    Wheel {
        /// Horizontal.
        x: f32,
        /// Vertical.
        y: f32,
    },
    /// A key went down (or repeated).
    Key(Key, Modifiers),
    /// Text typed, after the keyboard layout and IME.
    Text(String),
    /// Text pasted from the clipboard.
    Paste(String),
    /// The modifier keys held changed (so clicks know about Ctrl, Shift).
    Modifiers(Modifiers),
    /// Files dropped on the window from the OS (Explorer, Finder, a file manager), at the
    /// pointer's place.
    FilesDropped(Vec<std::path::PathBuf>),
}

/// A key press or typed text, in arrival order.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    /// A key with its modifiers.
    Key(Key, Modifiers),
    /// Text after the keyboard layout and IME.
    Text(String),
    /// Text pasted from the clipboard.
    Paste(String),
}

/// The mouse cursor a frame asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    /// The arrow.
    #[default]
    Default,
    /// A hand over links and clickable rows.
    Pointer,
    /// The I-beam over text.
    Text,
    /// Resizing left-right.
    ResizeHorizontal,
    /// Resizing up-down.
    ResizeVertical,
}

/// The colors views paint with, derived from the design tokens (UI-SPEC §8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Dark or light.
    pub dark: bool,
    /// Title bar and navigation.
    pub chrome: Color,
    /// Behind the content.
    pub bg: Color,
    /// Cards, list panels and fields at rest.
    pub surface: Color,
    /// Menus, dialogs and the editor panel.
    pub raised: Color,
    /// Text fields.
    pub field: Color,
    /// Hovered controls and rows.
    pub hover: Color,
    /// Pressed controls.
    pub press: Color,
    /// Selected rows and tabs: a faint accent.
    pub selected: Color,
    /// Hairlines.
    pub line: Color,
    /// Control outlines and stronger dividers.
    pub line_strong: Color,
    /// Main text.
    pub text: Color,
    /// Secondary text.
    pub text2: Color,
    /// Placeholders and disabled text.
    pub text3: Color,
    /// The accent: primary buttons, focus, the active tab.
    pub accent: Color,
    /// A hovered primary button.
    pub accent_hover: Color,
    /// Text and icons on the accent.
    pub on_accent: Color,
    /// Warnings.
    pub warn: Color,
    /// Errors.
    pub error: Color,
    /// Success, "connected".
    pub ok: Color,
    /// Drop shadows.
    pub shadow: Color,
    /// Dims the window behind a dialog.
    pub scrim: Color,
}

impl Theme {
    /// The theme for `appearance`.
    #[must_use]
    pub fn new(appearance: Appearance) -> Self {
        let p = Palette::for_appearance(appearance);
        let dark = appearance == Appearance::Dark;
        let surface = Color::from(p.bg_surface);
        let text2 = Color::from(p.text_secondary);
        let accent = Color::from(p.accent);
        let white = Color::WHITE;
        let black = Color::BLACK;
        Self {
            dark,
            chrome: p.bg_app.into(),
            bg: if dark {
                surface
            } else {
                Color::from(p.bg_app).mix(white, 0.55)
            },
            surface: if dark { p.bg_raised.into() } else { white },
            raised: if dark {
                Color::from(p.bg_raised).mix(white, 0.03)
            } else {
                white
            },
            field: if dark {
                Color::from(p.bg_app).mix(surface, 0.5)
            } else {
                white
            },
            hover: p.bg_hover.into(),
            press: Color::from(p.bg_hover).mix(if dark { white } else { black }, 0.06),
            selected: p.bg_selected.into(),
            line: p.border_subtle.into(),
            line_strong: Color::from(p.border_subtle).mix(text2, 0.25),
            text: p.text_primary.into(),
            text2,
            text3: text2.mix(surface, 0.35),
            accent,
            accent_hover: accent.mix(if dark { white } else { black }, 0.12),
            on_accent: if dark { Color::from(p.bg_app) } else { white },
            warn: p.status_warn.into(),
            error: p.status_error.into(),
            ok: p.status_ok.into(),
            shadow: black.alpha(if dark { 0.5 } else { 0.12 }),
            scrim: black.alpha(if dark { 0.55 } else { 0.3 }),
        }
    }
}

/// What one zone did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // Independent facts about one zone.
pub struct Response {
    /// The zone's rectangle.
    pub rect: Rect,
    /// The pointer is over it (and over nothing drawn above it).
    pub hovered: bool,
    /// A button went down on it and is still held.
    pub pressed: bool,
    /// Primary click, or Enter/Space while focused.
    pub clicked: bool,
    /// The second click of a double click.
    pub double_clicked: bool,
    /// Right click.
    pub secondary_clicked: bool,
    /// Middle click.
    pub middle_clicked: bool,
    /// It has keyboard focus.
    pub focused: bool,
    /// It has focus and the focus ring should show (focus came from the keyboard).
    pub focus_ring: bool,
}

/// An animated value moving toward a target.
#[derive(Clone, Copy, Debug)]
struct Anim {
    value: f32,
    target: f32,
    time: f64,
}

/// How long a secret copied with [`Ui::copy_secret`] stays on the clipboard.
pub const SECRET_ON_CLIPBOARD: std::time::Duration = std::time::Duration::from_secs(30);

/// What a finished frame asks of the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameOutput {
    /// The cursor to show.
    pub cursor: Cursor,
    /// Draw another frame right away (an animation is running, or hover changed).
    pub repaint: bool,
    /// The zones drawn, top-most last: hit-testing, the accessibility tree, tests.
    pub zones: Vec<Zone>,
    /// Draw another frame after this long even with no input (a sync round, auto-lock).
    pub repaint_after: Option<std::time::Duration>,
    /// Layout faults the widgets noticed (a button label cut off): the scene tests fail on
    /// any.
    pub faults: Vec<String>,
}

/// Toolkit state that lives across frames.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // Independent per-frame flags.
pub struct UiState {
    /// The zones of the frame on screen, top-most last.
    zones: Vec<Zone>,
    next_zones: Vec<Zone>,
    pointer: Option<Point>,
    modifiers: Modifiers,
    hovered: Option<Id>,
    pressed: Option<(Id, Button)>,
    /// Where the pointer was when the button went down: moving far from it makes a drag.
    pressed_at: Option<Point>,
    /// A drag that ended this frame: the zone it started on and where it was let go.
    drag_end: Option<(Id, Point)>,
    /// Files dropped from the OS this frame.
    dropped: Vec<std::path::PathBuf>,
    clicks: Vec<(Id, Button)>,
    double_clicks: Vec<Id>,
    last_click: Option<(Id, f64)>,
    focus: Option<Id>,
    focus_ring: bool,
    activated: Option<Id>,
    typed: Vec<Typed>,
    input_seen: bool,
    wheel: (f32, f32),
    time: f64,
    anims: HashMap<Id, Anim>,
    scroll: HashMap<Id, f32>,
    repaint: bool,
    repaint_after: Option<std::time::Duration>,
    cursor: Cursor,
    /// When the zone under the pointer became the one under it.
    hovered_since: f64,
    /// The tooltip to draw over this frame: its text and the control it explains.
    pub(crate) tooltip: Option<(String, Rect)>,
    edits: HashMap<Id, EditState>,
    pub(crate) popup: Option<Popup>,
    /// Text to put on the clipboard, and whether it is a secret.
    pub(crate) clipboard_out: Option<(String, bool)>,
    faults: Vec<String>,
    /// What assistive technology asked for since the last frame.
    requests: Vec<crate::access::Request>,
    /// No animations (UI-SPEC §8, "reduce motion").
    pub reduce_motion: bool,
}

/// Two clicks on the same zone closer than this are a double click, in seconds.
const DOUBLE_CLICK: f64 = 0.4;

/// How far the pointer moves with the button down before a press becomes a drag, in DIPs.
const DRAG_START: f32 = 6.0;

impl UiState {
    /// Resolves this frame's input against the zones on screen. `time` is in seconds.
    pub fn begin_frame(&mut self, events: &[Event], time: f64) {
        self.time = time;
        self.clicks.clear();
        self.double_clicks.clear();
        self.drag_end = None;
        self.dropped.clear();
        self.activated = None;
        for request in std::mem::take(&mut self.requests) {
            match request {
                crate::access::Request::Focus(id) => {
                    self.focus = Some(id);
                    self.focus_ring = true;
                }
                crate::access::Request::Click(id) => {
                    self.focus = Some(id);
                    self.activated = Some(id);
                }
            }
        }
        self.typed.clear();
        self.input_seen = events
            .iter()
            .any(|event| !matches!(event, Event::Modifiers(_)));
        self.faults.clear();
        self.wheel = (0.0, 0.0);
        // A click, key or text may change state after part of the frame was drawn (a card
        // opened above the list that opened it): one more frame shows it at once.
        self.repaint = events.iter().any(|event| {
            !matches!(
                event,
                Event::Modifiers(_) | Event::PointerMoved(_) | Event::PointerLeft
            )
        });
        self.repaint_after = None;
        self.cursor = Cursor::Default;
        self.tooltip = None;
        for event in events {
            match event {
                Event::PointerMoved(point) => self.move_pointer(*point, time),
                Event::PointerLeft => {
                    self.pointer = None;
                    self.hovered = None;
                }
                Event::PointerDown(button) => {
                    self.pressed = self.hovered.map(|id| (id, *button));
                    self.pressed_at = self.pointer;
                    if *button == Button::Primary {
                        let target = self.hovered.and_then(|id| self.zone(id));
                        self.focus = target.filter(|z| z.role.focusable()).map(|z| z.id);
                        self.focus_ring = false;
                    }
                }
                Event::PointerUp(button) => self.release(*button, time),
                Event::Wheel { x, y } => {
                    self.wheel.0 += x;
                    self.wheel.1 += y;
                }
                // An open menu takes Escape before anything under it (a panel that would
                // close with its draft).
                Event::Key(Key::Escape, modifiers)
                    if *modifiers == Modifiers::NONE && self.popup.is_some() =>
                {
                    self.popup = None;
                }
                Event::Key(Key::Tab, modifiers)
                    if !modifiers.ctrl
                        && !modifiers.alt
                        && self.focused_role() != Some(Role::Terminal) =>
                {
                    self.move_focus(!modifiers.shift);
                }
                Event::Key(key @ (Key::Enter | Key::Space), modifiers)
                    if *modifiers == Modifiers::NONE
                        && self
                            .focus
                            .and_then(|id| self.zone(id))
                            .is_some_and(|z| activates(z.role, *key)) =>
                {
                    self.activated = self.focus;
                }
                Event::Key(key, modifiers) => self.typed.push(Typed::Key(*key, *modifiers)),
                Event::Text(text) => self.typed.push(Typed::Text(text.clone())),
                Event::Paste(text) => self.typed.push(Typed::Paste(text.clone())),
                Event::Modifiers(modifiers) => self.modifiers = *modifiers,
                Event::FilesDropped(paths) => self.dropped.extend(paths.iter().cloned()),
            }
        }
    }

    /// A button came up: a click on the zone it went down on, unless the pointer was dragged
    /// away (then the drag ends there), and a double click when it is the second one.
    fn release(&mut self, button: Button, time: f64) {
        let dragged = self.dragged();
        let Some((id, pressed)) = self.pressed.take() else {
            return;
        };
        if pressed != button {
            return;
        }
        if let Some(at) = dragged {
            self.drag_end = Some((id, at));
            return;
        }
        if self.hovered != Some(id) {
            return;
        }
        self.clicks.push((id, button));
        if button == Button::Primary {
            if self
                .last_click
                .is_some_and(|(last, at)| last == id && time - at < DOUBLE_CLICK)
            {
                self.double_clicks.push(id);
                self.last_click = None;
            } else {
                self.last_click = Some((id, time));
            }
        }
    }

    /// Where the pointer is, when the primary button is down and has moved past
    /// [`DRAG_START`] since it went down.
    fn dragged(&self) -> Option<Point> {
        let (_, button) = self.pressed?;
        let (from, to) = (self.pressed_at?, self.pointer?);
        let far = (to.x - from.x).hypot(to.y - from.y) > DRAG_START;
        (button == Button::Primary && far).then_some(to)
    }

    /// The pointer moved to `point` at `time`: what it is over, and since when.
    fn move_pointer(&mut self, point: Point, time: f64) {
        self.pointer = Some(point);
        let hovered = self.hit(point);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.hovered_since = time;
        }
    }

    /// Queues what assistive technology asked for (a screen reader's click or focus move);
    /// the next frame does it, as the same input from the keyboard would.
    pub fn request(&mut self, request: crate::access::Request) {
        self.requests.push(request);
    }

    /// Ends the frame: the zones drawn become the ones input is resolved against.
    pub fn end_frame(&mut self) -> FrameOutput {
        self.zones = std::mem::take(&mut self.next_zones);
        // Layout may have moved under a still pointer: hover follows without an event.
        let hovered = self.pointer.and_then(|point| self.hit(point));
        if hovered != self.hovered {
            self.hovered = hovered;
            self.hovered_since = self.time;
            self.repaint = true;
        }
        if let Some(id) = self.focus
            && self.zone(id).is_none()
        {
            self.focus = None;
        }
        // A menu whose owner is gone (its panel closed) goes with it.
        if let Some(owner) = self.popup.as_ref().map(|popup| popup.owner)
            && self.zone(owner).is_none()
        {
            self.popup = None;
        }
        FrameOutput {
            cursor: self.cursor,
            repaint: self.repaint,
            repaint_after: self.repaint_after,
            zones: self.zones.clone(),
            faults: std::mem::take(&mut self.faults),
        }
    }

    /// The zones of the frame on screen.
    #[must_use]
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// The focused zone.
    #[must_use]
    pub fn focus(&self) -> Option<Id> {
        self.focus
    }

    /// The role of the focused zone, so a window can route keys (Ctrl+V pastes into a text
    /// field but belongs to the program in a terminal).
    #[must_use]
    pub fn focused_role(&self) -> Option<Role> {
        self.focus
            .and_then(|id| self.zone(id))
            .map(|zone| zone.role)
    }

    /// Gives `id` keyboard focus, with the focus ring when `ring`.
    pub fn set_focus(&mut self, id: Option<Id>, ring: bool) {
        self.focus = id;
        self.focus_ring = ring && id.is_some();
    }

    /// Text a field asked to copy (Ctrl+C / Ctrl+X); the window puts it on the clipboard.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.take_copied().map(|(text, _)| text)
    }

    /// Like [`Self::take_clipboard`], with whether the text is a secret (see
    /// [`Ui::copy_secret`]).
    pub fn take_copied(&mut self) -> Option<(String, bool)> {
        self.clipboard_out.take()
    }

    /// Whether `id` was clicked this frame, without drawing it (menus drawn on top report to
    /// the widget that opened them on the next frame).
    #[must_use]
    pub fn was_clicked(&self, id: Id) -> bool {
        self.clicks.contains(&(id, Button::Primary)) || self.activated == Some(id)
    }

    /// The editor state of text field `id`.
    pub(crate) fn edit(&self, id: Id) -> EditState {
        self.edits.get(&id).copied().unwrap_or_default()
    }

    pub(crate) fn set_edit(&mut self, id: Id, state: EditState) {
        self.edits.insert(id, state);
    }

    /// Puts back keys a widget took but did not use, for widgets and shortcuts after it.
    pub(crate) fn return_keys(&mut self, keys: Vec<(Key, Modifiers)>) {
        self.typed.extend(
            keys.into_iter()
                .map(|(key, modifiers)| Typed::Key(key, modifiers)),
        );
    }

    fn zone(&self, id: Id) -> Option<&Zone> {
        self.zones.iter().find(|zone| zone.id == id)
    }

    fn hit(&self, point: Point) -> Option<Id> {
        self.zones
            .iter()
            .rev()
            .find(|zone| zone.rect.contains(point))
            .map(|zone| zone.id)
    }

    fn move_focus(&mut self, forward: bool) {
        let order: Vec<Id> = self
            .zones
            .iter()
            .filter(|zone| zone.role.focusable())
            .map(|zone| zone.id)
            .collect();
        if order.is_empty() {
            return;
        }
        let current = self
            .focus
            .and_then(|id| order.iter().position(|known| *known == id));
        let next = match (current, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(at), true) => (at + 1) % order.len(),
            (Some(at), false) => (at + order.len() - 1) % order.len(),
        };
        self.focus = Some(order[next]);
        self.focus_ring = true;
    }
}

/// Whether `key` activates a focused zone of `role`: Enter and Space press buttons, Space
/// toggles check boxes, and text fields keep both keys.
fn activates(role: Role, key: Key) -> bool {
    match role {
        Role::Button | Role::Tab | Role::ListItem | Role::MenuItem | Role::ComboBox => true,
        Role::CheckBox => key == Key::Space,
        Role::TextInput
        | Role::ScrollView
        | Role::Splitter
        | Role::Backdrop
        | Role::WindowButton
        | Role::Terminal => false,
    }
}

/// One frame being drawn: the canvas, the theme and the toolkit state.
pub struct Ui<'a> {
    /// Where everything is drawn.
    pub gfx: &'a mut dyn Canvas,
    /// The colors.
    pub theme: Theme,
    /// State across frames.
    pub state: &'a mut UiState,
    clips: Vec<Rect>,
    /// The window's size in DIPs.
    pub viewport: Rect,
}

impl<'a> Ui<'a> {
    /// A frame over `viewport` (DIPs). Call [`UiState::begin_frame`] before and
    /// [`UiState::end_frame`] after.
    pub fn new(
        gfx: &'a mut dyn Canvas,
        theme: Theme,
        state: &'a mut UiState,
        viewport: Rect,
    ) -> Self {
        Self {
            gfx,
            theme,
            state,
            clips: Vec::new(),
            viewport,
        }
    }

    /// Registers an interactive zone and reports what input did to it. A zone clipped out of
    /// sight takes no input.
    pub fn zone(&mut self, id: Id, rect: Rect, role: Role, label: &str) -> Response {
        let visible = self.clips.last().map_or(rect, |clip| rect.intersect(clip));
        if visible.w <= 0.0 || visible.h <= 0.0 {
            return Response {
                rect,
                ..Response::default()
            };
        }
        self.state
            .next_zones
            .push(Zone::new(id, visible, role, label));
        let state = &*self.state;
        let clicked = state.clicks.contains(&(id, Button::Primary)) || state.activated == Some(id);
        let focused = state.focus == Some(id);
        Response {
            rect,
            hovered: state.hovered == Some(id),
            pressed: state.pressed.is_some_and(|(pressed, _)| pressed == id)
                && state.hovered == Some(id),
            clicked,
            double_clicked: state.double_clicks.contains(&id),
            secondary_clicked: state.clicks.contains(&(id, Button::Secondary)),
            middle_clicked: state.clicks.contains(&(id, Button::Middle)),
            focused,
            focus_ring: focused && state.focus_ring,
        }
    }

    /// Zone `id` of this frame, to tell assistive technology more about it.
    fn drawn(&mut self, id: Id) -> Option<&mut Zone> {
        self.state
            .next_zones
            .iter_mut()
            .rev()
            .find(|zone| zone.id == id)
    }

    /// Shows `text` by zone `id` at `rect` once the pointer has rested on it for a moment, or
    /// at once while the keyboard's focus ring is on it (an icon-only control says what it
    /// does). Drawn over everything by [`Ui::overlays`].
    pub fn tooltip(&mut self, id: Id, rect: Rect, text: &str) {
        const DELAY: f64 = 0.6;
        let keyboard = self.state.focus == Some(id) && self.state.focus_ring;
        let pointer = self.state.hovered == Some(id) && self.state.pressed.is_none();
        if !keyboard && !pointer {
            return;
        }
        let rested = self.state.time - self.state.hovered_since;
        if keyboard || rested >= DELAY {
            self.state.tooltip = Some((text.to_owned(), rect));
        } else {
            self.request_repaint_after(std::time::Duration::from_secs_f64(DELAY - rested));
        }
    }

    /// Gives zone `id` the accessible name `name` (an icon-only button: "More actions"
    /// rather than its icon's name).
    pub fn set_name(&mut self, id: Id, name: &str) {
        if let Some(zone) = self.drawn(id) {
            name.clone_into(&mut zone.label);
        }
    }

    /// Tells assistive technology that zone `id` (a check box or switch) is on or off.
    pub fn set_toggled(&mut self, id: Id, on: bool) {
        if let Some(zone) = self.drawn(id) {
            zone.toggled = Some(on);
        }
    }

    /// Tells assistive technology zone `id`'s text or current choice.
    pub fn set_value(&mut self, id: Id, value: &str) {
        if let Some(zone) = self.drawn(id) {
            value.clone_into(zone.value.get_or_insert_default());
        }
    }

    /// Marks zone `id` as a secret field: assistive technology never gets its text.
    pub fn set_secret(&mut self, id: Id) {
        if let Some(zone) = self.drawn(id) {
            zone.secret = true;
            zone.value = None;
        }
    }

    /// Clips drawing and zones to `rect` (inside any clip already active).
    pub fn push_clip(&mut self, rect: Rect) {
        let rect = self.clips.last().map_or(rect, |clip| rect.intersect(clip));
        self.clips.push(rect);
        self.gfx.push_clip(rect);
    }

    /// Ends the innermost clip.
    pub fn pop_clip(&mut self) {
        if self.clips.pop().is_some() {
            self.gfx.pop_clip();
        }
    }

    /// Consumes a key press with exactly `modifiers`, if one arrived this frame.
    pub fn key(&mut self, key: Key, modifiers: Modifiers) -> bool {
        let at = self
            .state
            .typed
            .iter()
            .position(|typed| *typed == Typed::Key(key, modifiers));
        at.map(|at| self.state.typed.remove(at)).is_some()
    }

    /// Takes every key press and text of this frame that is still unconsumed, in the order
    /// they arrived.
    pub fn take_typed(&mut self) -> Vec<Typed> {
        std::mem::take(&mut self.state.typed)
    }

    /// Puts back what [`Self::take_typed`] took, ahead of anything left since: a layer drawn
    /// last (a palette over the screen) gets the keyboard before what is under it.
    pub fn give_back_typed(&mut self, typed: Vec<Typed>) {
        let left = std::mem::replace(&mut self.state.typed, typed);
        self.state.typed.extend(left);
    }

    /// Takes the vertical wheel movement of this frame if the pointer is over `rect`.
    pub fn take_wheel(&mut self, rect: Rect) -> f32 {
        if self.state.pointer.is_some_and(|point| rect.contains(point)) {
            std::mem::take(&mut self.state.wheel.1)
        } else {
            0.0
        }
    }

    /// Whether any input arrived for this frame (the idle timer of auto-lock).
    #[must_use]
    pub fn had_input(&self) -> bool {
        self.state.input_seen
    }

    /// Seconds on the window's clock, for this frame.
    #[must_use]
    pub fn time(&self) -> f64 {
        self.state.time
    }

    /// The modifier keys held now.
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        self.state.modifiers
    }

    /// Whether the primary button is held down on `id` (a drag in progress).
    #[must_use]
    pub fn dragging(&self, id: Id) -> bool {
        self.state
            .pressed
            .is_some_and(|(pressed, button)| pressed == id && button == Button::Primary)
    }

    /// Where the pointer is while `id` is dragged: its press has moved far enough to be a
    /// drag rather than a click.
    #[must_use]
    pub fn drag_moved(&self, id: Id) -> Option<Point> {
        self.dragging(id).then(|| self.state.dragged()).flatten()
    }

    /// Where a drag that started on `id` was let go, in the frame it ended.
    #[must_use]
    pub fn drag_released(&self, id: Id) -> Option<Point> {
        self.state
            .drag_end
            .and_then(|(from, at)| (from == id).then_some(at))
    }

    /// Takes the files dropped on the window from the OS this frame.
    pub fn take_dropped_files(&mut self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut self.state.dropped)
    }

    /// Whether the pointer is on `rect` or on a zone inside it: a card stays hovered while
    /// the pointer is on one of its buttons, which are zones of their own.
    #[must_use]
    pub fn hover_within(&self, rect: Rect) -> bool {
        self.state
            .hovered
            .and_then(|id| self.state.zone(id))
            .is_some_and(|zone| {
                let inner = zone.rect;
                inner.x >= rect.x - 0.5
                    && inner.y >= rect.y - 0.5
                    && inner.r() <= rect.r() + 0.5
                    && inner.b() <= rect.b() + 0.5
            })
    }

    /// The pointer, if it is over the window.
    #[must_use]
    pub fn pointer(&self) -> Option<Point> {
        self.state.pointer
    }

    /// Puts `text` on the clipboard when the frame ends.
    pub fn copy(&mut self, text: String) {
        self.state.clipboard_out = Some((text, false));
    }

    /// Puts the secret `text` (a recovery key) on the clipboard when the frame ends: kept out
    /// of clipboard history and the cloud clipboard where the OS allows, and cleared after
    /// [`SECRET_ON_CLIPBOARD`] unless something else was copied since (SECURITY.md rule 2).
    pub fn copy_secret(&mut self, text: String) {
        self.state.clipboard_out = Some((text, true));
    }

    /// Asks for the cursor shown this frame.
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.state.cursor = cursor;
    }

    /// Records a layout fault: something did not fit where it must.
    pub fn fault(&mut self, fault: String) {
        self.state.faults.push(fault);
    }

    /// Draws `text` like [`Canvas::text`], but records a fault when it does not fit: for
    /// labels that must never be cut (buttons, headings, field labels).
    pub fn strict_text(
        &mut self,
        rect: Rect,
        text: &str,
        font: crate::gfx::Font,
        color: Color,
        align: crate::gfx::Align,
    ) {
        if self.gfx.text(rect, text, font, color, align) {
            self.fault(format!("cut off: {text:?} in {:.0}x{:.0}", rect.w, rect.h));
        }
    }

    /// Asks for a frame after `after`, with no input; the soonest request of a frame wins.
    pub fn request_repaint_after(&mut self, after: std::time::Duration) {
        self.state.repaint_after = Some(
            self.state
                .repaint_after
                .map_or(after, |earlier| earlier.min(after)),
        );
    }

    /// Asks for another frame right away.
    pub fn request_repaint(&mut self) {
        self.state.repaint = true;
    }

    /// The scroll offset stored for `id`.
    #[must_use]
    pub fn scroll_offset(&self, id: Id) -> f32 {
        self.state.scroll.get(&id).copied().unwrap_or(0.0)
    }

    /// Stores the scroll offset for `id`.
    pub fn set_scroll_offset(&mut self, id: Id, offset: f32) {
        self.state.scroll.insert(id, offset);
    }

    /// A value easing toward `target` over the motion duration (UI-SPEC §8), for hover fades
    /// and panel slides; it jumps straight there under reduced motion.
    pub fn ease(&mut self, id: Id, target: f32) -> f32 {
        let now = self.state.time;
        let reduce = self.state.reduce_motion;
        let anim = self.state.anims.entry(id).or_insert(Anim {
            value: target,
            target,
            time: now,
        });
        if reduce {
            anim.value = target;
        } else {
            #[allow(clippy::cast_possible_truncation)] // Frame gaps are small.
            let elapsed = (now - anim.time) as f32;
            let step = elapsed * 1000.0 / f32::from(u16::try_from(MOTION_MS).unwrap_or(120));
            let distance = target - anim.value;
            if distance.abs() <= step.max(0.0) {
                anim.value = target;
            } else {
                anim.value += distance.signum() * step;
            }
        }
        anim.target = target;
        anim.time = now;
        let value = anim.value;
        if (value - target).abs() > f32::EPSILON {
            self.state.repaint = true;
        }
        // Ease out: fast at first, settling softly.
        let t = value.clamp(0.0, 1.0);
        if (0.0..=1.0).contains(&target) {
            1.0 - (1.0 - t) * (1.0 - t)
        } else {
            value
        }
    }

    /// Draws the focus ring around `rect` (UI-SPEC §8: accent, 2 px, offset 2 px).
    pub fn focus_ring(&mut self, rect: Rect, radius: f32) {
        let accent = self.theme.accent;
        self.gfx
            .stroke_rect(rect.outset(3.0), radius + 3.0, 2.0, accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::Recorder;

    fn frame(
        state: &mut UiState,
        events: &[Event],
        time: f64,
        draw: impl FnOnce(&mut Ui<'_>),
    ) -> FrameOutput {
        let mut canvas = Recorder::default();
        state.begin_frame(events, time);
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            state,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        draw(&mut ui);
        state.end_frame()
    }

    const A: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 30.0,
    };
    const B: Rect = Rect {
        x: 0.0,
        y: 40.0,
        w: 100.0,
        h: 30.0,
    };

    fn two_buttons(ui: &mut Ui<'_>) -> (Response, Response) {
        (
            ui.zone(Id::new("a"), A, Role::Button, "A"),
            ui.zone(Id::new("b"), B, Role::Button, "B"),
        )
    }

    #[test]
    fn a_press_that_moves_far_is_a_drag_not_a_click() {
        let mut state = UiState::default();
        let row = Id::new("row");
        let mut seen = (false, None, None);
        let mut run = |state: &mut UiState, events: &[Event], time: f64| {
            frame(state, events, time, |ui| {
                let response = ui.zone(row, A, Role::Button, "row");
                seen = (response.clicked, ui.drag_moved(row), ui.drag_released(row));
            });
            seen
        };
        run(&mut state, &[], 0.0);
        // A little wobble is still a click.
        let wobble = [
            Event::PointerMoved(Point::new(10.0, 10.0)),
            Event::PointerDown(Button::Primary),
            Event::PointerMoved(Point::new(13.0, 11.0)),
            Event::PointerUp(Button::Primary),
        ];
        assert_eq!(run(&mut state, &wobble, 1.0), (true, None, None));
        // Farther, it is a drag: reported while it moves and where it is let go.
        let down = [
            Event::PointerMoved(Point::new(10.0, 10.0)),
            Event::PointerDown(Button::Primary),
            Event::PointerMoved(Point::new(200.0, 150.0)),
        ];
        let (clicked, moved, _) = run(&mut state, &down, 2.0);
        assert!(!clicked);
        assert_eq!(moved, Some(Point::new(200.0, 150.0)));
        let up = [Event::PointerUp(Button::Primary)];
        assert_eq!(
            run(&mut state, &up, 3.0),
            (false, None, Some(Point::new(200.0, 150.0)))
        );
    }

    #[test]
    fn files_dropped_from_the_os_are_taken_once() {
        let mut state = UiState::default();
        let file = std::path::PathBuf::from("report.pdf");
        let mut taken = (Vec::new(), Vec::new());
        frame(
            &mut state,
            &[Event::FilesDropped(vec![file.clone()])],
            0.0,
            |ui| taken = (ui.take_dropped_files(), ui.take_dropped_files()),
        );
        assert_eq!(taken.0, [file]);
        assert_eq!(taken.1, Vec::<std::path::PathBuf>::new());
    }

    #[test]
    fn a_click_needs_down_and_up_on_the_same_zone() {
        let mut state = UiState::default();
        frame(&mut state, &[], 0.0, |ui| {
            two_buttons(ui);
        });
        let at_a = Event::PointerMoved(Point::new(10.0, 10.0));
        let at_b = Event::PointerMoved(Point::new(10.0, 50.0));
        let down = Event::PointerDown(Button::Primary);
        let up = Event::PointerUp(Button::Primary);
        let mut clicked = (false, false);
        frame(
            &mut state,
            &[at_a.clone(), down.clone(), up.clone()],
            0.1,
            |ui| {
                let (a, b) = two_buttons(ui);
                clicked = (a.clicked, b.clicked);
            },
        );
        assert_eq!(clicked, (true, false));
        // Pressed on A, released on B: nothing is clicked.
        frame(&mut state, &[at_a, down, at_b, up], 1.0, |ui| {
            let (a, b) = two_buttons(ui);
            clicked = (a.clicked, b.clicked);
        });
        assert_eq!(clicked, (false, false));
    }

    #[test]
    fn the_zone_drawn_last_wins_the_pointer() {
        let mut state = UiState::default();
        let backdrop = Rect::new(0.0, 0.0, 400.0, 300.0);
        let draw = |ui: &mut Ui<'_>| {
            let under = ui.zone(Id::new("under"), backdrop, Role::Backdrop, "");
            let over = ui.zone(Id::new("over"), A, Role::Button, "Over");
            (under.hovered, over.hovered)
        };
        frame(&mut state, &[], 0.0, |ui| {
            draw(ui);
        });
        let mut hovered = (false, false);
        frame(
            &mut state,
            &[Event::PointerMoved(Point::new(5.0, 5.0))],
            0.1,
            |ui| hovered = draw(ui),
        );
        assert_eq!(hovered, (false, true));
    }

    #[test]
    fn tab_cycles_focus_in_drawing_order_and_enter_activates() {
        let mut state = UiState::default();
        frame(&mut state, &[], 0.0, |ui| {
            two_buttons(ui);
        });
        let tab = Event::Key(Key::Tab, Modifiers::NONE);
        let mut seen = (Response::default(), Response::default());
        frame(&mut state, &[tab.clone(), tab.clone()], 0.1, |ui| {
            seen = two_buttons(ui);
        });
        assert!(seen.1.focused && seen.1.focus_ring && !seen.0.focused);
        frame(
            &mut state,
            &[Event::Key(Key::Tab, Modifiers::SHIFT)],
            0.2,
            |ui| seen = two_buttons(ui),
        );
        assert!(seen.0.focused);
        frame(
            &mut state,
            &[Event::Key(Key::Enter, Modifiers::NONE)],
            0.3,
            |ui| seen = two_buttons(ui),
        );
        assert!(seen.0.clicked && !seen.1.clicked);
    }

    #[test]
    fn a_focused_terminal_keeps_tab_and_enter() {
        let mut state = UiState::default();
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let draw = |ui: &mut Ui<'_>| ui.zone(Id::new("term"), rect, Role::Terminal, "Terminal");
        frame(&mut state, &[], 0.0, |ui| {
            draw(ui);
        });
        state.set_focus(Some(Id::new("term")), false);
        let mut typed = Vec::new();
        frame(
            &mut state,
            &[
                Event::Key(Key::Tab, Modifiers::NONE),
                Event::Key(Key::Enter, Modifiers::NONE),
                Event::Paste("ls".into()),
            ],
            0.1,
            |ui| {
                draw(ui);
                typed = ui.take_typed();
            },
        );
        assert_eq!(state.focus(), Some(Id::new("term")));
        assert_eq!(
            typed,
            [
                Typed::Key(Key::Tab, Modifiers::NONE),
                Typed::Key(Key::Enter, Modifiers::NONE),
                Typed::Paste("ls".into())
            ]
        );
    }

    #[test]
    fn window_buttons_click_but_are_skipped_by_tab() {
        let mut state = UiState::default();
        let draw = |ui: &mut Ui<'_>| {
            let close = ui.zone(Id::new("close"), A, Role::WindowButton, "Close");
            let field = ui.zone(Id::new("field"), B, Role::TextInput, "Search");
            (close, field)
        };
        frame(&mut state, &[], 0.0, |ui| {
            draw(ui);
        });
        let mut seen = (Response::default(), Response::default());
        frame(
            &mut state,
            &[Event::Key(Key::Tab, Modifiers::NONE)],
            0.1,
            |ui| seen = draw(ui),
        );
        assert!(seen.1.focused && !seen.0.focused);
        let click = [
            Event::PointerMoved(Point::new(5.0, 5.0)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        frame(&mut state, &click, 0.2, |ui| seen = draw(ui));
        assert!(seen.0.clicked);
    }

    #[test]
    fn clicking_focuses_without_a_ring_and_a_vanished_zone_loses_focus() {
        let mut state = UiState::default();
        frame(&mut state, &[], 0.0, |ui| {
            two_buttons(ui);
        });
        let mut a = Response::default();
        frame(
            &mut state,
            &[
                Event::PointerMoved(Point::new(5.0, 5.0)),
                Event::PointerDown(Button::Primary),
            ],
            0.1,
            |ui| a = two_buttons(ui).0,
        );
        assert!(a.focused && !a.focus_ring);
        frame(&mut state, &[], 0.2, |_| {});
        assert_eq!(state.focus(), None);
    }

    #[test]
    fn two_quick_clicks_are_a_double_click() {
        let mut state = UiState::default();
        let click = [
            Event::PointerMoved(Point::new(5.0, 5.0)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        let mut a = Response::default();
        frame(&mut state, &[], 0.0, |ui| {
            two_buttons(ui);
        });
        frame(&mut state, &click, 1.0, |ui| a = two_buttons(ui).0);
        assert!(a.clicked && !a.double_clicked);
        frame(&mut state, &click, 1.2, |ui| a = two_buttons(ui).0);
        assert!(a.double_clicked);
        // Slow clicks are two single clicks.
        frame(&mut state, &click, 3.0, |ui| a = two_buttons(ui).0);
        assert!(!a.double_clicked);
    }

    #[test]
    fn a_click_asks_for_one_more_frame_and_a_pointer_move_does_not() {
        let mut state = UiState::default();
        let far = [Event::PointerMoved(Point::new(300.0, 300.0))];
        assert!(
            !frame(&mut state, &far, 0.0, |ui| {
                let _ = two_buttons(ui);
            })
            .repaint
        );
        let click = [
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        assert!(
            frame(&mut state, &click, 1.0, |ui| {
                let _ = two_buttons(ui);
            })
            .repaint
        );
        assert!(
            !frame(&mut state, &[], 2.0, |ui| {
                let _ = two_buttons(ui);
            })
            .repaint
        );
    }

    #[test]
    fn a_card_stays_hovered_while_the_pointer_is_on_its_button() {
        const CARD: Rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 80.0,
        };
        const BUTTON: Rect = Rect {
            x: 150.0,
            y: 20.0,
            w: 30.0,
            h: 30.0,
        };
        fn draw(ui: &mut Ui<'_>) -> (bool, bool) {
            let card = ui.zone(Id::new("card"), CARD, Role::ListItem, "card");
            ui.zone(Id::new("button"), BUTTON, Role::Button, "button");
            (card.hovered, ui.hover_within(CARD))
        }
        let mut state = UiState::default();
        let mut within = (false, false);
        frame(&mut state, &[], 0.0, |ui| within = draw(ui));
        let on_button = [Event::PointerMoved(BUTTON.center())];
        frame(&mut state, &on_button, 1.0, |ui| within = draw(ui));
        assert_eq!(
            within,
            (false, true),
            "the button, not the card, is hovered"
        );
        let outside = [Event::PointerMoved(Point::new(300.0, 300.0))];
        frame(&mut state, &outside, 2.0, |ui| within = draw(ui));
        assert_eq!(within, (false, false));
    }

    #[test]
    fn a_screen_readers_click_and_focus_work_like_the_keyboards() {
        let mut state = UiState::default();
        let mut clicked = (false, false);
        frame(&mut state, &[], 0.0, |ui| {
            let _ = two_buttons(ui);
        });
        state.request(crate::access::Request::Click(Id::new("b")));
        frame(&mut state, &[], 1.0, |ui| {
            let (a, b) = two_buttons(ui);
            clicked = (a.clicked, b.clicked);
        });
        assert_eq!(clicked, (false, true));
        assert_eq!(state.focus(), Some(Id::new("b")));
        state.request(crate::access::Request::Focus(Id::new("a")));
        frame(&mut state, &[], 2.0, |ui| {
            let (a, b) = two_buttons(ui);
            clicked = (a.clicked, b.clicked);
        });
        assert_eq!(clicked, (false, false), "focus alone clicks nothing");
        assert_eq!(state.focus(), Some(Id::new("a")));
    }

    #[test]
    fn clipped_zones_take_no_input() {
        let mut state = UiState::default();
        let output = frame(&mut state, &[], 0.0, |ui| {
            ui.push_clip(Rect::new(0.0, 0.0, 100.0, 35.0));
            two_buttons(ui);
            ui.pop_clip();
        });
        assert_eq!(output.zones.len(), 1);
        assert_eq!(output.zones[0].label, "A");
    }

    #[test]
    fn easing_reaches_the_target_after_the_motion_time_and_asks_for_frames_until_then() {
        let mut state = UiState::default();
        let id = Id::new("fade");
        let mut value = 0.0;
        frame(&mut state, &[], 0.0, |ui| value = ui.ease(id, 0.0));
        let output = frame(&mut state, &[], 0.05, |ui| value = ui.ease(id, 1.0));
        assert!(value > 0.0 && value < 1.0 && output.repaint);
        let output = frame(&mut state, &[], 0.3, |ui| value = ui.ease(id, 1.0));
        assert!((value - 1.0).abs() < f32::EPSILON && !output.repaint);
        state.reduce_motion = true;
        frame(&mut state, &[], 0.31, |ui| value = ui.ease(id, 0.0));
        assert!(value.abs() < f32::EPSILON);
    }

    #[test]
    fn both_themes_keep_text_readable_on_every_surface() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::new(appearance);
            let rgb = |c: Color| annulus_config::tokens::Rgb::new(c.r, c.g, c.b);
            for surface in [
                theme.chrome,
                theme.bg,
                theme.surface,
                theme.raised,
                theme.field,
            ] {
                assert!(
                    rgb(theme.text).contrast(rgb(surface)) >= 4.5,
                    "{appearance:?}"
                );
                assert!(
                    rgb(theme.text2).contrast(rgb(surface)) >= 4.5,
                    "{appearance:?}"
                );
            }
            assert!(rgb(theme.on_accent).contrast(rgb(theme.accent)) >= 4.5);
        }
    }
}
