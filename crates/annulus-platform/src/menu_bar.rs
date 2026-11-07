//! The app's menu in the macOS menu bar (UI-SPEC §1): winit's default one (About, Services,
//! Hide, Quit) with our own items in it, About, Settings… and Lock Vault ([`MenuItem`]).
//!
//! Our items send their choice to one target object of ours, which keeps it for the window
//! host and wakes the window; the host hands it to the app before the next frame. The keys
//! shown beside an item come from the app after each frame ([`set_menu_keys`]), so they are
//! the shortcut table's, keys chosen in Settings included. The menu bar answers to those keys
//! before the window sees them, so the app leaves them out while Settings records new keys.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;

use annulus_ui::kit::{Key, Modifiers};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{AnyThread as _, class, define_class, msg_send, sel};

use crate::window::MenuItem;

define_class!(
    // SAFETY: NSObject has no rules for subclasses, and `MenuTarget` implements no `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "AnnulusMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        // The action of each of our items; the item's tag is its place in `MenuItem::ALL`.
        #[unsafe(method(choose:))]
        fn choose(&self, sender: &AnyObject) {
            // SAFETY: the sender of `choose:` is one of our NSMenuItems, and `tag` returns an
            // NSInteger.
            let tag: isize = unsafe { msg_send![sender, tag] };
            chosen(tag);
        }
    }
);

impl MenuTarget {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's `init`, on an object just allocated.
        unsafe { msg_send![super(this), init] }
    }
}

/// Our items in the menu bar, and the choices the window host has not taken yet.
struct Menu {
    /// The items' target; menu items hold theirs weakly.
    _target: Retained<MenuTarget>,
    /// Our `NSMenuItem`s, in [`MenuItem::ALL`]'s order.
    items: Vec<Retained<AnyObject>>,
    /// The keys each item shows now.
    keys: Vec<Option<(Key, Modifiers)>>,
    chosen: Vec<MenuItem>,
    wake: Rc<dyn Fn()>,
}

