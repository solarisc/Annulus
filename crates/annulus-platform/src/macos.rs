//! The window's frame on macOS (UI-SPEC §1): macOS's own window buttons, the traffic lights,
//! at the left end of our title bar.
//!
//! The window is titled, with a transparent title bar that our content runs under, so macOS
//! draws the buttons, the rounded corners and the shadow, and resizes the window from its
//! edges. A borderless window has none of that, and winit answers `is_maximized()` on one by
//! switching its style mask to titled and back, which loses the mouse button's release when
//! the button is down at the time: no click ever landed. Two things are then set by hand: the
//! clicks in the title bar's band reach our view (our tabs are there; the app drags the window
//! from the bar's empty space itself), and the buttons sit centered in our title bar, which is
//! taller than macOS's.

#![allow(unsafe_code)]

use std::ffi::CString;

use objc2::encode::{Encode, Encoding};
use objc2::runtime::{AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};
use winit::platform::macos::WindowAttributesExtMacOS as _;
use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

use crate::window::TRAFFIC_LIGHTS_LEFT;

/// `NSWindowStyleMaskFullScreen`.
const FULL_SCREEN: usize = 1 << 14;

/// `NSWindowCloseButton`, `NSWindowMiniaturizeButton` and `NSWindowZoomButton`, left to right.
const BUTTONS: [usize; 3] = [0, 1, 2];

/// A point (`NSPoint`), in points, from the bottom left as macOS counts.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NsPoint {
    /// From the left.
    pub x: f64,
    /// From the bottom.
    pub y: f64,
}

/// A size (`NSSize`), in points.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NsSize {
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// A rectangle (`NSRect`), in points.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NsRect {
    /// Its bottom left corner.
    pub origin: NsPoint,
    /// Its size.
    pub size: NsSize,
}

impl NsRect {
    fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: NsPoint { x, y },
            size: NsSize { width, height },
        }
    }
}

// SAFETY: on 64-bit macOS `NSPoint` is `CGPoint`, two doubles, laid out as above.
unsafe impl Encode for NsPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

// SAFETY: on 64-bit macOS `NSSize` is `CGSize`, two doubles, laid out as above.
unsafe impl Encode for NsSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

// SAFETY: on 64-bit macOS `NSRect` is `CGRect`, a `CGPoint` then a `CGSize`, as above.
unsafe impl Encode for NsRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[NsPoint::ENCODING, NsSize::ENCODING]);
}

/// `attributes` with the frame described above.
#[must_use]
pub fn frame(attributes: WindowAttributes) -> WindowAttributes {
    attributes
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_fullsize_content_view(true)
}

/// The window's `NSView`, which our frames are drawn into.
#[must_use]
pub fn ns_view(window: &Window) -> Option<*mut AnyObject> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::AppKit(handle) => Some(handle.ns_view.as_ptr().cast()),
        _ => None,
    }
}

/// Lets our view take the clicks in the title bar's band. macOS drags the window by any view
/// there that does not refuse to (`mouseDownCanMoveWindow`), and winit's view does not.
pub fn keep_title_bar_clicks(window: &Window) {
    extern "C-unwind" fn refuse(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }
    let Some(view) = ns_view(window) else { return };
    let Ok(types) = CString::new(format!("{}@:", Bool::ENCODING)) else {
        return;
    };
    // SAFETY: `view` is the live NSView of `window`. `refuse` has the C signature of an
    // Objective-C method with no arguments that returns BOOL, which `types` spells; `Imp` is
    // the type-erased form the runtime stores, and it calls `refuse` back with that same
    // signature. The method is added to the view's own class (winit's view, or AccessKit's
    // subclass of it), so no other view of the process changes.
    unsafe {
        let class = (*view).class();
        let imp = std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel) -> Bool, Imp>(refuse);
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            sel!(mouseDownCanMoveWindow),
            imp,
            types.as_ptr(),
        );
    }
}

/// Centers the window buttons in the window's top `band` points, the first
/// [`TRAFFIC_LIGHTS_LEFT`] from the left edge and the others as far apart as macOS spaces
/// them. Not in full screen, where macOS shows its own title bar over the window. Moves
/// nothing already in place, so it runs every frame: macOS lays the buttons out again its
/// own way when the window changes size.
pub fn place_buttons(window: &Window, band: f64) {
    let Some(view) = ns_view(window) else { return };
    let left = f64::from(TRAFFIC_LIGHTS_LEFT);
    // SAFETY: `view` is the live NSView of `window`. Each message is a public AppKit method,
    // sent with its argument and return types as declared there, and each object returned is
    // checked for nil before a message goes to it.
    unsafe {
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let mask: usize = msg_send![ns_window, styleMask];
        if mask & FULL_SCREEN != 0 {
            return;
        }
        let close: *mut AnyObject = msg_send![ns_window, standardWindowButton: BUTTONS[0]];
        let bar = superview(close);
        let container = superview(bar);
        let frame_view = superview(container);
        if frame_view.is_null() {
            return;
        }
        let whole: NsRect = msg_send![frame_view, bounds];
        let width = whole.size.width;
        set_frame(
            container,
            NsRect::new(0.0, whole.size.height - band, width, band),
        );
        set_frame(bar, NsRect::new(0.0, 0.0, width, band));
        // All read before any moves, so the gaps between them stay macOS's.
        let mut placed = Vec::with_capacity(BUTTONS.len());
        for kind in BUTTONS {
            let button: *mut AnyObject = msg_send![ns_window, standardWindowButton: kind];
            if !button.is_null() {
                let at: NsRect = msg_send![button, frame];
                placed.push((button, at));
            }
        }
        let Some(first) = placed.first().map(|(_, at)| at.origin.x) else {
            return;
        };
        for (button, at) in placed {
            let origin = NsPoint {
                x: left + at.origin.x - first,
                y: ((band - at.size.height) / 2.0).round(),
            };
            if at.origin != origin {
                let _: () = msg_send![button, setFrameOrigin: origin];
            }
        }
    }
}

/// The view holding `view`, or nil.
///
/// # Safety
/// `view` is nil or a live `NSView`.
unsafe fn superview(view: *mut AnyObject) -> *mut AnyObject {
    if view.is_null() {
        return view;
    }
    // SAFETY: `view` is a live NSView, as the caller promises; `superview` returns one or nil.
    unsafe { msg_send![view, superview] }
}

/// Gives `view` the frame `frame` unless it has it already.
///
/// # Safety
/// `view` is nil or a live `NSView`.
unsafe fn set_frame(view: *mut AnyObject, frame: NsRect) {
    if view.is_null() {
        return;
    }
    // SAFETY: `view` is a live NSView, as the caller promises; `frame` and `setFrame:` take
    // and give an NSRect.
    unsafe {
        let now: NsRect = msg_send![view, frame];
        if now != frame {
            let _: () = msg_send![view, setFrame: frame];
        }
    }
}
