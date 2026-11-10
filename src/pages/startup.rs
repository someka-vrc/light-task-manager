//! The startup apps page: programs started at sign-in from the Run keys and
//! the Startup folders, enabled or disabled the same way Windows does it
//! (the Explorer\StartupApproved keys).

use super::widgets::*;
use super::{icons, Cx, Ev, Nav, Page};
use crate::actions;
use crate::gfx::{Canvas, Rect};
use crate::model::ImageInfo;
use crate::sampler::{load_icon, version_string};
use crate::settings::Settings;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::UI::Shell::*;

windows_targets::link!("kernel32.dll" "system" fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32);

static COLUMNS: [Column; 5] = [
    col("名前", 220, false),
    col("発行元", 200, false),
    col("状態", 80, false),
    col("場所", 170, false),
    col("コマンド", 360, false),
];

const ACT_TOGGLE: u32 = 1;
const ACT_LOCATION: u32 = 2;
const ACT_PROPERTIES: u32 = 3;
const ACT_COPY: u32 = 4;
const ACT_FOLDER: u32 = 5;

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN32: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\";

#[derive(Clone)]
struct Item {
    id: u64,
    name: String,
    command: String,
    /// The executable or shortcut, for the icon and "open file location".
    path: String,
    location: &'static str,
    enabled: bool,
    publisher: String,
    approved_root: HKEY,
    approved_key: String,
    value: String,
    image: Option<Arc<ImageInfo>>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn expand(s: &str) -> String {
    let src = wide(s);
    let mut buf = vec![0u16; 2048];
    let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize > buf.len() {
        return s.to_string();
    }
    from_wide(&buf)
}

/// The program part of a command line.
fn program_of(command: &str) -> String {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return expand(rest.split('"').next().unwrap_or(""));
    }
    let lower = c.to_lowercase();
    if let Some(i) = lower.find(".exe") {
        return expand(&c[..i + 4]);
    }
    expand(c.split_whitespace().next().unwrap_or(""))
}

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Reads the StartupApproved flag: an even first byte (or no value) means enabled.
fn approved(root: HKEY, key: &str, value: &str) -> bool {
    let (k, v) = (wide(&format!("{APPROVED}{key}")), wide(value));
    let mut data = [0u8; 12];
    let mut size = 12u32;
    let r = unsafe {
        RegGetValueW(root, k.as_ptr(), v.as_ptr(), RRF_RT_REG_BINARY, null_mut(), data.as_mut_ptr().cast(), &mut size)
    };
    r != 0 || data[0] & 1 == 0
}

fn set_approved(root: HKEY, key: &str, value: &str, enable: bool) -> Result<(), u32> {
    let k = wide(&format!("{APPROVED}{key}"));
    let v = wide(value);
    let mut data = [0u8; 12];
    data[0] = if enable { 2 } else { 3 };
    if !enable {
        let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        unsafe { GetSystemTimeAsFileTime(&mut ft) };
        data[4..8].copy_from_slice(&ft.dwLowDateTime.to_le_bytes());
        data[8..12].copy_from_slice(&ft.dwHighDateTime.to_le_bytes());
    }
    unsafe {
        let mut hk: HKEY = null_mut();
        let r = RegCreateKeyExW(root, k.as_ptr(), 0, null(), 0, KEY_SET_VALUE, null(), &mut hk, null_mut());
        if r != 0 {
            return Err(r);
        }
        let r = RegSetValueExW(hk, v.as_ptr(), 0, REG_BINARY, data.as_ptr(), 12);
        RegCloseKey(hk);
        if r != 0 {
            return Err(r);
        }
    }
    Ok(())
}

fn run_key(out: &mut Vec<Item>, root: HKEY, sub: &str, location: &'static str, approved_key: &str) {
    unsafe {
        let mut hk: HKEY = null_mut();
        if RegOpenKeyExW(root, wide(sub).as_ptr(), 0, KEY_READ, &mut hk) != 0 {
            return;
        }
        let mut index = 0;
        loop {
            let mut name = vec![0u16; 512];
            let mut name_len = name.len() as u32;
            let mut data = vec![0u16; 4096];
            let mut data_len = (data.len() * 2) as u32;
            let mut kind = 0u32;
            let r = RegEnumValueW(hk, index, name.as_mut_ptr(), &mut name_len, null(), &mut kind, data.as_mut_ptr().cast(), &mut data_len);
            index += 1;
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r != 0 || !(kind == REG_SZ || kind == REG_EXPAND_SZ) {
                continue;
            }
            let value = String::from_utf16_lossy(&name[..name_len as usize]);
            let command = from_wide(&data[..(data_len as usize / 2).min(data.len())]);
            if command.trim().is_empty() {
                continue;
            }
            out.push(Item {
                id: fnv(&format!("{location}\u{1}{value}")),
                name: value.clone(),
                path: program_of(&command),
                command,
                location,
                enabled: approved(root, approved_key, &value),
                publisher: String::new(),
                approved_root: root,
                approved_key: approved_key.to_string(),
                value,
                image: None,
            });
        }
        RegCloseKey(hk);
    }
}

