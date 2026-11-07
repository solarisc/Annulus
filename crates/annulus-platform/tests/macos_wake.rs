//! The window's waker (`annulus_platform::window::event_loop_waker`) on a real macOS event
//! loop: a thread that holds a lock wakes the window while the main thread waits for that same
//! lock to draw, the way a terminal's reader thread wakes it in the middle of parsing a title.
//! The wake must arrive and neither thread may wait for the other. winit's `request_redraw`
//! from another thread waits for the main thread on macOS, which froze every terminal tab.
//!
//! The event loop only lives on the main thread, so this runs without the test harness. It is
//! headless: the window is transparent and off every screen, and its app never comes to the
//! front.

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
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use annulus_platform::macos;
    use annulus_platform::window::event_loop_waker;
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use winit::application::ApplicationHandler;
    use winit::dpi::{LogicalPosition, LogicalSize};
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS as _};
    use winit::window::{Window, WindowId};

    /// How long the wake may take before the test calls it a freeze.
    const LIMIT: Duration = Duration::from_secs(10);

    struct Woken;

    struct Test {
        proxy: EventLoopProxy<Woken>,
        window: Option<Window>,
        /// The main thread took the lock after the other thread let it go.
        drew: bool,
    }

    impl ApplicationHandler<Woken> for Test {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_inner_size(LogicalSize::new(300.0, 200.0))
                        .with_position(LogicalPosition::new(-30_000.0, -30_000.0))
                        .with_visible(false)
                        .with_active(false),
                )
                .unwrap();
            let view = macos::ns_view(&window).expect("an AppKit view");
            // SAFETY: `view` is the window's live NSView; `window` returns its NSWindow, and
            // `setAlphaValue:` takes a CGFloat.
            unsafe {
                let ns: *mut AnyObject = msg_send![view, window];
                let _: () = msg_send![ns, setAlphaValue: 0.0_f64];
            }
            window.set_visible(true);
            self.window = Some(window);

            let waker = event_loop_waker(self.proxy.clone(), || Woken);
            let term = Arc::new(Mutex::new(()));
            let (held, is_held) = channel();
            let reader = Arc::clone(&term);
            std::thread::spawn(move || {
                let parsing = reader.lock().unwrap();
                held.send(()).unwrap();
                // Give the main thread time to start waiting for the lock.
                std::thread::sleep(Duration::from_millis(100));
                waker();
                drop(parsing);
            });
            is_held.recv().unwrap();
            // A frame: it reads the terminal, so it waits for the lock.
            drop(term.lock().unwrap());
            self.drew = true;
        }

        fn user_event(&mut self, _: &ActiveEventLoop, _: Woken) {
            if self.drew {
                std::process::exit(0);
            }
            eprintln!("FAIL: the wake arrived before the main thread drew");
            std::process::exit(1);
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    pub(super) fn run() {
        std::thread::spawn(|| {
            std::thread::sleep(LIMIT);
            eprintln!(
                "FAIL: no wake within {LIMIT:?}: the waker waited for the main thread, which \
                 waited for the lock"
            );
            std::process::exit(1);
        });
        let event_loop = EventLoop::<Woken>::with_user_event()
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_activate_ignoring_other_apps(false)
            .with_default_menu(false)
            .build()
            .unwrap();
        let proxy = event_loop.create_proxy();
        event_loop
            .run_app(&mut Test {
                proxy,
                window: None,
                drew: false,
            })
            .unwrap();
        panic!("the event loop ended before the test did");
    }
}
