//! The Windows window ([ADR-0017] decision 4): one Win32 window whose client area covers the
//! caption, so the title bar is ours to draw, while Windows keeps the borders, resizing,
//! Aero Snap, the shadow and (on Windows 11) Snap Layouts over our maximize button. Frames
//! are drawn by Direct2D with `ClearType` text, only when input arrives or the app asks
//! (P-06). This settles it: our own Win32 window, not winit, on Windows.
//!
//! Screen readers reach the UI through UI Automation: AccessKit's adapter answers
//! `WM_GETOBJECT` with the tree of the frame's zones (`annulus_ui::access`).
//!
//! [ADR-0017]: ../../../docs/adr/0017-native-ui.md

#![allow(unsafe_code)]
// The bindings take raw pointers to locals that outlive each call.
#![allow(clippy::borrow_as_ptr)]

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use accesskit_windows::Adapter;
use annulus_ui::access::accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use annulus_ui::access::{self, Request};
use annulus_ui::gfx::{Point, Rect};
use annulus_ui::kit::{self, Button, Cursor, Event, Modifiers, Ui, UiState};
use windows::Win32::Foundation::{
    D2DERR_RECREATE_TARGET, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, ID2D1HwndRenderTarget, ID2D1RenderTarget,
};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
    TrackMouseEvent, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
    VK_F1, VK_F24, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, HDROP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_DBLCLKS, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW, HTCAPTION,
    HTCLIENT, HTMAXBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, IDC_HAND, IDC_IBEAM,
    IDC_SIZENS, IDC_SIZEWE, IsZoomed, KillTimer, LoadCursorW, MINMAXINFO, MSG, NCCALCSIZE_PARAMS,
    PostQuitMessage, RegisterClassExW, SM_CXPADDEDBORDER, SM_CYSIZEFRAME, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetCursor,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WM_CHAR, WM_CREATE, WM_DESTROY, WM_DPICHANGED, WM_DROPFILES, WM_ERASEBKGND, WM_GETMINMAXINFO,
    WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCACTIVATE, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
    WM_NCMOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE,
    WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::d2d::{D2d, D2dCanvas};

/// The timer that brings a frame the app asked for later.
const REPAINT_TIMER: usize = 1;

/// `WM_MOUSELEAVE`, which the bindings file under `UI::Controls`.
const WM_MOUSELEAVE: u32 = 0x02A3;
use crate::window::{
    WindowApp, WindowError, WindowRequest, is_paste, paste_event, publish_clipboard,
};

/// Hands a screen reader's requests to the window's thread and wakes it. UI Automation calls
/// it on threads of its own.
struct Requests {
    sender: Sender<ActionRequest>,
    hwnd: isize,
}

impl ActionHandler for Requests {
    fn do_action(&mut self, request: ActionRequest) {
        if self.sender.send(request).is_ok() {
            // SAFETY: InvalidateRect may be called from any thread; a stale handle after
            // the window closed is rejected by Windows, not used.
            unsafe {
                let _ =
                    InvalidateRect(Some(HWND(self.hwnd as *mut core::ffi::c_void)), None, false);
            }
        }
    }
}

/// The tree a screen reader gets when it first asks (`WM_GETOBJECT`).
struct Initial(Option<TreeUpdate>);

impl ActivationHandler for Initial {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.take()
    }
}