fn known_folder(id: &windows_sys::core::GUID) -> Option<String> {
    unsafe {
        let mut p = null_mut();
        if SHGetKnownFolderPath(id, 0, null_mut(), &mut p) < 0 || p.is_null() {
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p as *const _);
        Some(s)
    }
}

/// Pulls the target path out of a .lnk file without COM (MS-SHLLINK LinkInfo).
fn shortcut_target(path: &str) -> Option<String> {
    let b = std::fs::read(path).ok()?;
    let u32_at = |at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let flags = u32_at(0x14)?;
    let mut at = 0x4C;
    if flags & 1 != 0 {
        let len = u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize;
        at += 2 + len;
    }
    if flags & 2 == 0 {
        return None;
    }
    let header = u32_at(at + 4)? as usize;
    if header >= 0x24 {
        let off = u32_at(at + 28)? as usize;
        if off != 0 {
            let start = at + off;
            let mut chars = Vec::new();
            let mut i = start;
            while let (Some(lo), Some(hi)) = (b.get(i), b.get(i + 1)) {
                let c = u16::from_le_bytes([*lo, *hi]);
                if c == 0 {
                    break;
                }
                chars.push(c);
                i += 2;
            }
            if !chars.is_empty() {
                return Some(String::from_utf16_lossy(&chars));
            }
        }
    }
    let off = u32_at(at + 16)? as usize;
    let start = at + off;
    let end = b[start..].iter().position(|c| *c == 0)? + start;
    let s = String::from_utf8_lossy(&b[start..end]).to_string();
    (!s.is_empty()).then_some(s)
}

fn folder(out: &mut Vec<Item>, id: &windows_sys::core::GUID, root: HKEY, location: &'static str) {
    let Some(dir) = known_folder(id) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        if file.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let full = e.path().to_string_lossy().to_string();
        let name = file.rsplit_once('.').map_or(file.clone(), |(n, _)| n.to_string());
        let target = if file.to_lowercase().ends_with(".lnk") { shortcut_target(&full) } else { None };
        out.push(Item {
            id: fnv(&format!("{location}\u{1}{file}")),
            name,
            command: target.clone().unwrap_or_else(|| full.clone()),
            path: target.unwrap_or(full),
            location,
            enabled: approved(root, "StartupFolder", &file),
            publisher: String::new(),
            approved_root: root,
            approved_key: "StartupFolder".into(),
            value: file,
            image: None,
        });
    }
}

fn collect(icon_px: i32) -> Vec<Item> {
    let mut out = Vec::new();
    run_key(&mut out, HKEY_CURRENT_USER, RUN, "レジストリ (ユーザー)", "Run");
    run_key(&mut out, HKEY_LOCAL_MACHINE, RUN, "レジストリ (コンピューター)", "Run");
    run_key(&mut out, HKEY_LOCAL_MACHINE, RUN32, "レジストリ (32 ビット)", "Run32");
    folder(&mut out, &FOLDERID_Startup, HKEY_CURRENT_USER, "スタートアップ フォルダー");
    folder(&mut out, &FOLDERID_CommonStartup, HKEY_LOCAL_MACHINE, "スタートアップ フォルダー (共通)");
    for it in &mut out {
        let w = wide(&it.path);
        it.publisher = version_string(&w, "CompanyName");
        let description = version_string(&w, "FileDescription");
        let info = ImageInfo { path: it.path.clone(), description: OnceLock::new(), icon: OnceLock::new() };
        let _ = info.description.set(Arc::from(description));
        let _ = info.icon.set(load_icon(&w, icon_px));
        it.image = Some(Arc::new(info));
    }
    out
}

pub struct StartupPage {
    table: Table,
    items: Vec<Item>,
    toolbar: Toolbar,
    menu: Option<Menu>,
    loaded: bool,
}

impl StartupPage {
    pub fn new() -> Self {
        let mut table = Table::new(&COLUMNS, 0, false);
        table.empty_text = "スタートアップ アプリはありません";
        Self { table, items: Vec::new(), toolbar: Toolbar::default(), menu: None, loaded: false }
    }

    fn refresh(&mut self, cx: &Cx) {
        self.items = collect(cx.shared.icon_px.load(Ordering::Relaxed));
        let rows = self
            .items
            .iter()
            .map(|it| {
                let state = if it.enabled { "有効" } else { "無効" };
                let mut row = Row::new(
                    it.id,
                    vec![it.name.clone(), it.publisher.clone(), state.into(), it.location.into(), it.command.clone()],
                    vec![text(&it.name), text(&it.publisher), text(state), text(it.location), text(&it.command)],
                );
                row.icon = it.image.clone();
                row.dim = !it.enabled;
                row
            })
            .collect();
        self.table.set_rows(rows);
        self.loaded = true;
    }