thread_local! {
    /// Menus live on the main thread, and so does this.
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

/// `NSEventModifierFlagShift`, `…Control`, `…Option` and `…Command`.
const SHIFT: usize = 1 << 17;
const CONTROL: usize = 1 << 18;
const OPTION: usize = 1 << 19;
const COMMAND: usize = 1 << 20;

/// An item's title in the menu, for the app called `app`.
#[must_use]
pub fn title(item: MenuItem, app: &str) -> String {
    match item {
        MenuItem::About => format!("About {app}"),
        MenuItem::Settings => "Settings\u{2026}".to_owned(),
        MenuItem::Lock => "Lock Vault".to_owned(),
    }
}

/// Puts our items into the app's menu, once: About in place of winit's (which opens the
/// standard About panel), then Settings… and Lock Vault in a group of their own. Hide and
/// Quit are named after `app` rather than the program's file. `wake` asks the window for a
/// frame, after a choice. Does nothing without winit's default menu.
pub fn add_menu_items(app: &str, wake: impl Fn() + 'static) {
    if MENU.with(|menu| menu.borrow().is_some()) {
        return;
    }
    // SAFETY: on the main thread, where AppKit's objects live: each message is a public
    // AppKit method sent with its argument and return types as declared there, and each
    // object returned is checked for nil before a message goes to it.
    let added = unsafe {
        let app_menu = app_menu();
        if app_menu.is_null() {
            return;
        }
        let target = MenuTarget::new();
        let mut items = Vec::with_capacity(MenuItem::ALL.len());
        for (tag, item) in (0_isize..).zip(MenuItem::ALL) {
            let Some(made) = new_item(&title(item, app), &target, tag) else {
                return;
            };
            items.push(made);
        }
        // About takes the place of winit's, or the top.
        let about = find(app_menu, sel!(orderFrontStandardAboutPanel:));
        if let Some(at) = about {
            let _: () = msg_send![app_menu, removeItemAtIndex: at];
        }
        let at = about.unwrap_or(0);
        let _: () = msg_send![app_menu, insertItem: &*items[0], atIndex: at];
        let Some(separator) = separator() else { return };
        let _: () = msg_send![app_menu, insertItem: &*separator, atIndex: at + 1];
        for (offset, item) in (2_isize..).zip(&items[1..]) {
            let _: () = msg_send![app_menu, insertItem: &**item, atIndex: at + offset];
        }
        for (action, verb) in [(sel!(hide:), "Hide"), (sel!(terminate:), "Quit")] {
            if let Some(at) = find(app_menu, action)
                && let Some(named) = ns_string(&format!("{verb} {app}"))
            {
                let item: *mut AnyObject = msg_send![app_menu, itemAtIndex: at];
                let _: () = msg_send![item, setTitle: &*named];
            }
        }
        Menu {
            _target: target,
            keys: vec![None; items.len()],
            items,
            chosen: Vec::new(),
            wake: Rc::new(wake),
        }
    };
    MENU.with(|menu| *menu.borrow_mut() = Some(added));
}

/// The items chosen since the last call, oldest first.
#[must_use]
pub fn take_chosen() -> Vec<MenuItem> {
    MENU.with(|menu| {
        menu.borrow_mut()
            .as_mut()
            .map(|menu| std::mem::take(&mut menu.chosen))
            .unwrap_or_default()
    })
}

/// Shows `keys(item)` beside each of our items, which then answer to them; `None` shows
/// none. Touches only the items whose keys changed, so it runs after every frame.
pub fn set_menu_keys(keys: impl Fn(MenuItem) -> Option<(Key, Modifiers)>) {
    MENU.with(|menu| {
        let mut menu = menu.borrow_mut();
        let Some(Menu {
            items, keys: shown, ..
        }) = menu.as_mut()
        else {
            return;
        };
        for ((item, ns_item), shown) in MenuItem::ALL.into_iter().zip(items).zip(shown) {
            let wanted =
                keys(item).filter(|&(key, modifiers)| equivalent(key, modifiers).is_some());
            if *shown == wanted {
                continue;
            }
            *shown = wanted;
            let (text, mask) = wanted
                .and_then(|(key, modifiers)| equivalent(key, modifiers))
                .unwrap_or_default();
            // SAFETY: `ns_item` is one of our live NSMenuItems, on the main thread;
            // `setKeyEquivalent:` takes an NSString and `setKeyEquivalentModifierMask:` an
            // NSUInteger.
            unsafe {
                if let Some(text) = ns_string(&text) {
                    let _: () = msg_send![&**ns_item, setKeyEquivalent: &*text];
                }
                let _: () = msg_send![&**ns_item, setKeyEquivalentModifierMask: mask];
            }
        }
    });
}

/// `key` with `modifiers` as `AppKit` spells a menu item's keys: the key's character (arrows
/// and function keys have their own in Unicode's private area) and a mask of the modifiers.
/// `None` for a key no menu item can show.
fn equivalent(key: Key, modifiers: Modifiers) -> Option<(String, usize)> {
    let character = match key {
        Key::Char(c) => c,
        Key::F(n @ 1..=35) => char::from_u32(0xF704 + u32::from(n) - 1)?,
        Key::F(_) => return None,
        Key::Up => '\u{F700}',
        Key::Down => '\u{F701}',
        Key::Left => '\u{F702}',
        Key::Right => '\u{F703}',
        Key::Delete => '\u{F728}',
        Key::Home => '\u{F729}',
        Key::End => '\u{F72B}',
        Key::PageUp => '\u{F72C}',
        Key::PageDown => '\u{F72D}',
        Key::Backspace => '\u{8}',
        Key::Tab => '\t',
        Key::Enter => '\r',
        Key::Escape => '\u{1b}',
        Key::Space => ' ',
    };
    let mask = [
        (modifiers.shift, SHIFT),
        (modifiers.ctrl, CONTROL),
        (modifiers.alt, OPTION),
        (modifiers.logo, COMMAND),
    ]
    .into_iter()
    .filter_map(|(held, bit)| held.then_some(bit))
    .sum();
    Some((character.to_string(), mask))
}

/// Keeps a choice for the window host and wakes the window.
fn chosen(tag: isize) {
    let wake = MENU.with(|menu| {
        let mut menu = menu.borrow_mut();
        let menu = menu.as_mut()?;
        let item = usize::try_from(tag)
            .ok()
            .and_then(|at| MenuItem::ALL.get(at))?;
        menu.chosen.push(*item);
        Some(menu.wake.clone())
    });
    // Outside the borrow: waking may run more of the app's code.
    if let Some(wake) = wake {
        wake();
    }
}

/// The app's menu (the menu bar's first item's), or nil.
///
/// # Safety
/// On the main thread.
unsafe fn app_menu() -> *mut AnyObject {
    // SAFETY: on the main thread, as the caller promises; each object is checked for nil
    // before a message goes to it.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let bar: *mut AnyObject = msg_send![app, mainMenu];
        if bar.is_null() {
            return bar;
        }
        let count: isize = msg_send![bar, numberOfItems];
        if count == 0 {
            return std::ptr::null_mut();
        }
        let first: *mut AnyObject = msg_send![bar, itemAtIndex: 0_isize];
        msg_send![first, submenu]
    }
}