/// Runs `app` in a new window until it closes.
///
/// # Errors
/// When the window or Direct2D cannot be created.
pub fn run(title: &str, size: (f32, f32), app: Box<dyn WindowApp>) -> Result<(), WindowError> {
    let d2d = D2d::new().map_err(|e| WindowError::Platform(e.to_string()))?;
    let (sender, requests) = channel();
    let mut host = Box::new(Host {
        title: title.to_owned(),
        access: None,
        requests,
        hwnd: HWND::default(),
        app,
        d2d,
        target: None,
        state: UiState::default(),
        events: Vec::new(),
        scale: 1.0,
        size: (0.0, 0.0),
        started: Instant::now(),
        tracking: false,
        painting: false,
        cursor: Cursor::Default,
    });
    // SAFETY: standard Win32 window setup on this thread. The host is boxed so its address
    // is stable; the window procedure reaches it through GWLP_USERDATA only while the box
    // lives, which is until after the message loop ends.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).map_err(|e| WindowError::Platform(e.to_string()))?;
        let class = w!("AnnulusWindow");
        let wc = WNDCLASSEXW {
            cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(0),
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..WNDCLASSEXW::default()
        };
        RegisterClassExW(&wc);
        let title = HSTRING::from(title);
        let host_ptr: *mut Host = &mut *host;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance.into()),
            Some(host_ptr.cast()),
        )
        .map_err(|e| WindowError::Platform(e.to_string()))?;
        // The size in DIPs at the window's DPI.
        #[allow(clippy::cast_precision_loss)] // DPI values are small.
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        host.scale = scale;
        #[allow(clippy::cast_possible_truncation)] // Window sizes fit in i32.
        let (w, h) = ((size.0 * scale) as i32, (size.1 * scale) as i32);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            w,
            h,
            SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        // The waker may run on any thread: InvalidateRect is safe to call from one.
        let handle = hwnd.0 as isize;
        host.app.set_waker(std::sync::Arc::new(move || {
            // A stale handle after the window closed is rejected by Windows, not used.
            let _ = InvalidateRect(Some(HWND(handle as *mut core::ffi::c_void)), None, false);
        }));
        // Before the window shows: UI Automation must be ready before `WM_GETOBJECT`.
        host.access = Some(Adapter::new(
            hwnd,
            false,
            Requests {
                sender,
                hwnd: handle,
            },
        ));
        let dark = host.app.theme().dark;
        let value = i32::from(dark);
        // DWMWA_USE_IMMERSIVE_DARK_MODE: dark borders and system menu.
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), (&raw const value).cast(), 4);
        // Files dropped from Explorer arrive as WM_DROPFILES.
        DragAcceptFiles(hwnd, true);
        // DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND: rounded corners on Windows 11 (Windows
        // 10 has no such attribute and ignores it).
        let round = 2_i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), (&raw const round).cast(), 4);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(host);
    Ok(())
}

struct Host {
    title: String,
    /// The screen-reader bridge; it builds a tree only once one asks.
    access: Option<Adapter>,
    /// Screen-reader requests from `Requests`.
    requests: Receiver<ActionRequest>,
    hwnd: HWND,
    app: Box<dyn WindowApp>,
    d2d: D2d,
    target: Option<ID2D1HwndRenderTarget>,
    state: UiState,
    events: Vec<Event>,
    /// Device pixels per DIP.
    scale: f32,
    /// Client size in DIPs.
    size: (f32, f32),
    started: Instant,
    /// Asked Windows for `WM_MOUSELEAVE`.
    tracking: bool,
    /// Drawing a frame; messages sent during it do not draw again.
    painting: bool,
    cursor: Cursor,
}

extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: on WM_CREATE the create parameter is the boxed `Host`, alive until the message
    // loop in `run` ends; afterwards the pointer is read back from GWLP_USERDATA. Messages
    // are handled on the window's own thread only.
    unsafe {
        if message == WM_CREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let host = create.lpCreateParams.cast::<Host>();
            (*host).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, host as isize);
            return LRESULT(0);
        }
        let host = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
        if host.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        if message == WM_GETOBJECT {
            // Turning the answer into an LRESULT can send a nested WM_GETOBJECT, so it
            // happens after the host is no longer borrowed.
            return match (*host).get_object(wparam, lparam) {
                Some(answer) => answer.into(),
                None => DefWindowProcW(hwnd, message, wparam, lparam),
            };
        }
        (*host).handle(message, wparam, lparam)
    }
}

/// A hit-test answer as a window procedure result.
fn hit(code: u32) -> LRESULT {
    LRESULT(isize::try_from(code).unwrap_or_default())
}

/// A `WPARAM` as the signed word the 16-bit halves are read from.
#[allow(clippy::cast_possible_wrap)] // Reinterpreting the bits is the point.
fn signed(value: usize) -> isize {
    value as isize
}

/// The low and high 16 bits of a message parameter, sign-extended (mouse coordinates).
#[allow(clippy::cast_possible_truncation)] // Taking the 16-bit halves is the point.
fn low(value: isize) -> i32 {
    i32::from(value as i16)
}

#[allow(clippy::cast_possible_truncation)] // Taking the 16-bit halves is the point.
fn high(value: isize) -> i32 {
    i32::from((value >> 16) as i16)
}

