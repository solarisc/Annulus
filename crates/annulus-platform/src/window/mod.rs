//! Windows and the event loop (winit, ADR-0012). One window shows one widget tree: OS input
//! goes in, frames come out through the CPU renderer and `softbuffer`. Nothing is redrawn
//! unless the tree changed, so a still window does no work (P-06).
//!
//! On Windows the Direct2D backend replaces the CPU renderer here in a later step.

pub mod input;

use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Surface};
use annulus_ui::tokens::Appearance;
use annulus_ui::{Action, Event, NodeId, Point, Size, Tree};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::cpu::{Canvas, TextEngine};

/// Why the window could not run.
#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    /// The OS event loop could not start or failed.
    #[error("event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
}

/// What the app does with the window's tree: react to widget actions and the OS theme.
pub trait WindowApp {
    /// The window title.
    fn title(&self) -> String;

    /// Called with each action widgets reported, after the event that caused it.
    fn on_action(&mut self, tree: &mut Tree, node: NodeId, action: Action);

    /// Called when the OS switches between light and dark.
    fn on_appearance(&mut self, tree: &mut Tree, appearance: Appearance);
}

/// Opens one window showing `tree`, drawing text with `text`, and runs until it is closed.
///
/// # Errors
/// [`WindowError`] if the OS event loop fails.
pub fn run(tree: Tree, text: TextEngine, app: impl WindowApp) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    // Sleep until the OS has something for us: idle means idle.
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner {
        app,
        tree,
        text: Some(text),
        window: None,
        modifiers: ModifiersState::empty(),
        cursor: Point::default(),
    };
    event_loop.run_app(&mut runner)?;
    Ok(())
}

struct Shown {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
}

struct Runner<A> {
    app: A,
    tree: Tree,
    /// Lent to each frame's canvas while it draws.
    text: Option<TextEngine>,
    window: Option<Shown>,
    modifiers: ModifiersState,
    cursor: Point,
}

impl<A: WindowApp> Runner<A> {
    fn scale(&self) -> f64 {
        self.window
            .as_ref()
            .map_or(1.0, |shown| shown.window.scale_factor())
    }

    /// Routes `event`, hands the app any actions, and asks for a frame if anything changed.
    fn dispatch(&mut self, event: &Event) {
        self.tree.dispatch(event);
        for (node, action) in self.tree.take_actions() {
            self.app.on_action(&mut self.tree, node, action);
        }
        self.request_redraw_if_needed();
    }

    fn request_redraw_if_needed(&self) {
        let root = self.tree.root();
        let dirty = self.tree.dirty(root).is_some_and(|dirty| !dirty.is_empty())
            || !self.tree.damage().is_empty();
        if let Some(shown) = &self.window
            && dirty
        {
            shown.window.request_redraw();
        }
    }

    fn redraw(&mut self) {
        let Some(shown) = &mut self.window else {
            return;
        };
        let physical = shown.window.inner_size();
        let (Some(width), Some(height)) = (
            NonZeroU32::new(physical.width),
            NonZeroU32::new(physical.height),
        ) else {
            return;
        };
        let scale = shown.window.scale_factor();
        #[allow(clippy::cast_possible_truncation)] // Window sizes and scales are small.
        let viewport = Size::new(
            (f64::from(physical.width) / scale) as f32,
            (f64::from(physical.height) / scale) as f32,
        );
        let Some(mut text) = self.text.take() else {
            return;
        };
        self.tree.layout(viewport, &mut text);
        let list = self.tree.paint(&mut text);

        #[allow(clippy::cast_possible_truncation)] // Same.
        let Ok(mut canvas) = Canvas::new(physical.width, physical.height, scale as f32) else {
            self.text = Some(text);
            return;
        };
        let bg = self.tree.palette().bg_app;
        canvas.clear(annulus_ui::Color::rgb(bg.r, bg.g, bg.b));
        canvas.set_text_engine(text);
        canvas.draw(&list);
        self.text = canvas.take_text_engine();
        if shown.surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = shown.surface.buffer_mut() else {
            return;
        };
        for (pixel, rgba) in buffer.iter_mut().zip(canvas.pixels().chunks_exact(4)) {
            // The frame is opaque, so premultiplied and straight color are the same.
            *pixel = (u32::from(rgba[0]) << 16) | (u32::from(rgba[1]) << 8) | u32::from(rgba[2]);
        }
        let _ = buffer.present();
    }
}

impl<A: WindowApp> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.app.title())
            .with_inner_size(LogicalSize::new(1100.0, 720.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0));
        let Ok(window) = event_loop.create_window(attributes) else {
            event_loop.exit();
            return;
        };
        let window = Rc::new(window);
        let surface =
            Context::new(window.clone()).and_then(|context| Surface::new(&context, window.clone()));
        let Ok(surface) = surface else {
            event_loop.exit();
            return;
        };
        if let Some(theme) = window.theme() {
            self.app.on_appearance(&mut self.tree, appearance(theme));
        }
        window.request_redraw();
        self.window = Some(Shown { window, surface });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let scale = self.scale();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(shown) = &self.window {
                    shown.window.request_redraw();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                self.app.on_appearance(&mut self.tree, appearance(theme));
                self.request_redraw_if_needed();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                #[allow(clippy::cast_possible_truncation)] // Window coordinates are small.
                let at = Point::new((position.x / scale) as f32, (position.y / scale) as f32);
                self.cursor = at;
                self.dispatch(&Event::PointerMove { position: at });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(button) = input::button(button) else {
                    return;
                };
                let position = self.cursor;
                let event = if input::pressed(state) {
                    Event::PointerDown { position, button }
                } else {
                    Event::PointerUp { position, button }
                };
                self.dispatch(&event);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let event = Event::Wheel {
                    position: self.cursor,
                    delta: input::wheel(delta, scale),
                };
                self.dispatch(&event);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        text,
                        state,
                        ..
                    },
                ..
            } => {
                if !input::pressed(state) {
                    return;
                }
                let modifiers = input::modifiers(self.modifiers);
                let command = modifiers.ctrl || modifiers.meta || modifiers.alt;
                // Typed text goes in as text; keys with a command modifier are shortcuts.
                if let Some(typed) = input::typed_text(text.as_deref()).filter(|_| !command) {
                    self.dispatch(&Event::Text(typed));
                } else if let Some(key) = input::key(&logical_key) {
                    self.dispatch(&Event::KeyDown { key, modifiers });
                }
            }
            _ => {}
        }
    }
}

fn appearance(theme: winit::window::Theme) -> Appearance {
    match theme {
        winit::window::Theme::Light => Appearance::Light,
        winit::window::Theme::Dark => Appearance::Dark,
    }
}