    fn selected(&self) -> Option<&Item> {
        let id = self.table.selected?;
        self.items.iter().find(|i| i.id == id)
    }

    fn buttons(&self) -> [ButtonSpec; 2] {
        let s = self.selected();
        let toggle = match s {
            Some(i) if !i.enabled => btn(icons::PLAY, "有効にする", true),
            Some(_) => btn(icons::BLOCK, "無効にする", true),
            None => btn(icons::BLOCK, "無効にする", false),
        };
        [btn(icons::FOLDER, "ファイルの場所を開く", s.is_some_and(|i| !i.path.is_empty())), toggle]
    }

    fn run(&mut self, act: u32, cx: &mut Cx) {
        match act {
            ACT_FOLDER => {
                if let Some(dir) = known_folder(&FOLDERID_Startup) {
                    actions::shell_open(&dir);
                }
                return;
            }
            ACT_COPY => {
                if let Some(t) = self.table.selected_text() {
                    actions::set_clipboard(&t);
                }
                return;
            }
            _ => {}
        }
        let Some(it) = self.selected().cloned() else { return };
        match act {
            ACT_TOGGLE => {
                match set_approved(it.approved_root, &it.approved_key, &it.value, !it.enabled) {
                    Ok(()) => self.refresh(cx),
                    Err(e) => cx.cmds.push(super::Cmd::Toast(format!("{} を変更できませんでした。{}", it.name, error_text(e)))),
                }
                *cx.dirty = true;
            }
            ACT_LOCATION => actions::open_location(&it.path),
            ACT_PROPERTIES => actions::properties(&it.path),
            _ => {}
        }
    }
}

impl Page for StartupPage {
    fn nav(&self) -> Nav {
        Nav { icon: icons::STARTUP, label: "スタートアップ アプリ", bottom: false }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.table.set_filter(text);
        *cx.dirty = true;
    }

    fn on_show(&mut self, cx: &mut Cx) {
        self.refresh(cx);
        *cx.dirty = true;
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.menu = None;
    }

    fn animating(&self) -> bool {
        self.table.animating()
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        self.menu.is_none() && self.table.pannable(x, y)
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Some(m) = &mut self.menu {
            if !matches!(ev, Ev::Tick { .. }) {
                match m.event(ev, cx.dirty) {
                    MenuOut::Stay => {}
                    MenuOut::Close => {
                        self.menu = None;
                        *cx.dirty = true;
                    }
                    MenuOut::Pick(id) => {
                        self.menu = None;
                        *cx.dirty = true;
                        self.run(id, cx);
                    }
                }
                return;
            }
        }
        match *ev {
            Ev::Move { x, y } => {
                if self.toolbar.hover_at(x, y) {
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } if self.toolbar.hit(x, y).is_some() => {
                let i = self.toolbar.hit(x, y).unwrap();
                if self.buttons()[i].enabled {
                    self.run([ACT_LOCATION, ACT_TOGGLE][i], cx);
                }
                return;
            }
            Ev::Key { vk: VK_C, ctrl: true } => return self.run(ACT_COPY, cx),
            Ev::Key { vk: VK_DELETE, .. } => return self.run(ACT_TOGGLE, cx),
            Ev::Key { vk: VK_F5, .. } => {
                self.refresh(cx);
                *cx.dirty = true;
                return;
            }
            _ => {}
        }
        match self.table.event(ev, cx.dirty) {
            TableOut::Context { x, y } => {
                let enabled = self.selected().is_some_and(|i| i.enabled);
                let has_path = self.selected().is_some_and(|i| !i.path.is_empty());
                self.menu = Some(Menu::new(
                    x,
                    y,
                    vec![
                        item(if enabled { "無効にする" } else { "有効にする" }, ACT_TOGGLE, true),
                        item("ファイルの場所を開く", ACT_LOCATION, has_path),
                        item("プロパティ", ACT_PROPERTIES, has_path),
                        item("行をコピー", ACT_COPY, true),
                        item("スタートアップ フォルダーを開く", ACT_FOLDER, true),
                    ],
                ));
                *cx.dirty = true;
            }
            TableOut::Activate => self.run(ACT_TOGGLE, cx),
            _ => {}
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, _settings: &Settings) {
        let (bar, body) = split(c, area);
        let enabled = self.items.iter().filter(|i| i.enabled).count();
        let info = if !self.loaded {
            String::new()
        } else if self.table.shown() == self.table.len() {
            format!("{} 項目   有効 {enabled}", self.table.len())
        } else {
            format!("{} / {} 項目", self.table.shown(), self.table.len())
        };
        let specs = self.buttons();
        let left = self.toolbar.paint(c, bar, &specs);
        paint_title(c, bar, "スタートアップ アプリ", &info, left);
        self.table.paint(c, body);
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
    }
}