impl Host {
    /// The screen reader's `WM_GETOBJECT`: the tree of the frame on screen.
    fn get_object(&mut self, wparam: WPARAM, lparam: LPARAM) -> Option<impl Into<LRESULT> + use<>> {
        let tree = access::tree(
            self.state.zones(),
            self.state.focus(),
            self.scale,
            &self.title,
        );
        self.access
            .as_mut()?
            .handle_wm_getobject(wparam, lparam, &mut Initial(Some(tree)))
    }

    /// Tells the screen reader whether the window has keyboard focus.
    fn window_focus(&mut self, focused: bool) {
        if let Some(events) = self
            .access
            .as_mut()
            .and_then(|access| access.update_window_focus_state(focused))
        {
            events.raise();
        }
    }

    fn point(&self, lparam: LPARAM) -> Point {
        #[allow(clippy::cast_precision_loss)] // Window coordinates are small.
        Point::new(
            low(lparam.0) as f32 / self.scale,
            high(lparam.0) as f32 / self.scale,
        )
    }

    fn invalidate(&self) {
        // SAFETY: our own window.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn push(&mut self, event: Event) {
        self.events.push(event);
        self.invalidate();
    }

    fn maximized(&self) -> bool {
        // SAFETY: our own window.
        unsafe { IsZoomed(self.hwnd).as_bool() }
    }

