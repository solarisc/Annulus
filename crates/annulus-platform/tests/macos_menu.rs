//! Our items in the macOS menu bar (`annulus_platform::menu_bar`), in winit's real app menu:
//! where they sit and what they are called, the keys shown beside them, and that choosing one
//! (by a click or by its keys) reaches the window host and wakes the window.
//!
//! `AppKit`'s menus only live on the main thread, so this runs without the test harness. It is
//! headless: no window opens, and the app never comes to the front.

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
    use std::cell::Cell;
    use std::ffi::{CStr, CString};
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use annulus_platform::macos::NsPoint;
    use annulus_platform::menu_bar;
    use annulus_platform::window::MenuItem;
    use annulus_ui::kit::{Key, Modifiers};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS as _};
    use winit::window::WindowId;

    const CMD: Modifiers = Modifiers {
        logo: true,
        ..Modifiers::NONE
    };
    const CMD_CTRL: Modifiers = Modifiers {
        logo: true,
        ctrl: true,
        ..Modifiers::NONE
    };
    /// `NSEventModifierFlagControl` and `…Command`.
    const CONTROL: usize = 1 << 18;
    const COMMAND: usize = 1 << 20;

    fn app_menu() -> *mut AnyObject {
        // SAFETY: on the main thread; winit's default menu bar has the app's menu first.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let bar: *mut AnyObject = msg_send![app, mainMenu];
            assert!(!bar.is_null(), "winit made a menu bar");
            let first: *mut AnyObject = msg_send![bar, itemAtIndex: 0_isize];
            msg_send![first, submenu]
        }
    }

    fn text(string: *mut AnyObject) -> String {
        // SAFETY: `string` is a live NSString; `UTF8String` points into it while it lives.
        unsafe {
            let utf8: *const std::ffi::c_char = msg_send![string, UTF8String];
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    }

    fn item(menu: *mut AnyObject, at: isize) -> *mut AnyObject {
        // SAFETY: `menu` is a live NSMenu and `at` within its items.
        unsafe { msg_send![menu, itemAtIndex: at] }
    }

    /// Each item's title, separators as "".
    fn titles(menu: *mut AnyObject) -> Vec<String> {
        // SAFETY: `menu` is a live NSMenu.
        let count: isize = unsafe { msg_send![menu, numberOfItems] };
        (0..count)
            .map(|at| {
                // SAFETY: a live NSMenuItem of `menu`, whose `title` is an NSString.
                text(unsafe { msg_send![item(menu, at), title] })
            })
            .collect()
    }

    /// The keys an item shows: its key and its modifier mask (0 with no key, whose mask
    /// shows nothing).
    fn keys(item: *mut AnyObject) -> (String, usize) {
        // SAFETY: `item` is a live NSMenuItem.
        unsafe {
            let key: *mut AnyObject = msg_send![item, keyEquivalent];
            let mask: usize = msg_send![item, keyEquivalentModifierMask];
            let key = text(key);
            let mask = if key.is_empty() { 0 } else { mask };
            (key, mask)
        }
    }

    fn ns_string(text: &str) -> Retained<AnyObject> {
        let text = CString::new(text).unwrap();
        // SAFETY: a NUL-terminated UTF-8 string, copied into an autoreleased NSString.
        let string: Option<Retained<AnyObject>> =
            unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] };
        string.unwrap()
    }

    /// Whether the menu bar takes a key press of `character` (key code `code`) held with
    /// `flags`, as it does before the window sees the key.
    fn press(character: &str, code: u16, flags: usize) -> bool {
        let characters = ns_string(character);
        // SAFETY: `keyEventWithType:...` builds an autoreleased key-down event (type 10) from
        // plain values (nil for the unused graphics context), which the menu bar's
        // `performKeyEquivalent:` reads.
        unsafe {
            let event: Option<Retained<AnyObject>> = msg_send![
                class!(NSEvent),
                keyEventWithType: 10_usize,
                location: NsPoint { x: 0.0, y: 0.0 },
                modifierFlags: flags,
                timestamp: 0.0_f64,
                windowNumber: 0_isize,
                context: std::ptr::null_mut::<AnyObject>(),
                characters: &*characters,
                charactersIgnoringModifiers: &*characters,
                isARepeat: Bool::NO,
                keyCode: code
            ];
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let bar: *mut AnyObject = msg_send![app, mainMenu];
            let taken: Bool = msg_send![bar, performKeyEquivalent: &*event.unwrap()];
            taken.as_bool()
        }
    }

    struct Test {
        wakes: Rc<Cell<usize>>,
    }

    impl Test {
        fn check(&self) -> Vec<String> {
            let mut failures = Vec::new();
            let menu = app_menu();
            let wakes = self.wakes.clone();
            menu_bar::add_menu_items("Annulus", move || wakes.set(wakes.get() + 1));
            // Twice changes nothing.
            menu_bar::add_menu_items("Annulus", || {});
            let expected = [
                "About Annulus",
                "",
                "Settings\u{2026}",
                "Lock Vault",
                "",
                "Services",
                "Hide Annulus",
                "Hide Others",
                "Show All",
                "",
                "Quit Annulus",
            ];
            let titles = titles(menu);
            if titles != expected {
                failures.push(format!("the app menu reads {titles:?}"));
                return failures;
            }
            let (about, settings, lock) = (item(menu, 0), item(menu, 2), item(menu, 3));
            menu_bar::set_menu_keys(|item| match item {
                MenuItem::About => None,
                MenuItem::Settings => Some((Key::Char(','), CMD)),
                MenuItem::Lock => Some((Key::Char('l'), CMD_CTRL)),
            });
            for (name, item, shown) in [
                ("About", about, (String::new(), 0)),
                ("Settings", settings, (",".to_owned(), COMMAND)),
                ("Lock", lock, ("l".to_owned(), COMMAND | CONTROL)),
            ] {
                if keys(item) != shown {
                    failures.push(format!("{name} shows {:?}, not {shown:?}", keys(item)));
                }
            }
            // A click on an item: the window host hears of it, and the window wakes.
            // SAFETY: `menu` is a live NSMenu and 2 is one of its items.
            unsafe {
                let _: () = msg_send![menu, performActionForItemAtIndex: 2_isize];
            }
            let chosen = menu_bar::take_chosen();
            if chosen != [MenuItem::Settings] || self.wakes.get() != 1 {
                failures.push(format!(
                    "a click on Settings… gave {chosen:?} and {} wakes",
                    self.wakes.get()
                ));
            }
            if !menu_bar::take_chosen().is_empty() {
                failures.push("a choice was handed over twice".into());
            }
            // Its keys, before the window sees them.
            let taken = press("l", 37, COMMAND | CONTROL);
            let chosen = menu_bar::take_chosen();
            if !taken || chosen != [MenuItem::Lock] {
                failures.push(format!("Cmd+Ctrl+L: taken {taken}, chose {chosen:?}"));
            }
            // Keys taken away (Settings records keys): the menu bar leaves them to the window.
            menu_bar::set_menu_keys(|_| None);
            if keys(settings) != (String::new(), 0) {
                failures.push(format!("Settings still shows {:?}", keys(settings)));
            }
            let taken = press(",", 43, COMMAND);
            let chosen = menu_bar::take_chosen();
            if taken || !chosen.is_empty() {
                failures.push(format!(
                    "Cmd+, with no keys shown: taken {taken}, {chosen:?}"
                ));
            }
            failures
        }
    }

    impl ApplicationHandler for Test {
        fn resumed(&mut self, _: &ActiveEventLoop) {
            let failures = self.check();
            for failure in &failures {
                eprintln!("FAIL: {failure}");
            }
            std::process::exit(i32::from(!failures.is_empty()));
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    pub(super) fn run() {
        let event_loop = EventLoop::builder()
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_activate_ignoring_other_apps(false)
            .with_default_menu(true)
            .build()
            .unwrap();
        let mut test = Test {
            wakes: Rc::new(Cell::new(0)),
        };
        event_loop.run_app(&mut test).unwrap();
        panic!("the event loop ended before the test did");
    }
}
