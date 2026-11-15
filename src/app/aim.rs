//! Aim mode: pick a process by clicking one of its windows.
//!
//! Without a button held, `SetCapture` does not see the mouse over other threads' windows,
//! and a low-level mouse hook misses input aimed at elevated windows. So while aiming, a
//! nearly transparent window of ours (alpha 1) covers the whole virtual screen and takes
//! the click, and the window under it is found by walking the top-level windows in Z order.
//! The frame around the target and its label are small layered windows that clicks pass
//! through. The result goes to the main window as `WM_APP_AIM`.

use crate::system::win::{colorref, wide};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::sync::Once;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Posted to the main window when aiming ends: `wParam` is the PID, `lParam` is 1 when a
/// window was picked and 0 when aiming was cancelled.
pub const WM_APP_AIM: u32 = WM_APP + 3;

const OVERLAY_CLASS: &str = "LightTaskManagerAim";
const EDGE_CLASS: &str = "LightTaskManagerAimEdge";
const LABEL_CLASS: &str = "LightTaskManagerAimLabel";

const EDGE_COLOR: u32 = 0x5AA2FF;
const LABEL_BG: u32 = 0x2C2C33;
const LABEL_TEXT: u32 = 0xEDEDF2;
const LABEL_BORDER: u32 = 0x5AA2FF;

struct State {
    main: HWND,
    overlay: HWND,
    edges: [HWND; 4],
    label: HWND,
    font: HFONT,
    /// Pixels per 96 DPI pixel.
    scale: f32,
    /// Names as the process list shows them, for processes this app cannot open.
    names: HashMap<u32, String>,
    /// The window under the cursor, and its process.
    target: Option<(HWND, u32)>,
    text: Vec<u16>,
    /// The left button went down on the overlay, so its release picks.
    pressed: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn active() -> bool {
    STATE.with(|s| s.try_borrow().map(|s| s.is_some()).unwrap_or(true))
}

unsafe fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let hinstance = GetModuleHandleW(null());
        let classes = [
            (OVERLAY_CLASS, overlay_proc as WndProc, LoadCursorW(null_mut(), IDC_CROSS), GetStockObject(BLACK_BRUSH) as HBRUSH),
            (EDGE_CLASS, DefWindowProcW as WndProc, LoadCursorW(null_mut(), IDC_CROSS), CreateSolidBrush(colorref(EDGE_COLOR))),
            (LABEL_CLASS, label_proc as WndProc, LoadCursorW(null_mut(), IDC_CROSS), null_mut()),
        ];
        for (name, proc, cursor, brush) in classes {
            let class = wide(name);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: null_mut(),
                hCursor: cursor,
                hbrBackground: brush,
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
                hIconSm: null_mut(),
            };
            RegisterClassExW(&wc);
        }
    });
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Starts aiming. `names` maps PIDs to the names the process list shows.
pub fn start(main: HWND, names: HashMap<u32, String>) {
    if active() {
        return;
    }
    unsafe {
        register();
        let hinstance = GetModuleHandleW(null());
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let scale = GetDpiForWindow(main).max(96) as f32 / 96.0;
        let class = wide(OVERLAY_CLASS);
        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            null(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            main,
            null_mut(),
            hinstance,
            null(),
        );
        if overlay.is_null() {
            PostMessageW(main, WM_APP_AIM, 0, 0);
            return;
        }
        SetLayeredWindowAttributes(overlay, 0, 1, LWA_ALPHA);
        let marker = |class: &str| {
            let class = wide(class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                overlay,
                null_mut(),
                hinstance,
                null(),
            );
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
            hwnd
        };
        let edges = [marker(EDGE_CLASS), marker(EDGE_CLASS), marker(EDGE_CLASS), marker(EDGE_CLASS)];
        let label = marker(LABEL_CLASS);
        let round = DWMWCP_ROUNDSMALL;
        DwmSetWindowAttribute(label, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &round as *const _ as _, 4);
        let border = colorref(LABEL_BORDER);
        DwmSetWindowAttribute(label, DWMWA_BORDER_COLOR as u32, &border as *const _ as _, 4);
        let (face, weight) = crate::i18n::current().fonts()[0];
        let font_name = wide(face);
        let font = CreateFontW(
            -(14.0 * scale).round() as i32,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_NATURAL_QUALITY,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            font_name.as_ptr(),
        );
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                main,
                overlay,
                edges,
                label,
                font,
                scale,
                names,
                target: None,
                text: Vec::new(),
                pressed: false,
            })
        });
        ShowWindow(overlay, SW_SHOW);
        SetForegroundWindow(overlay);
        SetFocus(overlay);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        track(pt);
    }
}