    fn frame_thickness(&self) -> i32 {
        // SAFETY: plain metric queries.
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd);
            GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
                + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
        }
    }

    /// The id of the top-most zone under `point` in the frame on screen.
    fn zone_at(&self, point: Point) -> Option<kit::Id> {
        self.state
            .zones()
            .iter()
            .rev()
            .find(|zone| zone.rect.contains(point))
            .map(|zone| zone.id)
    }

    fn modifiers() -> Modifiers {
        let down = |key: VIRTUAL_KEY| {
            // SAFETY: GetKeyState only reads this thread's keyboard state.
            let state = unsafe { GetKeyState(i32::from(key.0)) };
            state < 0
        };
        Modifiers {
            ctrl: down(VK_CONTROL),
            shift: down(VK_SHIFT),
            alt: down(VK_MENU),
            logo: down(VK_LWIN) || down(VK_RWIN),
        }
    }

    #[allow(clippy::too_many_lines)] // One match over the window's messages.
    fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hwnd = self.hwnd;
        // SAFETY: called from `wndproc` on the window's thread with the message's own
        // parameters; pointer parameters are those the message documents.
        unsafe {
            match message {
                WM_NCCALCSIZE if wparam.0 != 0 => {
                    // Standard left, right and bottom frame; no caption: the client area
                    // starts at the very top.
                    let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                    let top = params.rgrc[0].top;
                    let _ = DefWindowProcW(hwnd, message, wparam, lparam);
                    params.rgrc[0].top = top;
                    if self.maximized() {
                        // A maximized window hangs its frame off-screen: keep ours on it.
                        params.rgrc[0].top += self.frame_thickness();
                    }
                    LRESULT(0)
                }
                WM_NCHITTEST => {
                    let default = DefWindowProcW(hwnd, message, wparam, lparam);
                    if default != hit(HTCLIENT) {
                        return default;
                    }
                    let mut screen = POINT {
                        x: low(lparam.0),
                        y: high(lparam.0),
                    };
                    let _ = ScreenToClient(hwnd, &raw mut screen);
                    #[allow(clippy::cast_precision_loss)] // Window coordinates are small.
                    let point =
                        Point::new(screen.x as f32 / self.scale, screen.y as f32 / self.scale);
                    if !self.maximized() {
                        #[allow(clippy::cast_precision_loss)] // A few pixels.
                        let edge = (self.frame_thickness() as f32 / self.scale).min(6.0);
                        if point.y < edge {
                            let code = if point.x < 12.0 {
                                HTTOPLEFT
                            } else if point.x > self.size.0 - 12.0 {
                                HTTOPRIGHT
                            } else {
                                HTTOP
                            };
                            return hit(code);
                        }
                    }
                    let zone = self.zone_at(point);
                    if zone == Some(self.app.maximize_zone()) {
                        return hit(HTMAXBUTTON);
                    }
                    if zone == Some(self.app.drag_zone()) {
                        return hit(HTCAPTION);
                    }
                    hit(HTCLIENT)
                }
                WM_NCACTIVATE => {
                    self.invalidate();
                    // -1: Windows must not paint a classic caption over ours.
                    DefWindowProcW(hwnd, message, wparam, LPARAM(-1))
                }
                // Over the maximize button Windows sends non-client messages (that is what
                // makes Snap Layouts appear); hand them to the app as ordinary pointer input.
                WM_NCMOUSEMOVE | WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCLBUTTONUP
                    if u32::try_from(wparam.0) == Ok(HTMAXBUTTON) =>
                {
                    let mut screen = POINT {
                        x: low(lparam.0),
                        y: high(lparam.0),
                    };
                    let _ = ScreenToClient(hwnd, &raw mut screen);
                    #[allow(clippy::cast_precision_loss)] // Window coordinates are small.
                    let point =
                        Point::new(screen.x as f32 / self.scale, screen.y as f32 / self.scale);
                    self.push(Event::PointerMoved(point));
                    match message {
                        WM_NCMOUSEMOVE => {
                            let mut track = TRACKMOUSEEVENT {
                                cbSize: u32::try_from(std::mem::size_of::<TRACKMOUSEEVENT>())
                                    .unwrap_or(0),
                                dwFlags: TME_LEAVE | TME_NONCLIENT,
                                hwndTrack: hwnd,
                                dwHoverTime: 0,
                            };
                            let _ = TrackMouseEvent(&raw mut track);
                            DefWindowProcW(hwnd, message, wparam, lparam)
                        }
                        WM_NCLBUTTONUP => {
                            self.push(Event::PointerUp(Button::Primary));
                            LRESULT(0)
                        }
                        _ => {
                            self.push(Event::PointerDown(Button::Primary));
                            LRESULT(0)
                        }
                    }
                }
                WM_NCMOUSELEAVE | WM_MOUSELEAVE => {
                    self.tracking = false;
                    self.push(Event::PointerLeft);
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    if !self.tracking {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: u32::try_from(std::mem::size_of::<TRACKMOUSEEVENT>())
                                .unwrap_or(0),
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&raw mut track);
                        self.tracking = true;
                    }
                    let point = self.point(lparam);
                    self.push(Event::PointerMoved(point));
                    LRESULT(0)
                }
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                    let point = self.point(lparam);
                    // The release reaches this window even outside it, so a press never stays
                    // down (a drag let go elsewhere does not drop later by surprise).
                    SetCapture(hwnd);
                    self.events.push(Event::Modifiers(Self::modifiers()));
                    self.events.push(Event::PointerMoved(point));
                    self.push(Event::PointerDown(Button::Primary));
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let _ = ReleaseCapture();
                    let point = self.point(lparam);
                    self.events.push(Event::PointerMoved(point));
                    self.push(Event::PointerUp(Button::Primary));
                    LRESULT(0)
                }
                WM_RBUTTONDOWN => {
                    self.push(Event::PointerDown(Button::Secondary));
                    LRESULT(0)
                }
                WM_RBUTTONUP => {
                    self.push(Event::PointerUp(Button::Secondary));
                    LRESULT(0)
                }
                WM_MBUTTONDOWN => {
                    self.push(Event::PointerDown(Button::Middle));
                    LRESULT(0)
                }
                WM_MBUTTONUP => {
                    self.push(Event::PointerUp(Button::Middle));
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    #[allow(clippy::cast_precision_loss)] // Wheel deltas are small.
                    let notches = high(signed(wparam.0)) as f32 / 120.0;
                    self.push(Event::Wheel {
                        x: 0.0,
                        y: notches * 48.0,
                    });
                    LRESULT(0)
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    #[allow(clippy::cast_possible_truncation)] // Virtual-key codes are 16-bit.
                    let code = VIRTUAL_KEY(wparam.0 as u16);
                    let key = map_key(code);
                    let modifiers = Self::modifiers();
                    let paste_key = key.unwrap_or(kit::Key::Space);
                    if is_paste(&self.state, paste_key, modifiers, code == VK_INSERT) {
                        if let Some(paste) = paste_event() {
                            self.push(paste);
                        }
                        return LRESULT(0);
                    }
                    if let Some(key) = key {
                        self.push(Event::Key(key, modifiers));
                    }
                    if message == WM_SYSKEYDOWN {
                        return DefWindowProcW(hwnd, message, wparam, lparam);
                    }
                    LRESULT(0)
                }
                WM_CHAR => {
                    #[allow(clippy::cast_possible_truncation)] // A UTF-16 unit.
                    let unit = wparam.0 as u16;
                    if let Some(Ok(c)) = char::decode_utf16([unit]).next()
                        && !c.is_control()
                    {
                        self.push(Event::Text(c.to_string()));
                    }
                    LRESULT(0)
                }
                WM_SETCURSOR if u32::try_from(lparam.0 & 0xFFFF) == Ok(HTCLIENT) => {
                    let shape = match self.cursor {
                        Cursor::Default => IDC_ARROW,
                        Cursor::Pointer => IDC_HAND,
                        Cursor::Text => IDC_IBEAM,
                        Cursor::ResizeHorizontal => IDC_SIZEWE,
                        Cursor::ResizeVertical => IDC_SIZENS,
                    };
                    SetCursor(LoadCursorW(None, shape).ok());
                    LRESULT(1)
                }
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &raw mut paint);
                    self.paint();
                    let _ = EndPaint(hwnd, &raw const paint);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_DROPFILES => {
                    let drop = HDROP(wparam.0 as *mut core::ffi::c_void);
                    let mut at = POINT::default();
                    let _ = DragQueryPoint(drop, &raw mut at);
                    let mut paths = Vec::new();
                    for index in 0..DragQueryFileW(drop, u32::MAX, None) {
                        let length = DragQueryFileW(drop, index, None) as usize;
                        let mut name = vec![0_u16; length + 1];
                        let written = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                        paths.push(std::path::PathBuf::from(
                            <std::ffi::OsString as std::os::windows::ffi::OsStringExt>::from_wide(
                                &name[..written.min(length)],
                            ),
                        ));
                    }
                    DragFinish(drop);
                    #[allow(clippy::cast_precision_loss)] // Window coordinates are small.
                    let point = Point::new(at.x as f32 / self.scale, at.y as f32 / self.scale);
                    self.push(Event::PointerMoved(point));
                    self.push(Event::FilesDropped(paths));
                    LRESULT(0)
                }
                WM_SIZE => {
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &raw mut client);
                    #[allow(clippy::cast_sign_loss)] // Client sizes are positive.
                    let (w, h) = (
                        (client.right - client.left).max(1) as u32,
                        (client.bottom - client.top).max(1) as u32,
                    );
                    if let Some(target) = &self.target {
                        let _ = target.Resize(&D2D_SIZE_U {
                            width: w,
                            height: h,
                        });
                    }
                    self.invalidate();
                    LRESULT(0)
                }
                WM_GETMINMAXINFO => {
                    let info = &mut *(lparam.0 as *mut MINMAXINFO);
                    #[allow(clippy::cast_possible_truncation)] // Small sizes.
                    {
                        info.ptMinTrackSize = POINT {
                            x: (640.0 * self.scale) as i32,
                            y: (400.0 * self.scale) as i32,
                        };
                    }
                    LRESULT(0)
                }
                WM_DPICHANGED => {
                    #[allow(clippy::cast_precision_loss)] // DPI values are small.
                    let dpi = high(signed(wparam.0)).max(96) as f32;
                    self.scale = dpi / 96.0;
                    if let Some(target) = &self.target {
                        target.SetDpi(dpi, dpi);
                    }
                    let suggested = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    LRESULT(0)
                }
                WM_TIMER => {
                    let _ = KillTimer(Some(hwnd), REPAINT_TIMER);
                    self.invalidate();
                    LRESULT(0)
                }
                WM_SETFOCUS | WM_KILLFOCUS => {
                    self.window_focus(message == WM_SETFOCUS);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    fn ensure_target(&mut self) -> Option<ID2D1RenderTarget> {
        if let Some(target) = &self.target {
            return windows::core::Interface::cast(target).ok();
        }
        let mut client = RECT::default();
        // SAFETY: our own window; the properties outlive the call.
        unsafe {
            let _ = GetClientRect(self.hwnd, &raw mut client);
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0 * self.scale,
                dpiY: 96.0 * self.scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            #[allow(clippy::cast_sign_loss)] // Client sizes are positive.
            let window = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U {
                    width: (client.right - client.left).max(1) as u32,
                    height: (client.bottom - client.top).max(1) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = self
                .d2d
                .factory()
                .CreateHwndRenderTarget(&properties, &window)
                .ok()?;
            // An opaque target: ClearType text, as crisp as native controls.
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            self.target = Some(target.clone());
            windows::core::Interface::cast(&target).ok()
        }
    }

    fn paint(&mut self) {
        if self.painting {
            return;
        }
        let Some(target) = self.ensure_target() else {
            return;
        };
        self.painting = true;
        let mut client = RECT::default();
        // SAFETY: our own window.
        unsafe {
            let _ = GetClientRect(self.hwnd, &raw mut client);
        }
        #[allow(clippy::cast_precision_loss)] // Client sizes are small.
        let size = (
            (client.right - client.left) as f32 / self.scale,
            (client.bottom - client.top) as f32 / self.scale,
        );
        self.size = size;
        let events = std::mem::take(&mut self.events);
        for request in self.requests.try_iter() {
            if let Some(request) = Request::from_action(&request) {
                self.state.request(request);
            }
        }
        let time = self.started.elapsed().as_secs_f64();
        let maximized = self.maximized();
        let theme = self.app.theme();
        let mut request = None;
        let output = {
            let Ok(mut canvas) = D2dCanvas::new(&self.d2d, target.clone(), self.scale) else {
                // The input waits for the next frame.
                self.events = events;
                self.painting = false;
                return;
            };
            // SAFETY: drawing is bracketed by BeginDraw and EndDraw on this thread.
            unsafe { target.BeginDraw() };
            self.state.begin_frame(&events, time);
            {
                let mut ui = Ui::new(
                    &mut canvas,
                    theme,
                    &mut self.state,
                    Rect::new(0.0, 0.0, size.0, size.1),
                );
                request = request.or(self.app.frame(&mut ui, maximized));
            }
            canvas.finish();
            let mut output = self.state.end_frame();
            publish_clipboard(&mut self.state, &mut output);
            output
        };
        // SAFETY: as above.
        if let Err(error) = unsafe { target.EndDraw(None, None) }
            && error.code() == D2DERR_RECREATE_TARGET
        {
            self.target = None;
        }
        self.cursor = output.cursor;
        self.painting = false;
        let focus = self.state.focus();
        let (scale, title) = (self.scale, &self.title);
        if let Some(events) = self.access.as_mut().and_then(|access| {
            access.update_if_active(|| access::tree(&output.zones, focus, scale, title))
        }) {
            events.raise();
        }
        // SAFETY: window state changes on our own window.
        unsafe {
            match request {
                Some(WindowRequest::Minimize) => {
                    let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                }
                Some(WindowRequest::ToggleMaximize) => {
                    let _ = ShowWindow(self.hwnd, if maximized { SW_RESTORE } else { SW_MAXIMIZE });
                }
                Some(WindowRequest::Close) => {
                    let _ = DestroyWindow(self.hwnd);
                    return;
                }
                None => {}
            }
        }
        if output.repaint || self.target.is_none() {
            self.invalidate();
        }
        if let Some(after) = output.repaint_after {
            let ms = u32::try_from(after.as_millis()).unwrap_or(u32::MAX).max(1);
            // SAFETY: a timer on our own window; WM_TIMER below repaints and stops it.
            unsafe {
                let _ = SetTimer(Some(self.hwnd), REPAINT_TIMER, ms, None);
            }
        }
    }
}

fn map_key(key: VIRTUAL_KEY) -> Option<kit::Key> {
    use kit::Key;
    Some(match key {
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_OEM_PERIOD => Key::Char('.'),
        VK_OEM_COMMA => Key::Char(','),
        VK_OEM_MINUS => Key::Char('-'),
        VK_OEM_PLUS => Key::Char('='),
        VIRTUAL_KEY(code @ (0x30..=0x39 | 0x41..=0x5A)) => {
            Key::Char(char::from(u8::try_from(code).ok()?).to_ascii_lowercase())
        }
        VIRTUAL_KEY(code) if (VK_F1.0..=VK_F24.0).contains(&code) => {
            Key::F(u8::try_from(code - VK_F1.0 + 1).ok()?)
        }
        _ => return None,
    })
}
