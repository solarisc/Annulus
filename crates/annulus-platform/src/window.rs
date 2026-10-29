//! The window our toolkit runs in ([ADR-0017] decision 4): on Windows our own Win32 window
//! drawn by Direct2D (`crate::win32`), elsewhere `winit` + `softbuffer` drawn by the CPU
//! renderer (below).
//!
//! The window has no OS title bar: the app draws its own and names the zones that drag the
//! window and maximize it. Frames are drawn only when input arrives or the app asks for one
//! (P-06).
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![forbid(unsafe_code)]

use annulus_ui::kit::{Id, Theme, Ui};

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

/// An app the window runs.
pub trait WindowApp {
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
    return crate::win32::run(title, size, Box::new(app));
    #[cfg(not(windows))]
    return portable::run(title, size, Box::new(app));
}

#[cfg(not(windows))]
mod portable {
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::time::Instant;

    use softbuffer::{Context, Surface};
    use annulus_ui::gfx::{Point, Rect};
    use annulus_ui::kit::{self, Button, Cursor, Event, Modifiers, Ui, UiState};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::keyboard::{Key as WinitKey, NamedKey};
    use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

    use crate::cpu::{CpuCanvas, TextEngine};

    use super::{WindowApp, WindowError, WindowRequest};

    /// Runs `app` in a `winit` window until it closes.
    pub(super) fn run(
        title: &str,
        size: (f32, f32),
        app: Box<dyn WindowApp>,
    ) -> Result<(), WindowError> {
        let event_loop = EventLoop::new().map_err(|e| WindowError::Platform(e.to_string()))?;
        let mut host = Host {
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
        };
        event_loop
            .run_app(&mut host)
            .map_err(|e| WindowError::Platform(e.to_string()))
    }

    struct Shown {
        window: Rc<Window>,
        surface: Surface<Rc<Window>, Rc<Window>>,
    }

    struct Host {
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
            let output = self.state.end_frame();
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
                (Some(ResizeDirection::East | ResizeDirection::West), _) => CursorIcon::EwResize,
                (Some(ResizeDirection::North | ResizeDirection::South), _) => CursorIcon::NsResize,
                (Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast), _) => {
                    CursorIcon::NwseResize
                }
                (Some(_), _) => CursorIcon::NeswResize,
                (None, Cursor::Default) => CursorIcon::Default,
                (None, Cursor::Pointer) => CursorIcon::Pointer,
                (None, Cursor::Text) => CursorIcon::Text,
                (None, Cursor::ResizeHorizontal) => CursorIcon::EwResize,
                (None, Cursor::ResizeVertical) => CursorIcon::NsResize,
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
        }
    }

    impl ApplicationHandler for Host {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title(self.title.clone())
                .with_inner_size(LogicalSize::new(self.size.0, self.size.1))
                .with_min_inner_size(LogicalSize::new(640.0, 400.0))
                .with_decorations(false);
            let Ok(window) = event_loop.create_window(attributes) else {
                event_loop.exit();
                return;
            };
            let window = Rc::new(window);
            let Ok(context) = Context::new(window.clone()) else {
                event_loop.exit();
                return;
            };
            let Ok(surface) = Surface::new(&context, window.clone()) else {
                event_loop.exit();
                return;
            };
            window.request_redraw();
            self.window = Some(Shown { window, surface });
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            let Some(shown) = self.window.as_ref() else {
                return;
            };
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
                    if let Some(key) = key(&event.logical_key) {
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
