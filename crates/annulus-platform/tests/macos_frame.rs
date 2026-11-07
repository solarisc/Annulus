//! The macOS window's frame (`annulus_platform::macos`), on a real macOS window: a click in
//! the title bar's band reaches the app even when the window is asked whether it is maximized
//! while the button is down (on a borderless window that lost every release, so no click
//! landed), and the traffic lights sit centered in our title bar.
//!
//! Such windows only live on the main thread, so this runs without the test harness. It is
//! headless: the window is transparent and off every screen, its app never comes to the
//! front, and the clicks are handed to the window, never to the mouse.

// Not a `#[test]` (see above), so clippy's allowances for tests are spelled out.
#![allow(
    unsafe_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::{Duration, Instant};

    use annulus_platform::macos::{self, NsPoint, NsRect};
    use annulus_platform::window::{TRAFFIC_LIGHTS_LEFT, TRAFFIC_LIGHTS_W};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use winit::application::ApplicationHandler;
    use winit::dpi::{LogicalPosition, LogicalSize};
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS as _};
    use winit::window::{Window, WindowId};

    const WIDTH: f64 = 600.0;
    const HEIGHT: f64 = 400.0;
    /// The app's title bar.
    const BAND: f64 = 44.0;

    #[derive(Default)]
    struct Test {
        window: Option<Window>,
        clicks: Vec<ElementState>,
        /// Where the press landed, from the window's top left.
        pressed_at: Option<(f64, f64)>,
        sent: Option<Instant>,
    }

    fn ns_window(window: &Window) -> *mut AnyObject {
        let view = macos::ns_view(window).expect("an AppKit view");
        // SAFETY: `view` is the window's live NSView; `window` returns its NSWindow.
        unsafe { msg_send![view, window] }
    }

    /// Hands `window` a left button event at `at`, in window coordinates (from the bottom).
    fn click(window: *mut AnyObject, kind: usize, at: NsPoint) {
        // SAFETY: `window` is a live NSWindow. `mouseEventWithType:...` builds an autoreleased
        // event from plain values (nil for the unused graphics context), and `sendEvent:`
        // dispatches it the way AppKit dispatches a real click.
        unsafe {
            let number: isize = msg_send![window, windowNumber];
            let event: Option<Retained<AnyObject>> = msg_send![
                class!(NSEvent),
                mouseEventWithType: kind,
                location: at,
                modifierFlags: 0_usize,
                timestamp: 0.0_f64,
                windowNumber: number,
                context: std::ptr::null_mut::<AnyObject>(),
                eventNumber: 0_isize,
                clickCount: 1_isize,
                pressure: 1.0_f32
            ];
            let event = event.expect("an NSEvent");
            let _: () = msg_send![window, sendEvent: &*event];
        }
    }

    /// Where each traffic light the window has is drawn, in window coordinates (from the
    /// bottom).
    fn buttons(window: *mut AnyObject) -> Vec<NsRect> {
        (0..3_usize)
            .filter_map(|kind| {
                // SAFETY: `window` is a live NSWindow; `standardWindowButton:` gives a button
                // or nil, and `bounds` and `convertRect:toView:` take and give NSRects (a nil
                // view: the window's own coordinates).
                unsafe {
                    let button: *mut AnyObject = msg_send![window, standardWindowButton: kind];
                    if button.is_null() {
                        return None;
                    }
                    let bounds: NsRect = msg_send![button, bounds];
                    Some(msg_send![
                        button,
                        convertRect: bounds,
                        toView: std::ptr::null_mut::<AnyObject>()
                    ])
                }
            })
            .collect()
    }

    impl ApplicationHandler for Test {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = macos::frame(
                Window::default_attributes()
                    .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
                    .with_position(LogicalPosition::new(-30_000.0, -30_000.0))
                    .with_visible(false)
                    .with_active(false),
            );
            let window = event_loop.create_window(attributes).unwrap();
            macos::keep_title_bar_clicks(&window);
            // SAFETY: a live NSWindow; `setAlphaValue:` takes a CGFloat.
            unsafe {
                let _: () = msg_send![ns_window(&window), setAlphaValue: 0.0_f64];
            }
            window.set_visible(true);
            self.window = Some(window);
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CursorMoved { position, .. } if self.clicks.is_empty() => {
                    let scale = self.window.as_ref().map_or(1.0, Window::scale_factor);
                    self.pressed_at = Some((position.x / scale, position.y / scale));
                }
                WindowEvent::MouseInput { state, .. } => self.clicks.push(state),
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            let Some(window) = &self.window else { return };
            let ns = ns_window(window);
            match self.sent {
                None => {
                    // Over a tab, 10 points below the window's top edge.
                    let at = NsPoint {
                        x: 300.0,
                        y: HEIGHT - 10.0,
                    };
                    click(ns, 1, at); // NSEventTypeLeftMouseDown
                    // The window host asks this of every frame drawn while the button is down.
                    let _ = window.is_maximized();
                    click(ns, 2, at); // NSEventTypeLeftMouseUp
                    macos::place_buttons(window, BAND);
                    self.sent = Some(Instant::now());
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        Instant::now() + Duration::from_millis(250),
                    ));
                }
                Some(sent) if sent.elapsed() >= Duration::from_millis(200) => {
                    let failures = self.check(ns);
                    for failure in &failures {
                        eprintln!("FAIL: {failure}");
                    }
                    std::process::exit(i32::from(!failures.is_empty()));
                }
                Some(_) => {}
            }
        }
    }

    impl Test {
        fn check(&self, window: *mut AnyObject) -> Vec<String> {
            let mut failures = Vec::new();
            if self.clicks != [ElementState::Pressed, ElementState::Released] {
                failures.push(format!(
                    "a click in the title bar's band reached the app as {:?}",
                    self.clicks
                ));
            }
            match self.pressed_at {
                Some((x, y)) if (x - 300.0).abs() < 1.0 && (y - 10.0).abs() < 1.0 => {}
                other => failures.push(format!("the press landed at {other:?}, not (300, 10)")),
            }
            let placed = buttons(window);
            if placed.len() != 3 {
                failures.push(format!("the window has {} of its 3 buttons", placed.len()));
                return failures;
            }
            let first = placed[0].origin.x;
            if (first - f64::from(TRAFFIC_LIGHTS_LEFT)).abs() > 0.5 {
                failures.push(format!("the close button starts at x {first}"));
            }
            for (index, rect) in placed.iter().enumerate() {
                let middle = rect.origin.y + rect.size.height / 2.0;
                if (middle - (HEIGHT - BAND / 2.0)).abs() > 1.0 {
                    failures.push(format!(
                        "button {index} is at {rect:?}: its middle is not {} below the top",
                        BAND / 2.0
                    ));
                }
            }
            // The title bar keeps room for all three, and a gap.
            let end = placed[2].origin.x + placed[2].size.width;
            if end + 6.0 > f64::from(TRAFFIC_LIGHTS_W) {
                failures.push(format!("the buttons end at x {end}, past the room kept"));
            }
            failures
        }
    }

    pub(super) fn run() {
        let event_loop = EventLoop::builder()
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_activate_ignoring_other_apps(false)
            .with_default_menu(false)
            .build()
            .unwrap();
        event_loop.run_app(&mut Test::default()).unwrap();
        panic!("the event loop ended before the test did");
    }
}