/// Where the item of `menu` with `action` is.
///
/// # Safety
/// `menu` is a live `NSMenu`, on the main thread.
unsafe fn find(menu: *mut AnyObject, action: Sel) -> Option<isize> {
    // SAFETY: `menu` is a live NSMenu, as the caller promises; `itemAtIndex:` gives its items
    // within `numberOfItems`, and `action` an item's selector or none.
    unsafe {
        let count: isize = msg_send![menu, numberOfItems];
        (0..count).find(|&at| {
            let item: *mut AnyObject = msg_send![menu, itemAtIndex: at];
            let has: Option<Sel> = msg_send![item, action];
            has == Some(action)
        })
    }
}

/// A menu item titled `title` that sends `choose:` with `tag` to `target`.
///
/// # Safety
/// On the main thread.
unsafe fn new_item(title: &str, target: &MenuTarget, tag: isize) -> Option<Retained<AnyObject>> {
    // SAFETY: on the main thread, as the caller promises. `new` gives an NSMenuItem, and its
    // setters take an NSString, an object (held weakly: `Menu` keeps it), a selector and an
    // NSInteger.
    unsafe {
        let item: Option<Retained<AnyObject>> = msg_send![class!(NSMenuItem), new];
        let item = item?;
        let title = ns_string(title)?;
        let _: () = msg_send![&*item, setTitle: &*title];
        let _: () = msg_send![&*item, setTarget: target];
        let _: () = msg_send![&*item, setAction: sel!(choose:)];
        let _: () = msg_send![&*item, setTag: tag];
        Some(item)
    }
}

/// A separator line for a menu.
///
/// # Safety
/// On the main thread.
unsafe fn separator() -> Option<Retained<AnyObject>> {
    // SAFETY: on the main thread, as the caller promises; `separatorItem` gives an
    // autoreleased NSMenuItem, which this retains.
    unsafe { msg_send![class!(NSMenuItem), separatorItem] }
}

/// `text` as an `NSString`; `None` if it holds a NUL.
fn ns_string(text: &str) -> Option<Retained<AnyObject>> {
    let text = CString::new(text).ok()?;
    // SAFETY: `stringWithUTF8String:` copies a NUL-terminated UTF-8 string, which `text` is,
    // into an autoreleased NSString, which this retains.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_item_shows_a_chords_key_and_its_modifiers_as_appkit_spells_them() {
        let cmd = Modifiers {
            logo: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            equivalent(Key::Char(','), cmd),
            Some((",".to_owned(), COMMAND))
        );
        let cmd_ctrl = Modifiers { ctrl: true, ..cmd };
        assert_eq!(
            equivalent(Key::Char('l'), cmd_ctrl),
            Some(("l".to_owned(), COMMAND | CONTROL))
        );
        let all = Modifiers {
            ctrl: true,
            alt: true,
            shift: true,
            logo: true,
        };
        assert_eq!(
            equivalent(Key::F(5), all),
            Some(("\u{F708}".to_owned(), SHIFT | CONTROL | OPTION | COMMAND))
        );
        assert_eq!(
            equivalent(Key::Left, cmd),
            Some(("\u{F702}".to_owned(), COMMAND))
        );
        assert_eq!(equivalent(Key::F(40), cmd), None);
    }
}