/// Ends aiming and reports the result.
unsafe fn finish(picked: bool) {
    let Some(state) = STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.take())) else {
        return;
    };
    let pid = state.target.map(|t| t.1);
    let main = state.main;
    for hwnd in state.edges.iter().chain([&state.label, &state.overlay]) {
        DestroyWindow(*hwnd);
    }
    DeleteObject(state.font as HGDIOBJ);
    SetForegroundWindow(main);
    match (picked, pid) {
        (true, Some(pid)) => PostMessageW(main, WM_APP_AIM, pid as usize, 1),
        _ => PostMessageW(main, WM_APP_AIM, 0, 0),
    };
}

unsafe fn cloaked(hwnd: HWND) -> bool {
    let mut value = 0u32;
    DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut value as *mut _ as _, 4) == 0 && value != 0
}

/// Windows that clicks pass through, or that cannot be seen.
unsafe fn click_through(hwnd: HWND) -> bool {
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TRANSPARENT != 0 && ex & WS_EX_LAYERED != 0 {
        return true;
    }
    if ex & WS_EX_LAYERED != 0 {
        let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
        if GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags) != 0 && flags & LWA_ALPHA != 0 && alpha == 0 {
            return true;
        }
    }
    false
}

/// The visible bounds of a window, without the invisible resize borders.
unsafe fn bounds(hwnd: HWND) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let size = std::mem::size_of::<RECT>() as u32;
    if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut _ as _, size) != 0 {
        GetWindowRect(hwnd, &mut r);
    }
    r
}

struct Search {
    pt: POINT,
    skip: [HWND; 6],
    found: HWND,
}

unsafe extern "system" fn find_top(hwnd: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp as *mut Search);
    if s.skip.contains(&hwnd) || IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || cloaked(hwnd) || click_through(hwnd) {
        return 1;
    }
    let r = bounds(hwnd);
    if s.pt.x >= r.left && s.pt.x < r.right && s.pt.y >= r.top && s.pt.y < r.bottom {
        s.found = hwnd;
        return 0;
    }
    1
}

unsafe fn class_is(hwnd: HWND, name: &str) -> bool {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == name
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

unsafe extern "system" fn find_core(hwnd: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp as *mut (u32, u32));
    if class_is(hwnd, "Windows.UI.Core.CoreWindow") {
        let pid = pid_of(hwnd);
        if pid != 0 && pid != s.0 {
            s.1 = pid;
            return 0;
        }
    }
    1
}

/// The process a top-level window stands for. A UWP app's frame belongs to
/// ApplicationFrameHost; the app itself owns the CoreWindow inside it.
unsafe fn process_of(hwnd: HWND) -> u32 {
    let pid = pid_of(hwnd);
    if class_is(hwnd, "ApplicationFrameWindow") {
        let mut s = (pid, 0u32);
        EnumChildWindows(hwnd, Some(find_core), &mut s as *mut _ as LPARAM);
        if s.1 != 0 {
            return s.1;
        }
    }
    pid
}

/// The top-level window under a screen point, skipping ours.
unsafe fn window_at(pt: POINT, skip: [HWND; 6]) -> HWND {
    let mut s = Search { pt, skip, found: null_mut() };
    EnumWindows(Some(find_top), &mut s as *mut _ as LPARAM);
    if s.found.is_null() {
        // Only the desktop is left there.
        GetShellWindow()
    } else {
        s.found
    }
}

