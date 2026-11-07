//! The window our toolkit runs in ([ADR-0017] decision 4): on Windows our own Win32 window
//! drawn by Direct2D (`crate::win32`), elsewhere `winit` + `softbuffer` drawn by the CPU
//! renderer (below).
//!
//! The app draws its own title bar and names the zones that drag the window and maximize it.
//! The OS draws none, except on macOS, whose title bar is transparent over ours and shows only
//! the window buttons, at the bar's left end ([`OS_WINDOW_BUTTONS`], `crate::macos`). Frames
//! are drawn only when input arrives or the app asks for one (P-06). Screen readers get the
//! tree of each frame's zones (`annulus_ui::access`) through AccessKit: UI Automation on
//! Windows, AT-SPI on Linux, `NSAccessibility` on macOS.
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![forbid(unsafe_code)]

use std::hash::{BuildHasher as _, RandomState};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use annulus_ui::kit::{self, Event, Id, Key, Modifiers, Role, Theme, Ui, UiState};
use zeroize::Zeroizing;

/// What the app asks of the window after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRequest {
    /// Minimize.
    Minimize,
    /// Maximize, or restore when maximized.
    ToggleMaximize,
    /// Close the window and end the app.
    Close,
}

/// An item the window adds to the app's menu in the system menu bar (macOS; elsewhere the app's
/// own screens offer them): the app hears of a choice through [`WindowApp::menu`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// About the app.
    About,
    /// The app's settings.
    Settings,
    /// Lock the vault.
    Lock,
}

impl MenuItem {
    /// Every item, in the menu's order.
    pub const ALL: [MenuItem; 3] = [MenuItem::About, MenuItem::Settings, MenuItem::Lock];
}

/// The OS draws the window buttons itself (macOS's traffic lights, at the title bar's left
/// end); elsewhere the app's title bar draws its own.
pub const OS_WINDOW_BUTTONS: bool = cfg!(target_os = "macos");

/// How far macOS's first window button sits from the window's left edge, in DIPs: as far as
/// it sits from the top in our title bar, where it is centered.
pub const TRAFFIC_LIGHTS_LEFT: f32 = 16.0;

/// The room macOS's window buttons take at the title bar's left end, with a gap after them.
pub const TRAFFIC_LIGHTS_W: f32 = 88.0;

/// Asks the window for a new frame; callable from any thread (a session with new output).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A [`Waker`] that sends `event()` to `proxy`'s event loop, which then asks for a frame. It
/// never waits for the main thread: a terminal's reader thread wakes the window while it holds
/// the terminal's lock, and winit's `request_redraw` from another thread waits for the main
/// thread on macOS, which may be waiting for that lock to draw.
#[cfg(not(windows))]
#[must_use]
pub fn event_loop_waker<T: Send + 'static>(
    proxy: winit::event_loop::EventLoopProxy<T>,
    event: impl Fn() -> T + Send + Sync + 'static,
) -> Waker {
    Arc::new(move || {
        let _ = proxy.send_event(event());
    })
}

