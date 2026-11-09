//! The Win32 window: translates messages into `Core` events.

use crate::actions;
use crate::core::{Caption, Core};
use crate::gfx::rect;
use crate::pages::{Cmd, Cursor, Ev};
use crate::sampler::{self, Shared, WM_APP_ENRICHED, WM_APP_SNAPSHOT};
use crate::settings::Settings;
use crate::theme::THEME;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const TIMER_ANIM: usize = 1;
const EDIT_ID: usize = 100;
const EDIT_MARGIN: isize = 0;

static ORIGINAL_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static EDIT_FONT: AtomicIsize = AtomicIsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn colorref(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

struct App {
    hwnd: HWND,
    edit: HWND,
    core: Core,
    field_brush: HBRUSH,
    timer_on: bool,
    last_tick: Instant,
    tracking: bool,
    nc_tracking: bool,
    pressed_caption: Option<Caption>,
    capturing: bool,
}

/// Asks Windows for dark native menus. These entry points are unnamed, so a
/// missing one only leaves the text box's context menu light.
fn enable_dark_menus() {
    unsafe {
        let name = wide("uxtheme.dll");
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() {
            return;
        }
        type SetMode = unsafe extern "system" fn(i32) -> i32;
        type Flush = unsafe extern "system" fn();
        if let Some(f) = GetProcAddress(module, 135 as *const u8) {
            let set: SetMode = std::mem::transmute(f);
            set(2);
        }
        if let Some(f) = GetProcAddress(module, 136 as *const u8) {
            let flush: Flush = std::mem::transmute(f);
            flush();
        }
    }
}

fn create_edit_font(px: i32) -> HFONT {
    let name = wide("Yu Gothic UI");
    unsafe {
        CreateFontW(
            -px, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_NATURAL_QUALITY as u32, (DEFAULT_PITCH | FF_DONTCARE as u8) as u32, name.as_ptr(),
        )
    }
}

fn app_icon(size: i32) -> HICON {
    let accent = THEME.accent;
    let on = 0x0B1730u32;
    let f = size as f32;
    let radius = f * 0.24;
    let points = [(0.16, 0.56), (0.36, 0.56), (0.46, 0.28), (0.58, 0.74), (0.68, 0.48), (0.84, 0.48)];
    let mut px = vec![0u32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let qx = (fx - f / 2.0).abs() - f / 2.0 + radius;
            let qy = (fy - f / 2.0).abs() - f / 2.0 + radius;
            let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
            let tile = (0.5 - d).clamp(0.0, 1.0);
            let mut line_d = f32::MAX;
            for w in points.windows(2) {
                let (ax, ay, bx, by) = (w[0].0 * f, w[0].1 * f, w[1].0 * f, w[1].1 * f);
                let (dx, dy) = (bx - ax, by - ay);
                let t = (((fx - ax) * dx + (fy - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                line_d = line_d.min(((fx - ax - t * dx).powi(2) + (fy - ay - t * dy).powi(2)).sqrt());
            }
            let line = (f * 0.05 + 0.5 - line_d).clamp(0.0, 1.0);
            let mix = |shift: u32| {
                let (a, b) = (((accent >> shift) & 255) as f32, ((on >> shift) & 255) as f32);
                ((a + (b - a) * line) * tile) as u32
            };
            px[(y * size + x) as usize] =
                (((tile * 255.0) as u32) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0);
        }
    }
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits = null_mut();
        let color = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask = CreateBitmap(size, size, 1, 1, null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    enable_dark_menus();
    let settings = Settings::load();
    let shared = Shared::new();
    shared.interval_ms.store(settings.interval_ms, Ordering::Relaxed);
    let want_gui = settings.columns.iter().any(|(c, _)| crate::columns::needs_gui_counts(*c));
    shared.want_gui_counts.store(want_gui, Ordering::Relaxed);

    unsafe {
        let hinstance = GetModuleHandleW(null());
        let class = wide("LightTaskManagerWindow");
        let big = app_icon(48);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: big,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: app_icon(20),
        };
        RegisterClassExW(&wc);

        let title = wide("Light Task Manager");
        let params = Box::into_raw(Box::new((settings, shared.clone())));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
            null_mut(),
            hinstance,
            params as *const _,
        );
        if hwnd.is_null() {
            return;
        }
        shared.hwnd.store(hwnd as isize, Ordering::Relaxed);
        sampler::start(shared);

        let app = app_of(hwnd);
        let maximized = (*app).core.settings.maximized;
        ShowWindow(hwnd, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn app_of(hwnd: HWND) -> *mut App {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App
}

fn lo(v: isize) -> i32 {
    (v & 0xFFFF) as u16 as i16 as i32
}

fn hi(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i16 as i32
}

unsafe fn scale_of(hwnd: HWND) -> f32 {
    GetDpiForWindow(hwnd) as f32 / 96.0
}

unsafe fn create(hwnd: HWND, cs: &CREATESTRUCTW) -> Box<App> {
    let (settings, shared) = *Box::from_raw(cs.lpCreateParams as *mut (Settings, Arc<Shared>));
    let scale = scale_of(hwnd);
    shared.icon_px.store((16.0 * scale).round() as i32, Ordering::Relaxed);

    let (w, h) = (
        (settings.width as f32 * scale) as i32,
        (settings.height as f32 * scale) as i32,
    );
    let mut work: RECT = std::mem::zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
    let (w, h) = (w.min(work.right - work.left), h.min(work.bottom - work.top));
    let (x, y) = (
        work.left + (work.right - work.left - w) / 2,
        work.top + (work.bottom - work.top - h) / 2,
    );

    let core = Core::new(settings, shared, scale, w, h);
    let edit_class = wide("EDIT");
    let edit = CreateWindowExW(
        0,
        edit_class.as_ptr(),
        null(),
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
        0, 0, 10, 10,
        hwnd,
        EDIT_ID as HMENU,
        cs.hInstance,
        null(),
    );
    let font = create_edit_font(core.canvas.px(13));
    EDIT_FONT.store(font as isize, Ordering::Relaxed);
    SendMessageW(edit, WM_SETFONT, font as usize, 0);
    SendMessageW(edit, EM_SETMARGINS, (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize, EDIT_MARGIN);
    let original = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
    ORIGINAL_EDIT_PROC.store(original, Ordering::Relaxed);
    MAIN_HWND.store(hwnd as isize, Ordering::Relaxed);

    let dark: BOOL = 1;
    DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, (&dark as *const BOOL).cast(), 4);
    let caption = colorref(THEME.window);
    DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, (&caption as *const u32).cast(), 4);
    SetWindowPos(hwnd, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);

    Box::new(App {
        hwnd,
        edit,
        core,
        field_brush: CreateSolidBrush(colorref(THEME.field)),
        timer_on: false,
        last_tick: Instant::now(),
        tracking: false,
        nc_tracking: false,
        pressed_caption: None,
        capturing: false,
    })
}

impl App {
    unsafe fn place_edit(&self) {
        let r = self.core.edit_rect();
        let show = if self.core.wants_filter() { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
        SetWindowPos(self.edit, null_mut(), r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | show);
    }

    unsafe fn fire(&mut self, ev: Ev) {
        let was_filter = self.core.wants_filter();
        let clear = self.core.event(ev);
        if clear {
            SetWindowTextW(self.edit, wide("").as_ptr());
            SetFocus(self.edit);
        }
        if was_filter != self.core.wants_filter() {
            self.place_edit();
        }
        self.after(!matches!(ev, Ev::Tick { .. }));
    }

    unsafe fn after(&mut self, immediate: bool) {
        for cmd in std::mem::take(&mut self.core.cmds) {
            match cmd {
                Cmd::Toast(m) => self.core.toast(m),
                Cmd::FocusFilter => {
                    SetFocus(self.edit);
                }
                Cmd::Elevate => {
                    if actions::relaunch_elevated() {
                        PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                    } else {
                        self.core.toast("管理者としての再起動はキャンセルされました。".into());
                    }
                }
            }
        }
        let animating = self.core.animating();
        if animating && !self.timer_on {
            self.last_tick = Instant::now();
            SetTimer(self.hwnd, TIMER_ANIM, 10, None);
            self.timer_on = true;
        } else if !animating && self.timer_on {
            KillTimer(self.hwnd, TIMER_ANIM);
            self.timer_on = false;
        }
        if self.core.dirty {
            InvalidateRect(self.hwnd, null(), 0);
            if immediate {
                UpdateWindow(self.hwnd);
            }
        }
    }

    unsafe fn track_leave(&mut self) {
        if !self.tracking {
            let mut t = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut t);
            self.tracking = true;
        }
    }

    unsafe fn set_caption_hover(&mut self, hover: Option<Caption>) {
        if self.core.caption_hover != hover {
            self.core.caption_hover = hover;
            self.core.dirty = true;
            self.after(true);
        }
    }

    unsafe fn caption_action(&mut self, caption: Caption) {
        match caption {
            Caption::Minimize => {
                ShowWindow(self.hwnd, SW_MINIMIZE);
            }
            Caption::Maximize => {
                let cmd = if IsZoomed(self.hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE };
                ShowWindow(self.hwnd, cmd);
            }
            Caption::Close => {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }

    unsafe fn save(&mut self) {
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        GetWindowPlacement(self.hwnd, &mut placement);
        let r = placement.rcNormalPosition;
        let scale = self.core.canvas.scale;
        let s = &mut self.core.settings;
        s.width = ((r.right - r.left) as f32 / scale) as i32;
        s.height = ((r.bottom - r.top) as f32 / scale) as i32;
        s.maximized = IsZoomed(self.hwnd) != 0;
        s.save();
    }

    unsafe fn hit_test(&self, screen_x: i32, screen_y: i32) -> u32 {
        let mut p = POINT { x: screen_x, y: screen_y };
        ScreenToClient(self.hwnd, &mut p);
        let frame = GetSystemMetricsForDpi(SM_CYFRAME, GetDpiForWindow(self.hwnd))
            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, GetDpiForWindow(self.hwnd));
        if IsZoomed(self.hwnd) == 0 && p.y < frame.min(self.core.canvas.px(5)) {
            return HTTOP;
        }
        if let Some(c) = self.core.caption_at(p.x, p.y) {
            return match c {
                Caption::Minimize => HTMINBUTTON,
                Caption::Maximize => HTMAXBUTTON,
                Caption::Close => HTCLOSE,
            };
        }
        if self.core.is_drag_area(p.x, p.y) {
            return HTCAPTION;
        }
        HTCLIENT
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let original: WNDPROC = std::mem::transmute(ORIGINAL_EDIT_PROC.load(Ordering::Relaxed));
    let main = MAIN_HWND.load(Ordering::Relaxed) as HWND;
    match msg {
        WM_KEYDOWN => match wp as u16 {
            VK_ESCAPE => {
                SetWindowTextW(hwnd, wide("").as_ptr());
                SetFocus(main);
                return 0;
            }
            VK_DOWN | VK_RETURN => {
                SetFocus(main);
                PostMessageW(main, WM_KEYDOWN, VK_DOWN as usize, 0);
                return 0;
            }
            _ => {}
        },
        WM_CHAR => match wp {
            0x1B | 0x0D | 0x09 => return 0,
            0x01 => {
                SendMessageW(hwnd, EM_SETSEL, 0, -1);
                return 0;
            }
            _ => {}
        },
        WM_PAINT => {
            let result = CallWindowProcW(original, hwnd, msg, wp, lp);
            if GetWindowTextLengthW(hwnd) == 0 {
                let dc = GetDC(hwnd);
                let previous = SelectObject(dc, EDIT_FONT.load(Ordering::Relaxed) as HFONT);
                SetBkMode(dc, TRANSPARENT as i32);
                SetTextColor(dc, colorref(THEME.text_faint));
                let cue: Vec<u16> = "名前、PID、ユーザー名、説明で絞り込み".encode_utf16().collect();
                TextOutW(dc, 1, 0, cue.as_ptr(), cue.len() as i32);
                SelectObject(dc, previous);
                ReleaseDC(hwnd, dc);
            }
            return result;
        }
        _ => {}
    }
    CallWindowProcW(original, hwnd, msg, wp, lp)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        EnableNonClientDpiScaling(hwnd);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_CREATE {
        let cs = &*(lp as *const CREATESTRUCTW);
        let app = Box::into_raw(create(hwnd, cs));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        // WM_NCCALCSIZE only removes the caption once the app state is reachable.
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        return 0;
    }
    let ptr = app_of(hwnd);
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let app = &mut *ptr;

    match msg {
        WM_NCCALCSIZE if wp != 0 => {
            let params = &mut *(lp as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let result = DefWindowProcW(hwnd, msg, wp, lp);
            if result != 0 {
                return result;
            }
            params.rgrc[0].top = top;
            if IsZoomed(hwnd) != 0 {
                let dpi = GetDpiForWindow(hwnd);
                params.rgrc[0].top +=
                    GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            }
            0
        }
        WM_NCHITTEST => {
            let default = DefWindowProcW(hwnd, msg, wp, lp) as u32;
            if default != HTCLIENT && default != HTCAPTION && default != HTTOP {
                return default as LRESULT;
            }
            app.hit_test(lo(lp), hi(lp)) as LRESULT
        }
        WM_NCMOUSEMOVE => {
            let caption = match wp as u32 {
                HTMINBUTTON => Some(Caption::Minimize),
                HTMAXBUTTON => Some(Caption::Maximize),
                HTCLOSE => Some(Caption::Close),
                _ => None,
            };
            if caption.is_some() && !app.nc_tracking {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut t);
                app.nc_tracking = true;
            }
            app.set_caption_hover(caption);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_NCMOUSELEAVE => {
            app.nc_tracking = false;
            app.pressed_caption = None;
            app.set_caption_hover(None);
            0
        }
        WM_NCLBUTTONDOWN if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            app.pressed_caption = app.core.caption_hover;
            0
        }
        WM_NCLBUTTONUP if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            if let Some(c) = app.pressed_caption.take() {
                if app.core.caption_hover == Some(c) {
                    app.caption_action(c);
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if app.core.dirty {
                app.core.render();
            }
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let r = ps.rcPaint;
            app.core.canvas.present(dc, rect(r.left, r.top, r.right - r.left, r.bottom - r.top));
            EndPaint(hwnd, &ps);
            0
        }
        WM_SIZE => {
            let (w, h) = (lo(lp), hi(lp));
            if wp as u32 == SIZE_MINIMIZED {
                app.core.shared.minimized.store(true, Ordering::Relaxed);
                return 0;
            }
            if app.core.shared.minimized.swap(false, Ordering::Relaxed) {
                app.core.shared.poke();
            }
            app.core.maximized = wp as u32 == SIZE_MAXIMIZED;
            app.core.canvas.resize(w, h);
            app.core.dirty = true;
            app.place_edit();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp as *mut MINMAXINFO);
            let scale = app.core.canvas.scale;
            info.ptMinTrackSize = POINT {
                x: (820.0 * scale) as i32,
                y: (600.0 * scale) as i32,
            };
            0
        }
        WM_DPICHANGED => {
            let scale = hi(wp as isize) as f32 / 96.0;
            app.core.canvas.set_scale(scale);
            let previous = EDIT_FONT.load(Ordering::Relaxed);
            let font = create_edit_font(app.core.canvas.px(13));
            EDIT_FONT.store(font as isize, Ordering::Relaxed);
            SendMessageW(app.edit, WM_SETFONT, font as usize, 1);
            DeleteObject(previous as HFONT);
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            app.core.dirty = true;
            0
        }
        WM_SETCURSOR => {
            if (lp & 0xFFFF) as u32 != HTCLIENT || wp as HWND != hwnd {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            ScreenToClient(hwnd, &mut p);
            let id = match app.core.cursor(p.x, p.y) {
                Cursor::Arrow => IDC_ARROW,
                Cursor::SizeWE => IDC_SIZEWE,
                Cursor::IBeam => IDC_IBEAM,
            };
            SetCursor(LoadCursorW(null_mut(), id));
            1
        }
        WM_MOUSEMOVE => {
            app.track_leave();
            if app.core.caption_hover.is_some() {
                app.core.caption_hover = None;
                app.core.dirty = true;
            }
            app.fire(Ev::Move { x: lo(lp), y: hi(lp) });
            0
        }
        WM_MOUSELEAVE => {
            app.tracking = false;
            app.fire(Ev::Leave);
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN => {
            if GetFocus() == app.edit {
                let hit_search = app.core.search_rect().contains(lo(lp), hi(lp));
                if !hit_search {
                    SetFocus(hwnd);
                }
            } else {
                SetFocus(hwnd);
            }
            SetCapture(hwnd);
            app.capturing = true;
            app.fire(Ev::Down { x: lo(lp), y: hi(lp), right: msg == WM_RBUTTONDOWN });
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            if app.capturing {
                app.capturing = false;
                ReleaseCapture();
            }
            app.fire(Ev::Up { x: lo(lp) });
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = hi(wp as isize);
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            let horizontal = msg == WM_MOUSEHWHEEL || shift;
            let notches = delta as f32 / 120.0;
            let ev = if horizontal {
                let sign = if msg == WM_MOUSEHWHEEL { -1.0 } else { 1.0 };
                Ev::Wheel { dx: notches * sign, dy: 0.0 }
            } else {
                Ev::Wheel { dx: 0.0, dy: notches }
            };
            app.fire(ev);
            0
        }
        WM_TIMER if wp == TIMER_ANIM => {
            let now = Instant::now();
            let dt = (now - app.last_tick).as_secs_f32().min(0.05);
            app.last_tick = now;
            app.fire(Ev::Tick { dt });
            0
        }
        WM_KEYDOWN => {
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            if ctrl && wp as u16 == VK_F {
                SetFocus(app.edit);
                SendMessageW(app.edit, EM_SETSEL, 0, -1);
                return 0;
            }
            app.fire(Ev::Key { vk: wp as u16, ctrl });
            0
        }
        WM_CHAR => {
            let printable = wp >= 0x20 && wp != 0x7F;
            if printable && app.core.wants_filter() {
                SetFocus(app.edit);
                SendMessageW(app.edit, WM_CHAR, wp, lp);
            }
            0
        }
        WM_COMMAND if (wp & 0xFFFF) == EDIT_ID => {
            match (wp >> 16) as u32 {
                EN_CHANGE => {
                    let n = GetWindowTextLengthW(app.edit) as usize;
                    let mut buf = vec![0u16; n + 1];
                    let got = GetWindowTextW(app.edit, buf.as_mut_ptr(), buf.len() as i32) as usize;
                    app.core.set_filter(String::from_utf16_lossy(&buf[..got]));
                    app.after(true);
                }
                EN_SETFOCUS => {
                    app.core.filter_focused = true;
                    app.core.dirty = true;
                    app.after(true);
                }
                EN_KILLFOCUS => {
                    app.core.filter_focused = false;
                    app.core.dirty = true;
                    app.after(true);
                }
                _ => {}
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let dc = wp as HDC;
            SetTextColor(dc, colorref(THEME.text));
            SetBkColor(dc, colorref(THEME.field));
            app.field_brush as LRESULT
        }
        WM_APP_SNAPSHOT => {
            let snap = app.core.shared.slot.lock().unwrap().take();
            if let Some(snap) = snap {
                app.core.on_snapshot(&snap);
                app.after(false);
            }
            0
        }
        WM_APP_ENRICHED => {
            app.core.shared.enrich_posted.store(false, Ordering::Release);
            app.core.on_enriched();
            app.after(false);
            0
        }
        WM_CLOSE => {
            app.save();
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
