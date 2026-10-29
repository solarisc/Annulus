//! The window our toolkit runs in ([ADR-0017] decision 4): on Windows our own Win32 window
//! drawn by Direct2D (`crate::win32`), elsewhere `winit` + `softbuffer` drawn by the CPU
//! renderer (below).
//!
//! The window has no OS title bar: the app draws its own and names the zones that drag the
//! window and maximize it. Frames are drawn only when input arrives or the app asks for one
//! (P-06). Screen readers get the tree of each frame's zones (`annulus_ui::access`) through
//! AccessKit: UI Automation on Windows, AT-SPI on Linux, `NSAccessibility` on macOS.
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

/// Asks the window for a new frame; callable from any thread (a session with new output).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

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
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let current = clipboard.get_text().ok().map(Zeroizing::new);
        if secret.clears(current.as_ref().map(|text| text.as_str()), now) {
            let _ = clipboard.clear();
        }
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

/// Whether a key press pastes: Ctrl+Shift+V and Shift+Insert anywhere, Ctrl+V only in a text
/// field (in a terminal it belongs to the program).
pub(crate) fn is_paste(state: &UiState, key: Key, modifiers: Modifiers, insert: bool) -> bool {
    let v = key == Key::Char('v');
    (insert && modifiers == Modifiers::SHIFT)
        || (v && modifiers == Modifiers::CTRL_SHIFT)
        || (v && modifiers == Modifiers::CTRL && state.focused_role() == Some(Role::TextInput))
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

    use softbuffer::{Context, Surface};
    use annulus_ui::access::{self, Request};
    use annulus_ui::gfx::{Point, Rect};
    use annulus_ui::kit::{self, Button, Cursor, Event, Modifiers, Ui, UiState};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::keyboard::{Key as WinitKey, NamedKey};
    use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

    use std::sync::Arc;

    use crate::cpu::{CpuCanvas, TextEngine};

    use super::{WindowApp, WindowError, WindowRequest, is_paste, paste_event, publish_clipboard};

    /// A screen reader's news, sent through the event loop by AccessKit's adapter.
    pub(super) struct Access(accesskit_winit::Event);

    impl From<accesskit_winit::Event> for Access {
        fn from(event: accesskit_winit::Event) -> Self {
            Self(event)
        }
    }

    /// Runs `app` in a `winit` window until it closes.
    pub(super) fn run(
        title: &str,
        size: (f32, f32),
        app: Box<dyn WindowApp>,
    ) -> Result<(), WindowError> {
        let event_loop = EventLoop::<Access>::with_user_event()
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
        proxy: EventLoopProxy<Access>,
        title: String,
        size: (f32, f32),
        app: Box<dyn WindowApp>,
        window: Option<Shown>,
        state: UiState,
        text: Option<TextEngine>,
        events: Vec<Event>,
        modifiers: Modifiers,
        pointer: Option<Point>,
        started: Instant,
        /// When the app asked for its next frame without input.
        wake_at: Option<Instant>,
    }

    /// The band along the window's edges that resizes it, in DIPs.
    const EDGE: f32 = 5.0;

    /// The resize direction of the window edge under `point`, if any.
    fn edge_at(window: &Window, point: Point) -> Option<ResizeDirection> {
        if window.is_maximized() {
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
            let time = self.started.elapsed().as_secs_f64();
            self.state.begin_frame(&events, time);
            #[allow(clippy::cast_precision_loss)] // Window sizes fit in f32.
            let viewport = Rect::new(
                0.0,
                0.0,
                width.get() as f32 / scale,
                height.get() as f32 / scale,
            );
            let theme = self.app.theme();
            let maximized = shown.window.is_maximized();
            let request = {
                let mut ui = Ui::new(&mut canvas, theme, &mut self.state, viewport);
                self.app.frame(&mut ui, maximized)
            };
            let mut output = self.state.end_frame();
            publish_clipboard(&mut self.state, &mut output);
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
            if output.repaint {
                shown.window.request_redraw();
            }
            self.wake_at = output.repaint_after.map(|after| Instant::now() + after);
        }
    }

    impl ApplicationHandler<Access> for Host {
        fn user_event(&mut self, _: &ActiveEventLoop, event: Access) {
            let Some(shown) = &self.window else { return };
            match event.0.window_event {
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
                .with_decorations(false)
                // AccessKit's adapter must exist before the window first shows.
                .with_visible(false);
            let Ok(window) = event_loop.create_window(attributes) else {
                event_loop.exit();
                return;
            };
            let access = accesskit_winit::Adapter::with_event_loop_proxy(
                event_loop,
                &window,
                self.proxy.clone(),
            );
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
            let waker = window.clone();
            self.app.set_waker(Arc::new(move || waker.request_redraw()));
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
                WindowEvent::MouseInput { state, button, .. } => {
                    let button = match button {
                        MouseButton::Left => Button::Primary,
                        MouseButton::Right => Button::Secondary,
                        MouseButton::Middle => Button::Middle,
                        _ => return,
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
                    let mapped = key(&event.logical_key);
                    let paste_key = mapped.unwrap_or(kit::Key::Space);
                    if is_paste(&self.state, paste_key, self.modifiers, insert) {
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
                _ => return None,
            },
            WinitKey::Character(text) => kit::Key::Char(text.chars().next()?.to_ascii_lowercase()),
            _ => return None,
        })
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
}
