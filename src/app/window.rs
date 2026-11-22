//! The Win32 window: translates messages into `Core` events.

use crate::app::core::{Caption, Core};
use crate::app::pointer::{Contact, Gesture, Out, HOLD_MS};
use crate::app::tray::{self, Tray, Want, WM_APP_TRAY};
use crate::i18n::tr;
use crate::pages::{Cmd, Cursor, Ev};
use crate::settings::Settings;
use crate::system::actions;
use crate::system::sampler::{self, Shared, WM_APP_ENRICHED, WM_APP_SNAPSHOT};
use crate::system::win::{colorref, wide};
use crate::ui::gfx::rect;
use crate::ui::theme::THEME;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

windows_targets::link!("user32.dll" "system" fn GetPointerType(id: u32, kind: *mut POINTER_INPUT_TYPE) -> BOOL);

const TIMER_ANIM: usize = 1;
const TIMER_HOLD: usize = 2;
const EDIT_ID: usize = 100;
const EDIT_MARGIN: isize = 0;
const TOUCH_SLOP: i32 = 10;
const PEN_SLOP: i32 = 5;

static ORIGINAL_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static EDIT_FONT: AtomicIsize = AtomicIsize::new(0);

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
    gesture: Gesture,
    started: Instant,
    /// The icon in the notification area.
    tray: Tray,
    /// The window is hidden in the notification area.
    hidden: bool,
    /// The app is ending: the next WM_CLOSE closes rather than hides.
    quitting: bool,
}

/// The message Explorer sends when the taskbar comes (back): the icons have to be added again.
fn taskbar_created() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
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
    let (face, weight) = crate::i18n::current().fonts()[0];
    let name = wide(face);
    unsafe {
        CreateFontW(
            -px, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_NATURAL_QUALITY, (DEFAULT_PITCH | FF_DONTCARE) as u32, name.as_ptr(),
        )
    }
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    enable_dark_menus();
    let mut settings = Settings::load();
    crate::i18n::set(crate::i18n::resolve(settings.language));
    // Start again elevated before any window is made. If the user declines the prompt,
    // this copy carries on without elevation.
    if settings.elevate_on_start && !actions::is_elevated() && actions::relaunch_elevated(false) {
        return;
    }
    // Only one copy runs. This comes after the restart above, which ends before it holds anything.
    let Some(_single) = crate::app::instance::acquire() else {
        return;
    };
    // Settings of the recording changed in the run before apply now, with the old copy gone.
    settings.apply_pending();
    let record = crate::system::recorder::plan(&settings);
    settings.rec_running = record.as_ref().map(|p| p.dir.clone());
    let shared = Shared::new();
    shared.interval_ms.store(settings.interval_ms, Ordering::Relaxed);

    unsafe {
        let hinstance = GetModuleHandleW(null());
        let class = wide(crate::app::instance::WINDOW_CLASS);
        let big = crate::app::icon::app_icon(48);
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
            hIconSm: crate::app::icon::app_icon(20),
        };
        RegisterClassExW(&wc);

        let title = wide(actions::app_title());
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
        sampler::start(shared, record);

        let app = app_of(hwnd);
        let maximized = (*app).core.settings.maximized;
        ShowWindow(hwnd, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
        UpdateWindow(hwnd);
        (*app).sync_tray();

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The "Save as" dialog. Returns the chosen path, or None when cancelled.
unsafe fn save_dialog(owner: HWND, name: &str, kind: &str, ext: &str) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut file: Vec<u16> = name.encode_utf16().collect();
    file.resize(2048, 0);
    // "CSV files (*.csv)\0*.csv\0All files (*.*)\0*.*\0\0"
    let (file_type, all) = ((tr().app.file_type)(kind), tr().app.all_files);
    let filter: Vec<u16> = format!("{file_type} (*.{ext})\0*.{ext}\0{all} (*.*)\0*.*\0\0").encode_utf16().collect();
    let def_ext: Vec<u16> = ext.encode_utf16().chain([0]).collect();
    let mut ofn: OPENFILENAMEW = std::mem::zeroed();
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrDefExt = def_ext.as_ptr();
    ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if GetSaveFileNameW(&mut ofn) == 0 {
        return None;
    }
    let end = file.iter().position(|c| *c == 0).unwrap_or(file.len());
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&file[..end])))
}