/// Whether the user asked the OS for fewer animations. Read on Windows; elsewhere not yet,
/// so it says no.
#[must_use]
pub fn os_reduces_motion() -> bool {
    #[cfg(windows)]
    {
        crate::win32::animations_off()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// An app the window runs.
pub trait WindowApp {
    /// Hands the app the window's waker, once, before the first frame.
    fn set_waker(&mut self, waker: Waker) {
        let _ = waker;
    }

    /// Draws one frame. `maximized` tells the title bar which button to show.
    fn frame(&mut self, ui: &mut Ui<'_>, maximized: bool) -> Option<WindowRequest>;

    /// The zone whose empty space drags the window.
    fn drag_zone(&self) -> Id;

    /// The maximize button's zone (Windows shows Snap Layouts over it).
    fn maximize_zone(&self) -> Id;

    /// The colors to draw with.
    fn theme(&self) -> Theme;

    /// The user chose `item` in the menu bar (macOS); a frame follows at once.
    fn menu(&mut self, item: MenuItem) {
        let _ = item;
    }

    /// The keys the menu bar shows beside `item` and answers to before any frame sees them;
    /// `None` shows none and leaves every key to the frame (while Settings records new keys,
    /// say). Asked after each frame.
    fn menu_keys(&self, item: MenuItem) -> Option<(Key, Modifiers)> {
        let _ = item;
        None
    }
}

/// Why the window could not run.
#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    /// The OS refused a window, an event loop or a renderer.
    #[error("window: {0}")]
    Platform(String),
}

/// Opens `url` in the default browser. Only `http`, `https` and `mailto` links are passed on;
/// anything else (a `file:` or custom scheme in terminal output) is refused.
pub fn open_url(url: &str) {
    let allowed = ["http://", "https://", "mailto:"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if allowed {
        let _ = webbrowser::open(url);
    }
}

/// Reads the clipboard's text, if any.
pub(crate) fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Puts the text the frame copied (Ctrl+C in a field) on the clipboard, and clears a secret
/// copied earlier once its time is up; `output` then wakes the window for that.
pub(crate) fn publish_clipboard(state: &mut UiState, output: &mut kit::FrameOutput) {
    let now = Instant::now();
    if let Some((text, secret)) = state.take_copied() {
        let text = Zeroizing::new(text);
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let mut held = held_secret();
            if secret {
                if set_secret(&mut clipboard, &text).is_ok() {
                    *held = Some(HeldSecret::new(&text, now));
                }
            } else if clipboard.set_text(text.as_str()).is_ok() {
                // The secret is no longer what the clipboard holds.
                *held = None;
            }
        }
    }
    if let Some(left) = expire_clipboard_secret(now) {
        output.repaint_after = Some(output.repaint_after.map_or(left, |after| after.min(left)));
    }
}

/// A secret this app put on the clipboard. Only a hash of it is kept, to tell whether the
/// clipboard still holds it.
struct HeldSecret {
    hasher: RandomState,
    hash: u64,
    until: Instant,
}

impl HeldSecret {
    fn new(text: &str, now: Instant) -> Self {
        let hasher = RandomState::new();
        Self {
            hash: hasher.hash_one(text),
            hasher,
            until: now + kit::SECRET_ON_CLIPBOARD,
        }
    }

    /// Whether a clipboard holding `current` at `now` is cleared: the time is up and nothing
    /// else was copied over the secret.
    fn clears(&self, current: Option<&str>, now: Instant) -> bool {
        now >= self.until && current.is_some_and(|text| self.hasher.hash_one(text) == self.hash)
    }
}

static HELD: Mutex<Option<HeldSecret>> = Mutex::new(None);

fn held_secret() -> std::sync::MutexGuard<'static, Option<HeldSecret>> {
    HELD.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Clears the copied secret if its time is up at `now`; otherwise, how long it has left.
fn expire_clipboard_secret(now: Instant) -> Option<std::time::Duration> {
    let mut held = held_secret();
    let secret = held.as_ref()?;
    if now < secret.until {
        return Some(secret.until - now);
    }
    let current = arboard::Clipboard::new().and_then(|mut clipboard| {
        let text = clipboard.get_text().map(Zeroizing::new);
        if let Ok(text) = &text
            && secret.clears(Some(text.as_str()), now)
        {
            clipboard.clear()?;
        }
        text.map(|_| ())
    });
    // Another program holds the clipboard: try again shortly, for at most a minute.
    if matches!(current, Err(arboard::Error::ClipboardOccupied))
        && now < secret.until + std::time::Duration::from_secs(60)
    {
        return Some(std::time::Duration::from_secs(1));
    }
    *held = None;
    None
}

/// Copies `text` where clipboard managers and the cloud clipboard leave it out.
fn set_secret(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    let set = clipboard.set();
    #[cfg(windows)]
    let set = {
        use arboard::SetExtWindows as _;
        set.exclude_from_history().exclude_from_cloud()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let set = {
        use arboard::SetExtLinux as _;
        set.exclude_from_history()
    };
    #[cfg(target_os = "macos")]
    let set = {
        use arboard::SetExtApple as _;
        set.exclude_from_history()
    };
    set.text(text)
}

/// Whether a key press pastes: Ctrl+Shift+V and Shift+Insert anywhere, and the command key
/// with V ([`Modifiers::COMMAND`]): on macOS anywhere, since Cmd is free there; elsewhere
/// only in a text field (Ctrl+V in a terminal belongs to the program).
pub(crate) fn is_paste(
    focused: Option<Role>,
    key: Key,
    modifiers: Modifiers,
    insert: bool,
) -> bool {
    let v = key == Key::Char('v');
    let command_v = v
        && modifiers == Modifiers::COMMAND
        && (cfg!(target_os = "macos") || focused == Some(Role::TextInput));
    (insert && modifiers == Modifiers::SHIFT)
        || (v && modifiers == Modifiers::CTRL_SHIFT)
        || command_v
}

/// The paste event for the clipboard's text.
pub(crate) fn paste_event() -> Option<Event> {
    clipboard_text().map(Event::Paste)
}

/// Opens a window titled `title`, `size` DIPs large, and runs `app` until it closes: our
/// Win32 window on Windows, `winit` elsewhere.
///
/// # Errors
/// When the OS refuses a window.
pub fn run(
    title: &str,
    size: (f32, f32),
    app: impl WindowApp + 'static,
) -> Result<(), WindowError> {
    #[cfg(windows)]
    let result = crate::win32::run(title, size, Box::new(app));
    #[cfg(not(windows))]
    let result = portable::run(title, size, Box::new(app));
    // A secret copied just before the window closed does not outlive it.
    let closing = held_secret().as_ref().map(|secret| secret.until);
    if let Some(until) = closing {
        expire_clipboard_secret(until);
    }
    result
}

#[cfg(not(windows))]
mod portable {
    use std::num::NonZeroU32;
    use std::time::Instant;

    use annulus_ui::access::{self, Request};
    use annulus_ui::gfx::{Point, Rect};
    use annulus_ui::kit::{self, Button, Cursor, Event, Modifiers, Ui, UiState};
    use softbuffer::{Context, Surface};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::keyboard::{Key as WinitKey, NamedKey};
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement as _;
    use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

    use std::sync::Arc;

    use crate::cpu::{CpuCanvas, TextEngine};

    use super::{WindowApp, WindowError, WindowRequest, is_paste, paste_event, publish_clipboard};

    /// What other threads send the event loop.
    pub(super) enum UserEvent {
        /// A screen reader's news, sent by AccessKit's adapter.
        Access(accesskit_winit::Event),
        /// The app has something new to draw (a session's output).
        Wake,
    }

    impl From<accesskit_winit::Event> for UserEvent {
        fn from(event: accesskit_winit::Event) -> Self {
            Self::Access(event)
        }
    }

    /// Runs `app` in a `winit` window until it closes.
    pub(super) fn run(
        title: &str,
        size: (f32, f32),
        app: Box<dyn WindowApp>,
    ) -> Result<(), WindowError> {
        let event_loop = EventLoop::<UserEvent>::with_user_event()
            .build()
            .map_err(|e| WindowError::Platform(e.to_string()))?;
        let mut host = Host {
            proxy: event_loop.create_proxy(),
            title: title.to_owned(),
            size,
            app,
            window: None,
            state: UiState::default(),
            text: Some(TextEngine::new()),
            events: Vec::new(),
            modifiers: Modifiers::NONE,
            buttons: Buttons::default(),
            pointer: None,
            started: Instant::now(),
            wake_at: None,
        };
        event_loop
            .run_app(&mut host)
            .map_err(|e| WindowError::Platform(e.to_string()))
    }

    struct Shown {
        window: Arc<Window>,
        surface: Surface<Arc<Window>, Arc<Window>>,
        /// The screen-reader bridge; it builds a tree only once one asks.
        access: accesskit_winit::Adapter,
    }

    struct Host {
        proxy: EventLoopProxy<UserEvent>,
        title: String,
        size: (f32, f32),
        app: Box<dyn WindowApp>,
        window: Option<Shown>,
        state: UiState,
        text: Option<TextEngine>,
        events: Vec<Event>,
        modifiers: Modifiers,
        buttons: Buttons,
        pointer: Option<Point>,
        started: Instant,
        /// When the app asked for its next frame without input.
        wake_at: Option<Instant>,
    }

    /// Which button a mouse button's press or release is. On a Mac a click held with Ctrl is a
    /// right click (UI-SPEC §7), from its press to its release even when Ctrl is let go in
    /// between.
    #[derive(Debug, Default)]
    struct Buttons {
        /// The left button went down with Ctrl held, on a Mac.
        ctrl_click: bool,
    }

    impl Buttons {
        fn button(
            &mut self,
            button: MouseButton,
            state: ElementState,
            modifiers: Modifiers,
        ) -> Option<Button> {
            match button {
                MouseButton::Left => {}
                MouseButton::Right => return Some(Button::Secondary),
                MouseButton::Middle => return Some(Button::Middle),
                _ => return None,
            }
            if state == ElementState::Pressed {
                self.ctrl_click = cfg!(target_os = "macos") && modifiers.ctrl;
            }
            let secondary = self.ctrl_click;
            if state == ElementState::Released {
                self.ctrl_click = false;
            }
            Some(if secondary {
                Button::Secondary
            } else {
                Button::Primary
            })
        }
    }

    /// The band along the window's edges that resizes it, in DIPs.
    const EDGE: f32 = 5.0;

    /// The time between two frames of an animation: 60 per second.
    const ANIMATION_FRAME: std::time::Duration = std::time::Duration::from_micros(16_667);

    /// The resize direction of the window edge under `point`, if any. None on macOS, which
    /// resizes a titled window from its edges itself.
    fn edge_at(window: &Window, point: Point) -> Option<ResizeDirection> {
        if cfg!(target_os = "macos") || window.is_maximized() {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)] // Window sizes fit in f32.
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();
        #[allow(clippy::cast_precision_loss)] // Window sizes fit in f32.
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let (left, right) = (point.x < EDGE, point.x > w - EDGE);
        let (top, bottom) = (point.y < EDGE, point.y > h - EDGE);
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(ResizeDirection::NorthWest),
            (_, true, true, _) => Some(ResizeDirection::NorthEast),
            (true, _, _, true) => Some(ResizeDirection::SouthWest),
            (_, true, _, true) => Some(ResizeDirection::SouthEast),
            (true, ..) => Some(ResizeDirection::West),
            (_, true, ..) => Some(ResizeDirection::East),
            (_, _, true, _) => Some(ResizeDirection::North),
            (_, _, _, true) => Some(ResizeDirection::South),
            _ => None,
        }
    }

    impl Host {
        fn redraw(&mut self, event_loop: &ActiveEventLoop) {
            let Some(shown) = self.window.as_mut() else {
                return;
            };
            let size = shown.window.inner_size();
            let (Some(width), Some(height)) =
                (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
            else {
                return;
            };
            #[allow(clippy::cast_possible_truncation)] // Scale factors are small.
            let scale = shown.window.scale_factor() as f32;
            let Some(text) = self.text.take() else { return };
            let Ok(mut canvas) = CpuCanvas::new(width.get(), height.get(), scale, text) else {
                return;
            };
            let events = std::mem::take(&mut self.events);
            let began = Instant::now();
            let time = self.started.elapsed().as_secs_f64();
            self.state.begin_frame(&events, time);
            #[allow(clippy::cast_precision_loss)] // Window sizes fit in f32.
            let viewport = Rect::new(
                0.0,
                0.0,
                width.get() as f32 / scale,
                height.get() as f32 / scale,
            );
            #[cfg(target_os = "macos")]
            for item in crate::menu_bar::take_chosen() {
                self.app.menu(item);
            }
            let theme = self.app.theme();
            let maximized = shown.window.is_maximized();
            let request = {
                let mut ui = Ui::new(&mut canvas, theme, &mut self.state, viewport);
                self.app.frame(&mut ui, maximized)
            };
            let mut output = self.state.end_frame();
            publish_clipboard(&mut self.state, &mut output);
            #[cfg(target_os = "macos")]
            crate::menu_bar::set_menu_keys(|item| self.app.menu_keys(item));
            // The window buttons sit centered in the title bar, which is the drag zone.
            #[cfg(target_os = "macos")]
            if let Some(bar) = output
                .zones
                .iter()
                .find(|zone| zone.id == self.app.drag_zone())
            {
                crate::macos::place_buttons(&shown.window, f64::from(bar.rect.b()));
            }
            let focus = self.state.focus();
            let title = &self.title;
            shown
                .access
                .update_if_active(|| access::tree(&output.zones, focus, scale, title));
            if shown.surface.resize(width, height).is_ok()
                && let Ok(mut buffer) = shown.surface.buffer_mut()
            {
                for (out, pixel) in buffer.iter_mut().zip(canvas.pixels().as_chunks::<4>().0) {
                    // The frame is opaque, so premultiplied and straight colors agree.
                    *out =
                        u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2]);
                }
                let _ = buffer.present();
            }
            self.text = Some(canvas.into_text_engine());
            let edge = self.pointer.and_then(|point| edge_at(&shown.window, point));
            shown.window.set_cursor(match (edge, output.cursor) {
                (Some(ResizeDirection::East | ResizeDirection::West), _)
                | (None, Cursor::ResizeHorizontal) => CursorIcon::EwResize,
                (Some(ResizeDirection::North | ResizeDirection::South), _)
                | (None, Cursor::ResizeVertical) => CursorIcon::NsResize,
                (Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast), _) => {
                    CursorIcon::NwseResize
                }
                (Some(_), _) => CursorIcon::NeswResize,
                (None, Cursor::Default) => CursorIcon::Default,
                (None, Cursor::Pointer) => CursorIcon::Pointer,
                (None, Cursor::Text) => CursorIcon::Text,
            });
            match request {
                Some(WindowRequest::Minimize) => shown.window.set_minimized(true),
                Some(WindowRequest::ToggleMaximize) => {
                    shown.window.set_maximized(!shown.window.is_maximized());
                }
                Some(WindowRequest::Close) => event_loop.exit(),
                None => {}
            }
            // softbuffer does not wait for the display, so an animation's frames are paced
            // here: the next one a frame's time after this one began, not at once.
            let next = output.repaint.then(|| began + ANIMATION_FRAME);
            let later = output.repaint_after.map(|after| Instant::now() + after);
            self.wake_at = match (next, later) {
                (Some(next), Some(later)) => Some(next.min(later)),
                (next, later) => next.or(later),
            };
        }
    }

    impl ApplicationHandler<UserEvent> for Host {
        fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
            let Some(shown) = &self.window else { return };
            let UserEvent::Access(event) = event else {
                shown.window.request_redraw();
                return;
            };
            match event.window_event {
                accesskit_winit::WindowEvent::ActionRequested(request) => {
                    if let Some(request) = Request::from_action(&request) {
                        self.state.request(request);
                    }
                }
                // The next frame sends the whole tree.
                accesskit_winit::WindowEvent::InitialTreeRequested => {}
                accesskit_winit::WindowEvent::AccessibilityDeactivated => return,
            }
            shown.window.request_redraw();
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            match self.wake_at {
                Some(at) if at <= Instant::now() => {
                    self.wake_at = None;
                    if let Some(shown) = &self.window {
                        shown.window.request_redraw();
                    }
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
                Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
        }

        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title(self.title.clone())
                .with_inner_size(LogicalSize::new(self.size.0, self.size.1))
                .with_min_inner_size(LogicalSize::new(640.0, 400.0))
                // AccessKit's adapter must exist before the window first shows.
                .with_visible(false);
            #[cfg(target_os = "macos")]
            let attributes = crate::macos::frame(attributes);
            #[cfg(not(target_os = "macos"))]
            let attributes = attributes.with_decorations(false);
            let Ok(window) = event_loop.create_window(attributes) else {
                event_loop.exit();
                return;
            };
            let access = accesskit_winit::Adapter::with_event_loop_proxy(
                event_loop,
                &window,
                self.proxy.clone(),
            );
            #[cfg(target_os = "macos")]
            crate::macos::keep_title_bar_clicks(&window);
            window.set_visible(true);
            let window = Arc::new(window);
            let Ok(context) = Context::new(window.clone()) else {
                event_loop.exit();
                return;
            };
            let Ok(surface) = Surface::new(&context, window.clone()) else {
                event_loop.exit();
                return;
            };
            self.app
                .set_waker(super::event_loop_waker(self.proxy.clone(), || {
                    UserEvent::Wake
                }));
            #[cfg(target_os = "macos")]
            {
                let waker = window.clone();
                crate::menu_bar::add_menu_items(&self.title, move || waker.request_redraw());
            }
            window.request_redraw();
            self.window = Some(Shown {
                window,
                surface,
                access,
            });
        }

        #[allow(clippy::too_many_lines)] // One match over the window's events.
        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            let Some(shown) = self.window.as_mut() else {
                return;
            };
            shown.access.process_event(&shown.window, &event);
            let window = shown.window.clone();
            #[allow(clippy::cast_possible_truncation)] // Scale factors are small.
            let scale = window.scale_factor() as f32;
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => {
                    self.redraw(event_loop);
                    return;
                }
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {}
                WindowEvent::ModifiersChanged(modifiers) => {
                    let state = modifiers.state();
                    self.modifiers = Modifiers {
                        ctrl: state.control_key(),
                        shift: state.shift_key(),
                        alt: state.alt_key(),
                        logo: state.super_key(),
                    };
                    self.events.push(Event::Modifiers(self.modifiers));
                    return;
                }
                WindowEvent::CursorMoved { position, .. } => {
                    #[allow(clippy::cast_possible_truncation)] // Window coordinates fit in f32.
                    let point = Point::new(position.x as f32 / scale, position.y as f32 / scale);
                    self.pointer = Some(point);
                    self.events.push(Event::PointerMoved(point));
                }
                WindowEvent::CursorLeft { .. } => {
                    self.pointer = None;
                    self.events.push(Event::PointerLeft);
                }
                // Files from Finder or a file manager (macOS and X11; winit has no file drops on
                // Wayland yet), one event per file.
                WindowEvent::DroppedFile(path) => {
                    self.events.push(Event::FilesDropped(vec![path]));
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let Some(button) = self.buttons.button(button, state, self.modifiers) else {
                        return;
                    };
                    if state == ElementState::Pressed && button == Button::Primary {
                        if let Some(direction) =
                            self.pointer.and_then(|point| edge_at(&window, point))
                        {
                            let _ = window.drag_resize_window(direction);
                            return;
                        }
                        let over_drag = self.pointer.is_some_and(|point| {
                            let drag = self.app.drag_zone();
                            self.state
                                .zones()
                                .iter()
                                .rev()
                                .find(|zone| zone.rect.contains(point))
                                .is_some_and(|zone| zone.id == drag)
                        });
                        if over_drag {
                            // The double click that maximizes still reaches the app.
                            self.events.push(Event::PointerDown(button));
                            self.events.push(Event::PointerUp(button));
                            let _ = window.drag_window();
                            window.request_redraw();
                            return;
                        }
                    }
                    self.events.push(match state {
                        ElementState::Pressed => Event::PointerDown(button),
                        ElementState::Released => Event::PointerUp(button),
                    });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (x, y) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                        #[allow(clippy::cast_possible_truncation)] // Wheel deltas are small.
                        MouseScrollDelta::PixelDelta(position) => {
                            (position.x as f32 / scale, position.y as f32 / scale)
                        }
                    };
                    self.events.push(Event::Wheel { x, y });
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    let insert = event.logical_key == WinitKey::Named(NamedKey::Insert);
                    let bare = event.key_without_modifiers();
                    let mapped = pressed_key(&event.logical_key, &bare, self.modifiers);
                    let paste_key = mapped.unwrap_or(kit::Key::Space);
                    if is_paste(self.state.focused_role(), paste_key, self.modifiers, insert) {
                        if let Some(paste) = paste_event() {
                            self.events.push(paste);
                        }
                        window.request_redraw();
                        return;
                    }
                    if let Some(key) = mapped {
                        self.events.push(Event::Key(key, self.modifiers));
                    }
                    if let Some(text) = event.text.as_ref()
                        && !self.modifiers.ctrl
                        && !self.modifiers.logo
                        && text.chars().all(|c| !c.is_control())
                    {
                        self.events.push(Event::Text(text.to_string()));
                    }
                }
                _ => return,
            }
            window.request_redraw();
        }
    }

    /// The key a press names, from its `logical` key and its `bare` one (without modifiers).
    /// With Ctrl or Cmd held, a character key is named by the key itself, not by the
    /// character Shift or Option would make of it: Cmd+Shift+] is "]", not "}". The shortcut
    /// table writes chords that way, and Windows names keys so (`win32::map_key`).
    fn pressed_key(logical: &WinitKey, bare: &WinitKey, modifiers: Modifiers) -> Option<kit::Key> {
        match bare {
            WinitKey::Character(_) if modifiers.ctrl || modifiers.logo => key(bare),
            _ => key(logical),
        }
    }

    fn key(key: &WinitKey) -> Option<kit::Key> {
        Some(match key {
            WinitKey::Named(named) => match named {
                NamedKey::Tab => kit::Key::Tab,
                NamedKey::Enter => kit::Key::Enter,
                NamedKey::Escape => kit::Key::Escape,
                NamedKey::Space => kit::Key::Space,
                NamedKey::Backspace => kit::Key::Backspace,
                NamedKey::Delete => kit::Key::Delete,
                NamedKey::ArrowLeft => kit::Key::Left,
                NamedKey::ArrowRight => kit::Key::Right,
                NamedKey::ArrowUp => kit::Key::Up,
                NamedKey::ArrowDown => kit::Key::Down,
                NamedKey::Home => kit::Key::Home,
                NamedKey::End => kit::Key::End,
                NamedKey::PageUp => kit::Key::PageUp,
                NamedKey::PageDown => kit::Key::PageDown,
                NamedKey::F1 => kit::Key::F(1),
                NamedKey::F2 => kit::Key::F(2),
                NamedKey::F3 => kit::Key::F(3),
                NamedKey::F4 => kit::Key::F(4),
                NamedKey::F5 => kit::Key::F(5),
                NamedKey::F6 => kit::Key::F(6),
                NamedKey::F7 => kit::Key::F(7),
                NamedKey::F8 => kit::Key::F(8),
                NamedKey::F9 => kit::Key::F(9),
                NamedKey::F10 => kit::Key::F(10),
                NamedKey::F11 => kit::Key::F(11),
                NamedKey::F12 => kit::Key::F(12),
                _ => return None,
            },
            WinitKey::Character(text) => kit::Key::Char(text.chars().next()?.to_ascii_lowercase()),
            _ => return None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_chord_names_the_key_pressed_not_the_character_shift_makes() {
            let character = |text: &str| WinitKey::Character(text.into());
            let cmd_shift = Modifiers {
                logo: true,
                shift: true,
                ..Modifiers::NONE
            };
            // Cmd+Shift+] on a Mac, Ctrl+Shift+. on Linux.
            let close = (character("}"), character("]"));
            assert_eq!(
                pressed_key(&close.0, &close.1, cmd_shift),
                Some(kit::Key::Char(']'))
            );
            let period = (character(">"), character("."));
            assert_eq!(
                pressed_key(&period.0, &period.1, Modifiers::CTRL_SHIFT),
                Some(kit::Key::Char('.'))
            );
            // Without Ctrl or Cmd the character stays what was typed.
            assert_eq!(
                pressed_key(&close.0, &close.1, Modifiers::SHIFT),
                Some(kit::Key::Char('}'))
            );
            let enter = WinitKey::Named(NamedKey::Enter);
            assert_eq!(
                pressed_key(&enter, &enter, cmd_shift),
                Some(kit::Key::Enter)
            );
        }

        #[test]
        fn on_a_mac_a_ctrl_click_is_a_right_click_from_press_to_release() {
            let mut buttons = Buttons::default();
            let ctrl_click = if cfg!(target_os = "macos") {
                Button::Secondary
            } else {
                Button::Primary
            };
            let left = |buttons: &mut Buttons, state, modifiers| {
                buttons.button(MouseButton::Left, state, modifiers)
            };
            assert_eq!(
                left(&mut buttons, ElementState::Pressed, Modifiers::CTRL),
                Some(ctrl_click)
            );
            // Ctrl let go before the release: still the same click.
            assert_eq!(
                left(&mut buttons, ElementState::Released, Modifiers::NONE),
                Some(ctrl_click)
            );
            assert_eq!(
                left(&mut buttons, ElementState::Pressed, Modifiers::NONE),
                Some(Button::Primary)
            );
            assert_eq!(
                left(&mut buttons, ElementState::Released, Modifiers::CTRL),
                Some(Button::Primary)
            );
            assert_eq!(
                buttons.button(MouseButton::Right, ElementState::Pressed, Modifiers::NONE),
                Some(Button::Secondary)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copied_secret_is_cleared_after_its_time_unless_something_else_was_copied() {
        let now = Instant::now();
        let held = HeldSecret::new("ABCD-EFGH", now);
        let later = now + kit::SECRET_ON_CLIPBOARD;
        assert!(!held.clears(Some("ABCD-EFGH"), now), "not before its time");
        assert!(held.clears(Some("ABCD-EFGH"), later));
        assert!(
            !held.clears(Some("a command the user copied"), later),
            "what the user copied since stays"
        );
        assert!(!held.clears(None, later));
    }

    #[test]
    fn the_command_key_and_v_paste_in_a_text_field_and_on_macos_anywhere() {
        let v = Key::Char('v');
        let field = Some(Role::TextInput);
        assert!(is_paste(field, v, Modifiers::COMMAND, false));
        assert_eq!(
            is_paste(Some(Role::Terminal), v, Modifiers::COMMAND, false),
            cfg!(target_os = "macos"),
            "outside a text field, Ctrl+V belongs to the terminal's program; Cmd+V is free"
        );
        assert!(is_paste(
            Some(Role::Terminal),
            v,
            Modifiers::CTRL_SHIFT,
            false
        ));
        assert!(!is_paste(field, Key::Char('c'), Modifiers::COMMAND, false));
    }
}
