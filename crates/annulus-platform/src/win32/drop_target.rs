//! Files dragged in from Explorer, as they move over the window and where they land: an OLE
//! drop target. `WM_DROPFILES` says only where files were dropped; this says where they are on
//! the way, so the app can show where they would go before they are let go.

// What `#[implement]` writes for a COM object.
#![allow(clippy::ref_as_ptr, clippy::inline_always)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use annulus_ui::gfx::Point;
use annulus_ui::kit::Event;
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows_core::{Ref, implement};

/// The window's drop target: it hands what it hears to the window's thread as events and
/// asks for a frame.
#[implement(IDropTarget)]
pub(super) struct DropTarget {
    hwnd: isize,
    events: Sender<Event>,
    /// The files of the drag over the window now.
    files: RefCell<Vec<PathBuf>>,
}

impl DropTarget {
    /// A target for window `hwnd` that sends its news to `events`.
    pub(super) fn target(hwnd: HWND, events: Sender<Event>) -> IDropTarget {
        DropTarget {
            hwnd: hwnd.0 as isize,
            events,
            files: RefCell::new(Vec::new()),
        }
        .into()
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// `at` on the screen, in the window's DIPs.
    fn place(&self, at: &POINTL) -> Point {
        let mut point = POINT { x: at.x, y: at.y };
        // SAFETY: our own window; `point` is a local the call writes back into.
        let dpi = unsafe {
            let _ = ScreenToClient(self.hwnd(), &raw mut point);
            GetDpiForWindow(self.hwnd())
        };
        #[allow(clippy::cast_precision_loss)] // DPI values and window coordinates are small.
        let scale = dpi.max(96) as f32 / 96.0;
        #[allow(clippy::cast_precision_loss)] // As above.
        Point::new(point.x as f32 / scale, point.y as f32 / scale)
    }

    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            // SAFETY: our own window; a stale handle is rejected by Windows, not used.
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd()), None, false);
            }
        }
    }

    /// Copy, when files are dragged; nothing else is taken.
    fn effect(&self, effect: *mut DROPEFFECT) {
        let wanted = if self.files.borrow().is_empty() {
            DROPEFFECT_NONE
        } else {
            DROPEFFECT_COPY
        };
        if !effect.is_null() {
            // SAFETY: OLE passes a valid pointer to the effect it reads back.
            unsafe { *effect = wanted };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let files = data.ok().map(files_of).unwrap_or_default();
        self.files.replace(files.clone());
        self.effect(effect);
        if !files.is_empty() {
            self.send(Event::FilesHovered(files, Some(self.place(at))));
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        self.effect(effect);
        let files = self.files.borrow().clone();
        if !files.is_empty() {
            self.send(Event::FilesHovered(files, Some(self.place(at))));
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows_core::Result<()> {
        self.files.replace(Vec::new());
        self.send(Event::FilesHoverLeft);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let files = data.ok().map(files_of).unwrap_or_default();
        self.files.replace(files.clone());
        self.effect(effect);
        self.files.replace(Vec::new());
        if files.is_empty() {
            self.send(Event::FilesHoverLeft);
        } else {
            let place = self.place(at);
            self.send(Event::PointerMoved(place));
            self.send(Event::FilesDropped(files, Some(place)));
        }
        Ok(())
    }
}

/// The files a drag carries (Explorer's `CF_HDROP`); none for text or anything else.
fn files_of(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: u32::try_from(TYMED_HGLOBAL.0).unwrap_or(1),
    };
    // SAFETY: GetData fills a medium we release below; its global memory is locked only
    // while the list is read from it, and the HDROP is that memory.
    unsafe {
        let Ok(mut medium) = data.GetData(&raw const format) else {
            return Vec::new();
        };
        let memory = medium.u.hGlobal;
        let locked = GlobalLock(memory);
        let files = if locked.is_null() {
            Vec::new()
        } else {
            let files = paths_of(HDROP(locked));
            let _ = GlobalUnlock(memory);
            files
        };
        ReleaseStgMedium(&raw mut medium);
        files
    }
}

/// The paths in a drop handle.
pub(super) fn paths_of(drop: HDROP) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // SAFETY: `drop` is a valid drop handle for the whole call; each name is read into a
    // buffer one longer than the length Windows reported for it.
    unsafe {
        for index in 0..DragQueryFileW(drop, u32::MAX, None) {
            let length = DragQueryFileW(drop, index, None) as usize;
            let mut name = vec![0_u16; length + 1];
            let written = DragQueryFileW(drop, index, Some(&mut name)) as usize;
            paths.push(PathBuf::from(
                <std::ffi::OsString as std::os::windows::ffi::OsStringExt>::from_wide(
                    &name[..written.min(length)],
                ),
            ));
        }
    }
    paths
}