/// The folder dialog of the current style (the one with the address bar and the navigation pane),
/// the common item dialog set to pick folders. Returns the chosen folder, or None when cancelled.
/// windows-sys has no COM vtables, so the few methods needed are called by their slot numbers.
unsafe fn folder_dialog(owner: HWND, title: &str, start: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    use std::ffi::c_void;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::SHCreateItemFromParsingName;

    const CLSID_FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xDC1C5A9C_E88A_4DDE_A5A1_60F82A20AEF7);
    const IID_FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xD57C7288_D4AD_4768_BE02_9D969532D960);
    const IID_SHELL_ITEM: GUID = GUID::from_u128(0x43826D1E_E718_42EE_BC55_A1E261C37BFE);
    const FOS_PICKFOLDERS: u32 = 0x20;
    const FOS_FORCEFILESYSTEM: u32 = 0x40;
    const SIGDN_FILESYSPATH: u32 = 0x8005_8000;

    // The function in slot `i` of a COM object's table.
    unsafe fn slot<T: Copy>(obj: *mut c_void, i: usize) -> T {
        let table = *(obj as *const *const usize);
        std::mem::transmute_copy(&*table.add(i))
    }
    unsafe fn release(obj: *mut c_void) {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = slot(obj, 2);
        f(obj);
    }

    let com = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) >= 0;
    let mut out = None;
    let mut dialog: *mut c_void = null_mut();
    if CoCreateInstance(&CLSID_FILE_OPEN_DIALOG, null_mut(), CLSCTX_INPROC_SERVER, &IID_FILE_OPEN_DIALOG, &mut dialog) >= 0 && !dialog.is_null() {
        // IFileDialog: SetOptions 9, GetOptions 10, SetFolder 12, SetTitle 17, Show 3, GetResult 20.
        let get_options: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = slot(dialog, 10);
        let set_options: unsafe extern "system" fn(*mut c_void, u32) -> i32 = slot(dialog, 9);
        let mut options = 0u32;
        get_options(dialog, &mut options);
        set_options(dialog, options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 = slot(dialog, 17);
        set_title(dialog, wide(title).as_ptr());
        if let Some(start) = start {
            let mut folder: *mut c_void = null_mut();
            let path = wide(&start.display().to_string());
            if SHCreateItemFromParsingName(path.as_ptr(), null_mut(), &IID_SHELL_ITEM, &mut folder) >= 0 && !folder.is_null() {
                let set_folder: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = slot(dialog, 12);
                set_folder(dialog, folder);
                release(folder);
            }
        }
        let show: unsafe extern "system" fn(*mut c_void, HWND) -> i32 = slot(dialog, 3);
        if show(dialog, owner) >= 0 {
            let mut result: *mut c_void = null_mut();
            let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = slot(dialog, 20);
            if get_result(dialog, &mut result) >= 0 && !result.is_null() {
                // IShellItem::GetDisplayName is slot 5.
                let get_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32 = slot(result, 5);
                let mut name: *mut u16 = null_mut();
                if get_name(result, SIGDN_FILESYSPATH, &mut name) >= 0 && !name.is_null() {
                    let mut len = 0;
                    while *name.add(len) != 0 {
                        len += 1;
                    }
                    out = Some(std::path::PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(name, len))));
                    CoTaskMemFree(name as *const _);
                }
                release(result);
            }
        }
        release(dialog);
    }
    if com {
        CoUninitialize();
    }
    out
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
    // Running as administrator, the window would not hear these from Explorer or from a
    // second start with fewer rights.
    for msg in [taskbar_created(), crate::app::instance::show_message(), WM_APP_TRAY] {
        ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, null_mut());
    }

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
        gesture: Gesture::new(),
        started: Instant::now(),
        tray: Tray::new(hwnd),
        hidden: false,
        quitting: false,
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

    /// Makes the search box's font again, for a new scale or language.
    unsafe fn reset_edit_font(&mut self) {
        let previous = EDIT_FONT.load(Ordering::Relaxed);
        let font = create_edit_font(self.core.canvas.px(13));
        EDIT_FONT.store(font as isize, Ordering::Relaxed);
        SendMessageW(self.edit, WM_SETFONT, font as usize, 1);
        DeleteObject(previous as HFONT);
    }

    /// Brings the notification area icon in line with the settings.
    unsafe fn sync_tray(&mut self) {
        let want = Want::of(&self.core.settings);
        self.tray.sync(want);
        // Without an icon there would be no way back to a hidden window.
        if want == Want::None && self.hidden {
            self.reveal();
        }
    }

    /// Shows the window, from the notification area or minimized, and brings it to the front.
    unsafe fn reveal(&mut self) {
        if self.hidden {
            self.hidden = false;
            ShowWindow(self.hwnd, SW_SHOW);
            // Hidden while minimized, it shows minimized: the restore below takes it from there.
            if IsIconic(self.hwnd) == 0 && self.core.shared.minimized.swap(false, Ordering::Relaxed) {
                self.core.shared.poke();
            }
            self.core.dirty = true;
            InvalidateRect(self.hwnd, null(), 0);
        }
        if IsIconic(self.hwnd) != 0 {
            ShowWindow(self.hwnd, SW_RESTORE);
        }
        SetForegroundWindow(self.hwnd);
    }

    /// Hides the window in the notification area. The sampler reads only what is needed while
    /// minimized, and the settings are saved, since the app may stay there for days.
    unsafe fn conceal(&mut self) {
        self.save();
        self.core.shared.minimized.store(true, Ordering::Relaxed);
        ShowWindow(self.hwnd, SW_HIDE);
        self.hidden = true;
    }

    /// Ends the app, whatever the close button would do.
    unsafe fn quit(&mut self) {
        self.quitting = true;
        PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
    }

    unsafe fn after(&mut self, immediate: bool) {
        for cmd in std::mem::take(&mut self.core.cmds) {
            match cmd {
                Cmd::Toast(m) => self.core.toast(m),
                Cmd::FocusFilter => {
                    SetFocus(self.edit);
                }
                Cmd::ShowProcess(pid) => {
                    // The process runs now: show the latest list, not a moment picked on the graph.
                    self.core.details_go_live();
                    self.core.navigate(0);
                    self.place_edit();
                    // EN_CHANGE applies the filter.
                    SetWindowTextW(self.edit, wide(&pid.to_string()).as_ptr());
                }
                Cmd::Save(req) => self.save_file(req),
                Cmd::Aim => {
                    let names = match self.core.page_is_details() {
                        Some(page) => page.names(),
                        None => Default::default(),
                    };
                    crate::app::aim::start(self.hwnd, names);
                }
                Cmd::Language => {
                    crate::i18n::set(crate::i18n::resolve(self.core.settings.language));
                    // The fonts for text that is not ASCII depend on the language.
                    let scale = self.core.canvas.scale;
                    self.core.canvas.set_scale(scale);
                    self.reset_edit_font();
                    SetWindowTextW(self.hwnd, wide(actions::app_title()).as_ptr());
                    InvalidateRect(self.edit, null(), 1);
                    self.core.dirty = true;
                    // The tooltip is in the language too.
                    self.sync_tray();
                }
                Cmd::Tray => self.sync_tray(),
                Cmd::PickRecDir => {
                    let current = crate::system::recorder::folder(&self.core.settings.rec_dir);
                    if let Some(dir) = folder_dialog(self.hwnd, tr().settings.record_pick, current.as_deref()) {
                        // A folder that holds something already: ask, since the files of this feature would join it.
                        let busy = crate::system::recorder::has_content(&dir);
                        let sure = !busy || {
                            let (text, caption) = (wide(tr().settings.record_dir_confirm), wide(tr().settings.sec_record));
                            MessageBoxW(self.hwnd, text.as_ptr(), caption.as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES
                        };
                        if sure {
                            // The default folder is kept as an empty setting, so that it follows the profile.
                            let chosen = dir.display().to_string();
                            let default = crate::system::recorder::default_dir().map(|d| d.display().to_string());
                            self.core.settings.rec_dir = if default.as_deref() == Some(chosen.as_str()) { String::new() } else { chosen };
                            self.core.settings.save_recording();
                        }
                    }
                    self.core.dirty = true;
                }
                Cmd::OpenRecDir => {
                    match crate::system::recorder::folder(&self.core.settings.rec_dir) {
                        Some(dir) if std::fs::create_dir_all(&dir).is_ok() => actions::shell_open(&dir.display().to_string()),
                        _ => self.core.toast(tr().settings.record_dir_failed.into()),
                    }
                }
                Cmd::Elevate => {
                    if actions::relaunch_elevated(true) {
                        // The new copy waits for this one to end, so this one must not stay in the tray.
                        self.quit();
                    } else {
                        self.core.toast(tr().app.elevate_cancelled.into());
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

    /// Shows the process aim mode picked, clearing the filter when it hides the process.
    unsafe fn reveal_aimed(&mut self, pid: u32, picked: bool) {
        use crate::pages::details::Reveal;
        self.core.navigate(0);
        self.place_edit();
        if picked {
            self.core.details_go_live();
        }
        let Some(page) = self.core.page_is_details() else { return };
        page.set_aiming(false);
        if picked {
            let mut found = page.reveal(pid);
            if found == Reveal::Filtered {
                // EN_CHANGE applies the empty filter.
                SetWindowTextW(self.edit, wide("").as_ptr());
                if let Some(page) = self.core.page_is_details() {
                    found = page.reveal(pid);
                }
            }
            if found != Reveal::Shown {
                self.core.toast((tr().app.pid_not_listed)(pid));
            }
        }
        self.core.dirty = true;
        self.after(true);
    }

    /// Shows the save dialog and writes the file there.
    unsafe fn save_file(&mut self, req: crate::pages::SaveRequest) {
        let Some(path) = save_dialog(self.hwnd, &req.name, req.kind, req.ext) else { return };
        let result = std::fs::File::create(&path).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            (req.write)(&mut w)?;
            std::io::Write::flush(&mut w)
        });
        let message = match result {
            Ok(()) => (tr().app.exported)(&path.display().to_string()),
            Err(e) => (tr().app.export_failed)(&e.to_string()),
        };
        self.core.toast(message);
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

    unsafe fn focus_for_press(&mut self, x: i32, y: i32) {
        if GetFocus() != self.edit || !self.core.search_rect().contains(x, y) {
            SetFocus(self.hwnd);
        }
    }

    unsafe fn dispatch(&mut self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Move { x, y } => self.fire(Ev::Move { x, y }),
                Out::Press { x, y, right } => {
                    self.focus_for_press(x, y);
                    self.fire(Ev::Down { x, y, right });
                }
                Out::Release { x } => self.fire(Ev::Up { x }),
                Out::Pan { dx, dy } => self.fire(Ev::Pan { dx, dy }),
                Out::Fling { vx, vy } => self.fire(Ev::Fling { vx, vy }),
            }
        }
    }

    /// Handles touch and pen. Returns false to leave the message to Windows.
    /// Contacts are captured to this window by the system.
    unsafe fn on_pointer(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> bool {
        let id = (wp & 0xFFFF) as u32;
        let flags = ((wp >> 16) & 0xFFFF) as u32;
        let mut kind: POINTER_INPUT_TYPE = 0;
        if GetPointerType(id, &mut kind) == 0 || (kind != PT_TOUCH && kind != PT_PEN) {
            return false;
        }
        self.core.touch = kind == PT_TOUCH;
        let mut p = POINT { x: lo(lp), y: hi(lp) };
        ScreenToClient(self.hwnd, &mut p);
        let at = (p.x, p.y);
        let t = self.started.elapsed().as_secs_f64();
        let mut out = Vec::new();
        match msg {
            WM_POINTERDOWN => {
                if !self.gesture.idle() || flags & POINTER_MESSAGE_FLAG_PRIMARY == 0 {
                    return true;
                }
                self.fire(Ev::Fling { vx: 0.0, vy: 0.0 });
                let slop = if kind == PT_TOUCH { TOUCH_SLOP } else { PEN_SLOP };
                let slop = self.core.canvas.px(slop);
                let pannable = self.core.pannable(p.x, p.y);
                let barrel = kind == PT_PEN && flags & POINTER_MESSAGE_FLAG_SECONDBUTTON != 0;
                self.gesture.down(id, at, t, Contact { slop, pannable, barrel }, &mut out);
                if self.gesture.pending() {
                    SetTimer(self.hwnd, TIMER_HOLD, HOLD_MS, None);
                }
            }
            WM_POINTERUPDATE if flags & POINTER_MESSAGE_FLAG_INCONTACT != 0 => {
                self.gesture.update(id, at, t, &mut out);
            }
            WM_POINTERUPDATE => {
                if kind == PT_PEN && self.gesture.idle() {
                    self.fire(Ev::Move { x: p.x, y: p.y });
                }
            }
            WM_POINTERUP => {
                let canceled = flags & POINTER_MESSAGE_FLAG_CANCELED != 0;
                self.gesture.up(id, t, canceled, &mut out);
            }
            WM_POINTERCAPTURECHANGED => self.gesture.up(id, t, true, &mut out),
            WM_POINTERLEAVE if self.gesture.idle() => self.fire(Ev::Leave),
            _ => {}
        }
        if !self.gesture.pending() {
            KillTimer(self.hwnd, TIMER_HOLD);
        }
        self.dispatch(out);
        true
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
        // Not IsZoomed: minimized or hidden, the window is not zoomed, but comes back maximized.
        s.maximized = self.core.maximized;
        s.save();
        for page in &mut self.core.pages {
            page.on_exit();
        }
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

fn caption_of(hit: u32) -> Option<Caption> {
    match hit {
        HTMINBUTTON => Some(Caption::Minimize),
        HTMAXBUTTON => Some(Caption::Maximize),
        HTCLOSE => Some(Caption::Close),
        _ => None,
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
                let cue: Vec<u16> = tr().app.filter_cue.encode_utf16().collect();
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
    // Registered messages have no fixed number to match on.
    if msg == taskbar_created() {
        app.tray.reset();
        return 0;
    }
    if msg == crate::app::instance::show_message() {
        app.reveal();
        return 0;
    }

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
            let caption = caption_of(wp as u32);
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
            // A tap has no hover before it, so trust the hit test here.
            app.pressed_caption = caption_of(wp as u32);
            0
        }
        WM_NCLBUTTONUP if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            if let Some(c) = app.pressed_caption.take() {
                if caption_of(wp as u32) == Some(c) {
                    app.caption_action(c);
                }
            }
            0
        }
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERLEAVE | WM_POINTERCAPTURECHANGED => {
            if app.on_pointer(msg, wp, lp) {
                0
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
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
            // Hidden in the tray, a size change (a display or DPI change) must not make the
            // sampler read everything again; `reveal` takes care of that.
            if !app.hidden && app.core.shared.minimized.swap(false, Ordering::Relaxed) {
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
                x: (360.0 * scale) as i32,
                y: (420.0 * scale) as i32,
            };
            0
        }
        WM_DPICHANGED => {
            let scale = hi(wp as isize) as f32 / 96.0;
            app.core.canvas.set_scale(scale);
            app.reset_edit_font();
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
                Cursor::SizeNS => IDC_SIZENS,
                Cursor::IBeam => IDC_IBEAM,
            };
            SetCursor(LoadCursorW(null_mut(), id));
            1
        }
        WM_MOUSEMOVE => {
            app.core.touch = false;
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
            app.core.touch = false;
            app.focus_for_press(lo(lp), hi(lp));
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
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let horizontal = msg == WM_MOUSEHWHEEL || shift;
            let notches = delta as f32 / 120.0;
            let ev = if horizontal {
                let sign = if msg == WM_MOUSEHWHEEL { -1.0 } else { 1.0 };
                Ev::Wheel { dx: notches * sign, dy: 0.0, ctrl }
            } else {
                Ev::Wheel { dx: 0.0, dy: notches, ctrl }
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
        WM_TIMER if wp == TIMER_HOLD => {
            KillTimer(hwnd, TIMER_HOLD);
            let mut out = Vec::new();
            app.gesture.hold(&mut out);
            app.dispatch(out);
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
            if app.core.captures_text() {
                if let Some(ch) = char::from_u32(wp as u32) {
                    app.fire(Ev::Char { ch });
                }
                return 0;
            }
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
                app.tray.update(tray::levels(&snap));
                app.after(false);
            }
            0
        }
        WM_APP_TRAY => {
            match tray::event(wp, lp) {
                // A click always shows the window: hiding is the close button's.
                Some(tray::Event::Select) => app.reveal(),
                Some(tray::Event::Menu(x, y)) => match app.tray.menu(x, y) {
                    Some(tray::Command::Show) => app.reveal(),
                    Some(tray::Command::Exit) => app.quit(),
                    None => {}
                },
                None => {}
            }
            0
        }
        crate::app::aim::WM_APP_AIM => {
            app.reveal_aimed(wp as u32, lp == 1);
            0
        }
        WM_APP_ENRICHED => {
            app.core.shared.enrich_posted.store(false, Ordering::Release);
            app.core.on_enriched();
            app.after(false);
            0
        }
        WM_CLOSE => {
            // With an icon to come back by, the close button only hides the window.
            if !app.quitting && app.core.settings.tray && app.tray.shown() {
                if !app.hidden {
                    app.conceal();
                }
                return 0;
            }
            app.tray.sync(Want::None);
            app.save();
            // The recording's session ends here, before the process does.
            app.core.shared.end_recording(false);
            DestroyWindow(hwnd);
            0
        }
        WM_ENDSESSION => {
            // Windows is shutting down: close the recording so it is not taken for a crash.
            if wp != 0 {
                app.save();
                app.core.shared.end_recording(true);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
