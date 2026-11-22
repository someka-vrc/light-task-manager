//! The icon in the notification area.
//!
//! What the icon should be comes from the settings alone (`Want::of`), and `Tray::sync` is the
//! only way to change it: it compares what is wanted with what is registered and applies the
//! difference, so calling it again changes nothing. Nothing else calls `Shell_NotifyIconW`.
//! The values of each snapshot go through `Tray::update`, which redraws within what was
//! last wanted and never adds an icon that the settings do not want.

use crate::app::icon::{self, Levels};
use crate::settings::Settings;
use crate::system::model::Snapshot;
use crate::system::win::wide;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The message the icon sends to the window.
pub const WM_APP_TRAY: u32 = WM_APP + 4;
/// The icon's id among the window's icons.
const ID: u32 = 1;
/// Enter on the icon (windows-sys lacks the name).
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

/// What the notification area should show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want {
    None,
    Plain,
    Graph,
}

impl Want {
    pub fn of(s: &Settings) -> Want {
        match (s.tray, s.tray_graph) {
            (false, _) => Want::None,
            (true, false) => Want::Plain,
            (true, true) => Want::Graph,
        }
    }
}

/// The values of the bar graph and the tooltip, from a snapshot.
pub fn levels(snap: &Snapshot) -> Option<Levels> {
    let s = &snap.sys;
    if s.mem.total == 0 {
        return None;
    }
    let used = s.mem.total.saturating_sub(s.mem.available);
    let gpu = s.gpus.iter().map(|g| g.util).fold(None, |m: Option<f32>, u| Some(m.map_or(u, |m| m.max(u))));
    Some(Levels { cpu: s.cpu.total, mem: (used as f64 / s.mem.total as f64 * 100.0) as f32, gpu })
}

/// What a registered icon shows; a new one is drawn only when this changes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Look {
    want: Want,
    size: i32,
    bars: [i32; 3],
}

pub struct Tray {
    hwnd: HWND,
    /// What the settings last asked for.
    want: Want,
    /// The icon is registered with the shell.
    added: bool,
    /// What the icon in `icon` shows, and the tooltip in force.
    look: Option<Look>,
    tip: String,
    icon: HICON,
    levels: Option<Levels>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        Self { hwnd, want: Want::None, added: false, look: None, tip: String::new(), icon: null_mut(), levels: None }
    }

    /// The icon is in the notification area.
    pub fn shown(&self) -> bool {
        self.added
    }

    /// Brings the icon in line with `want`.
    pub fn sync(&mut self, want: Want) {
        self.want = want;
        self.apply();
    }

    /// Explorer started again and forgot the icon: register it anew.
    pub fn reset(&mut self) {
        self.added = false;
        self.apply();
    }

    /// Takes the values of a new snapshot.
    pub fn update(&mut self, levels: Option<Levels>) {
        if levels.is_some() {
            self.levels = levels;
        }
        if self.want != Want::None {
            self.apply();
        }
    }

    fn apply(&mut self) {
        if self.want == Want::None {
            if self.added {
                let data = self.data(0);
                unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                self.added = false;
            }
            self.drop_icon();
            self.look = None;
            self.tip.clear();
            return;
        }
        // The shell draws its icons at the small icon size of the system DPI.
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16);
        let bars = if self.want == Want::Graph { icon::bar_heights(self.levels, size) } else { [0; 3] };
        let look = Look { want: self.want, size, bars };
        let tip = tooltip(self.levels);
        if self.added && self.look == Some(look) && self.tip == tip {
            return;
        }
        let fresh = if self.look != Some(look) || self.icon.is_null() {
            let px = match self.want {
                Want::Graph => icon::graph_pixels(size, bars),
                _ => icon::app_pixels(size),
            };
            icon::to_icon(&px, size)
        } else {
            null_mut()
        };
        let old = self.icon;
        if !fresh.is_null() {
            self.icon = fresh;
        }
        self.tip = tip;
        let data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
        unsafe {
            if self.added {
                Shell_NotifyIconW(NIM_MODIFY, &data);
            } else {
                // Right after sign-in the shell may not be ready; the icon then comes with
                // `TaskbarCreated`. An icon left from before is taken over.
                let ok = Shell_NotifyIconW(NIM_ADD, &data) != 0 || Shell_NotifyIconW(NIM_MODIFY, &data) != 0;
                if ok {
                    // Without this the shell sends the old kind of messages, and clicks go astray.
                    Shell_NotifyIconW(NIM_SETVERSION, &data);
                    self.added = true;
                }
            }
            // The old icon goes only once the shell holds the new one.
            if !fresh.is_null() && !old.is_null() {
                DestroyIcon(old);
            }
        }
        self.look = Some(look);
    }

    fn data(&self, flags: u32) -> NOTIFYICONDATAW {
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd;
        d.uID = ID;
        d.uFlags = flags;
        d.uCallbackMessage = WM_APP_TRAY;
        d.hIcon = self.icon;
        d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let tip = wide(&self.tip);
        let n = (tip.len() - 1).min(d.szTip.len() - 1);
        d.szTip[..n].copy_from_slice(&tip[..n]);
        d
    }

    fn drop_icon(&mut self) {
        if !self.icon.is_null() {
            unsafe { DestroyIcon(self.icon) };
            self.icon = null_mut();
        }
    }

    /// The icon's menu at a point of the screen. Returns the command picked.
    pub fn menu(&self, x: i32, y: i32) -> Option<Command> {
        let t = crate::i18n::tr();
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return None;
            }
            AppendMenuW(menu, MF_STRING, Command::Show as usize, wide(t.app.tray_show).as_ptr());
            AppendMenuW(menu, MF_STRING, Command::Exit as usize, wide(t.app.tray_exit).as_ptr());
            SetMenuDefaultItem(menu, Command::Show as u32, 0);
            // Without this the menu does not close when the user clicks elsewhere.
            SetForegroundWindow(self.hwnd);
            let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
            let picked = TrackPopupMenuEx(menu, align | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, x, y, self.hwnd, null_mut());
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            match picked {
                v if v == Command::Show as i32 => Some(Command::Show),
                v if v == Command::Exit as i32 => Some(Command::Exit),
                _ => None,
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.sync(Want::None);
    }
}

/// The commands of the icon's menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Show = 1,
    Exit = 2,
}

/// The tooltip: the app's name, then the CPU and memory use.
fn tooltip(levels: Option<Levels>) -> String {
    let name = crate::system::actions::app_title();
    match levels {
        Some(l) => format!("{name}\nCPU {:.0}%  {} {:.0}%", l.cpu, crate::i18n::tr().common.memory, l.mem),
        None => name.to_string(),
    }
}

/// What the icon's message says happened.
pub enum Event {
    /// A click or Enter on the icon.
    Select,
    /// A right-click or the menu key, at this point of the screen.
    Menu(i32, i32),
}

/// Reads the icon's message (`NOTIFYICON_VERSION_4`: the event in the low word of `lp`, the
/// point in `wp`).
pub fn event(wp: WPARAM, lp: LPARAM) -> Option<Event> {
    let x = (wp & 0xFFFF) as u16 as i16 as i32;
    let y = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
    match (lp & 0xFFFF) as u32 {
        NIN_SELECT | NIN_KEYSELECT => Some(Event::Select),
        WM_CONTEXTMENU => Some(Event::Menu(x, y)),
        _ => None,
    }
}