fn image_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        let path = ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))?;
        path.rsplit('\\').next().map(String::from)
    }
}

/// Follows the cursor: finds the window under it and moves the frame there.
unsafe fn track(pt: POINT) {
    let found = STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let s = s.as_ref()?;
        let skip = [s.overlay, s.label, s.edges[0], s.edges[1], s.edges[2], s.edges[3]];
        Some((window_at(pt, skip), s.target))
    });
    let Some((hwnd, previous)) = found else { return };
    if hwnd.is_null() || previous.is_some_and(|t| t.0 == hwnd) {
        return;
    }
    let pid = process_of(hwnd);
    let r = bounds(hwnd);
    let placed = STATE.with(|s| {
        let mut s = s.try_borrow_mut().ok()?;
        let s = s.as_mut()?;
        let name = s.names.get(&pid).cloned().or_else(|| image_name(pid)).unwrap_or_else(|| "?".into());
        s.target = Some((hwnd, pid));
        s.text = format!("{name} ({pid})").encode_utf16().collect();
        Some((s.edges, s.label, s.font, s.scale, s.text.clone()))
    });
    let Some((edges, label, font, scale, text)) = placed else { return };

    let t = (3.0 * scale).round() as i32;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let sides = [
        (r.left, r.top, w, t),
        (r.left, r.bottom - t, w, t),
        (r.left, r.top, t, h),
        (r.right - t, r.top, t, h),
    ];
    let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
    for (hwnd, (x, y, w, h)) in edges.iter().zip(sides) {
        SetWindowPos(*hwnd, HWND_TOPMOST, x, y, w.max(1), h.max(1), flags);
    }

    // The label sits just above the frame, or inside it when there is no room above.
    let dc = GetDC(label);
    let old = SelectObject(dc, font as HGDIOBJ);
    let mut size = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
    SelectObject(dc, old);
    ReleaseDC(label, dc);
    let pad = (8.0 * scale).round() as i32;
    let (lw, lh) = (size.cx + pad * 2, size.cy + pad);
    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let screen = info.rcMonitor;
    let gap = (4.0 * scale).round() as i32;
    let mut ly = r.top - lh - gap;
    if ly < screen.top {
        ly = r.top.max(screen.top) + t + gap;
    }
    let lx = (r.left.max(screen.left) + if ly > r.top { t + gap } else { 0 }).min(screen.right - lw).max(screen.left);
    SetWindowPos(label, HWND_TOPMOST, lx, ly, lw, lh, flags);
    InvalidateRect(label, null(), 1);
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            track(pt);
            0
        }
        WM_LBUTTONDOWN => {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            track(pt);
            STATE.with(|s| {
                if let Some(s) = s.try_borrow_mut().ok().as_mut().and_then(|s| s.as_mut()) {
                    s.pressed = true;
                }
            });
            0
        }
        // Acting on release keeps the release from reaching the window below.
        WM_LBUTTONUP => {
            let pressed = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.pressed)));
            if pressed == Some(true) {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                track(pt);
                finish(true);
            }
            0
        }
        WM_RBUTTONDOWN => 0,
        WM_RBUTTONUP => {
            finish(false);
            0
        }
        WM_KEYDOWN if wp as u16 == VK_ESCAPE => {
            finish(false);
            0
        }
        WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => {
            finish(false);
            0
        }
        WM_CANCELMODE => {
            finish(false);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn label_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut r);
            let brush = CreateSolidBrush(colorref(LABEL_BG));
            FillRect(dc, &r, brush);
            DeleteObject(brush as HGDIOBJ);
            let found = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| (s.font, s.text.clone()))));
            if let Some((font, mut text)) = found {
                let old = SelectObject(dc, font as HGDIOBJ);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, colorref(LABEL_TEXT));
                DrawTextW(dc, text.as_mut_ptr(), text.len() as i32, &mut r, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
                SelectObject(dc, old);
            }
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
