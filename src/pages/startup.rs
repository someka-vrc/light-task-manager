//! The startup apps page: programs started at sign-in from the Run keys and
//! the Startup folders, enabled or disabled the same way Windows does it
//! (the Explorer\StartupApproved keys), plus the startup tasks of packaged apps.

mod packaged;

use super::{Cx, Ev, Nav, Page};
use crate::i18n::{tr, Text};
use crate::settings::Settings;
use crate::system::actions;
use crate::system::model::ImageInfo;
use crate::system::sampler::{load_icon, version_string};
use crate::system::win::{from_wide, wide};
use crate::ui::gfx::{Canvas, Rect};
use crate::ui::icons;
use crate::ui::keys::*;
use crate::ui::widgets::*;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::UI::Shell::*;

windows_targets::link!("kernel32.dll" "system" fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32);
windows_targets::link!("kernel32.dll" "system" fn MultiByteToWideChar(codepage: u32, flags: u32, src: *const u8, srclen: i32, dst: *mut u16, dstlen: i32) -> i32);
const CP_ACP: u32 = 0;

static COLUMNS: [Column; 5] = [
    col(|s| s.common.name, "Item.name", 220, false),
    col(|s| s.common.publisher, "Item.publisher", 200, false),
    col(|s| s.common.status, "Item.enabled", 80, false),
    col(|s| s.startup.location, "Item.location", 170, false),
    col(|s| s.startup.command, "Item.command", 360, false),
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
    location: Text,
    enabled: bool,
    publisher: String,
    toggle: Toggle,
    image: Option<Arc<ImageInfo>>,
}

/// Where the enabled flag lives.
#[derive(Clone)]
enum Toggle {
    /// A value under Explorer\StartupApproved\{key}.
    Approved { root: HKEY, key: String, value: String },
    /// A packaged app's startup task (`<family>\<task id>`).
    Packaged(String),
}

impl Toggle {
    fn set(&self, enable: bool) -> Result<(), u32> {
        match self {
            Toggle::Approved { root, key, value } => set_approved(*root, key, value, enable),
            Toggle::Packaged(key) => packaged::set_enabled(key, enable),
        }
    }
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

fn run_key(out: &mut Vec<Item>, root: HKEY, sub: &str, location: Text, approved_key: &str) {
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
                id: fnv(&format!("{}\u{1}{value}", location(crate::i18n::Lang::En.strings()))),
                name: value.clone(),
                path: program_of(&command),
                command,
                location,
                enabled: approved(root, approved_key, &value),
                publisher: String::new(),
                toggle: Toggle::Approved { root, key: approved_key.to_string(), value },
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

/// Decodes text in the system ANSI code page (e.g. CP932), as older .lnk fields are stored.
fn ansi(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf)
    }
}

/// A .lnk file's target and arguments, read without COM (MS-SHLLINK).
struct Shortcut {
    target: String,
    args: String,
}

fn shortcut_target(path: &str) -> Option<Shortcut> {
    let b = std::fs::read(path).ok()?;
    let u16_at = |at: usize| b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    // NUL-terminated strings inside LinkInfo and ExtraData.
    let utf16z = |start: usize| {
        let mut chars = Vec::new();
        let mut i = start;
        while let Some(c) = u16_at(i) {
            if c == 0 {
                break;
            }
            chars.push(c);
            i += 2;
        }
        String::from_utf16_lossy(&chars)
    };
    let ansiz = |start: usize| {
        let s = b.get(start..).unwrap_or(&[]);
        let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
        ansi(&s[..end])
    };

    let flags = u32_at(0x14)?;
    let unicode = flags & 0x80 != 0;
    let mut at = 0x4C;
    if flags & 0x1 != 0 {
        at += 2 + u16_at(at)? as usize;
    }

    let mut target = String::new();
    if flags & 0x2 != 0 {
        let size = u32_at(at)? as usize;
        let header = u32_at(at + 4)? as usize;
        let info_flags = u32_at(at + 8)?;
        let field = |o: usize| u32_at(at + o).map(|v| v as usize).filter(|v| *v != 0);
        let (base, suffix) = if header >= 0x24 {
            (field(28).map(|o| utf16z(at + o)), field(32).map(|o| utf16z(at + o)))
        } else {
            (None, None)
        };
        let base = base
            .filter(|s| !s.is_empty())
            .or_else(|| (info_flags & 1 != 0).then(|| field(16).map(|o| ansiz(at + o))).flatten());
        let suffix = suffix.filter(|s| !s.is_empty()).or_else(|| field(24).map(|o| ansiz(at + o)));
        if let Some(base) = base {
            target = base;
            if let Some(suffix) = suffix.filter(|s| !s.is_empty()) {
                if !target.ends_with('\\') {
                    target.push('\\');
                }
                target.push_str(&suffix);
            }
        }
        at += size;
    }

    // StringData: name, relative path, working dir, arguments, icon location (each optional).
    let mut read = |present: bool| -> Option<String> {
        if !present {
            return Some(String::new());
        }
        let count = u16_at(at)? as usize;
        at += 2;
        let s = if unicode {
            let raw = b.get(at..at + count * 2)?;
            at += count * 2;
            String::from_utf16_lossy(&raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>())
        } else {
            let raw = b.get(at..at + count)?;
            at += count;
            ansi(raw)
        };
        Some(s)
    };
    let _name = read(flags & 0x04 != 0);
    let relative = read(flags & 0x08 != 0).unwrap_or_default();
    let _dir = read(flags & 0x10 != 0);
    let args = read(flags & 0x20 != 0).unwrap_or_default();
    let icon = read(flags & 0x40 != 0);

    // Without LinkInfo, fall back to the EnvironmentVariableDataBlock in ExtraData.
    if target.is_empty() && icon.is_some() {
        while let (Some(size), Some(sig)) = (u32_at(at), u32_at(at + 4)) {
            let size = size as usize;
            if size < 8 {
                break;
            }
            if sig == 0xA000_0001 && size >= 0x314 {
                let wide = utf16z(at + 0x10C);
                target = if wide.is_empty() { ansiz(at + 8) } else { wide };
                target = expand(&target);
                break;
            }
            at += size;
        }
    }

    // Then the relative path. Installers often write it against the common Start Menu, so a copy
    // placed elsewhere climbs to the wrong folder; retry from the drive root in that case.
    // (If neither exists, the caller shows the .lnk itself.)
    if target.is_empty() && !relative.is_empty() {
        let rel = std::path::Path::new(&relative);
        let lnk_dir = std::path::Path::new(path).parent()?;
        let rooted: std::path::PathBuf =
            rel.components().skip_while(|c| matches!(c, std::path::Component::ParentDir)).collect();
        let drive_root = lnk_dir.ancestors().last()?;
        for candidate in [normalize(&lnk_dir.join(rel)), normalize(&drive_root.join(rooted))] {
            if std::path::Path::new(&candidate).exists() {
                target = candidate;
                break;
            }
        }
    }
    (!target.is_empty()).then_some(Shortcut { target, args })
}

/// Resolves `.` and `..` without touching the file system (and without the `\\?\` prefix).
fn normalize(p: &std::path::Path) -> String {
    use std::path::Component;
    let mut prefix = String::new();
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(x) => prefix = x.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(x) => parts.push(x.to_string_lossy().into_owned()),
        }
    }
    format!("{prefix}\\{}", parts.join("\\"))
}

fn folder(out: &mut Vec<Item>, id: &windows_sys::core::GUID, root: HKEY, location: Text) {
    let Some(dir) = known_folder(id) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        if file.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let full = e.path().to_string_lossy().to_string();
        let name = file.rsplit_once('.').map_or(file.clone(), |(n, _)| n.to_string());
        let link = if file.to_lowercase().ends_with(".lnk") { shortcut_target(&full) } else { None };
        let (command, path) = match link {
            Some(l) => {
                let quoted = if l.target.contains(' ') { format!("\"{}\"", l.target) } else { l.target.clone() };
                let command = if l.args.is_empty() { quoted } else { format!("{quoted} {}", l.args) };
                (command, l.target)
            }
            None => (full.clone(), full),
        };
        out.push(Item {
            id: fnv(&format!("{}\u{1}{file}", location(crate::i18n::Lang::En.strings()))),
            name,
            command,
            path,
            location,
            enabled: approved(root, "StartupFolder", &file),
            publisher: String::new(),
            toggle: Toggle::Approved { root, key: "StartupFolder".into(), value: file },
            image: None,
        });
    }
}

fn collect(icon_px: i32) -> Vec<Item> {
    let mut out = Vec::new();
    run_key(&mut out, HKEY_CURRENT_USER, RUN, |s| s.startup.reg_user, "Run");
    run_key(&mut out, HKEY_LOCAL_MACHINE, RUN, |s| s.startup.reg_machine, "Run");
    run_key(&mut out, HKEY_LOCAL_MACHINE, RUN32, |s| s.startup.reg_32, "Run32");
    folder(&mut out, &FOLDERID_Startup, HKEY_CURRENT_USER, |s| s.startup.folder);
    folder(&mut out, &FOLDERID_CommonStartup, HKEY_LOCAL_MACHINE, |s| s.startup.folder_common);
    for t in packaged::tasks() {
        let location: Text = |s| s.startup.packaged;
        out.push(Item {
            id: fnv(&format!("{}\u{1}{}", location(crate::i18n::Lang::En.strings()), t.key)),
            name: t.name,
            command: if t.exe.contains(' ') { format!("\"{}\"", t.exe) } else { t.exe.clone() },
            path: t.exe,
            location,
            enabled: t.enabled,
            publisher: t.publisher,
            toggle: Toggle::Packaged(t.key),
            image: None,
        });
    }
    for it in &mut out {
        let w = wide(&it.path);
        let company = version_string(&w, "CompanyName");
        if !company.is_empty() {
            it.publisher = company;
        }
        let description = version_string(&w, "FileDescription");
        let info = ImageInfo { path: it.path.clone(), description: OnceLock::new(), company: OnceLock::new(), icon: OnceLock::new() };
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
        table.empty_text = |s| s.startup.empty;
        Self { table, items: Vec::new(), toolbar: Toolbar::default(), menu: None, loaded: false }
    }

    fn refresh(&mut self, cx: &Cx) {
        self.items = collect(cx.shared.icon_px.load(Ordering::Relaxed));
        let rows = self
            .items
            .iter()
            .map(|it| {
                let state = if it.enabled { tr().common.enabled } else { tr().common.disabled };
                let location = (it.location)(tr());
                let mut row = Row::new(
                    it.id,
                    vec![it.name.clone(), it.publisher.clone(), state.into(), location.into(), it.command.clone()],
                    vec![text(&it.name), text(&it.publisher), text(state), text(location), text(&it.command)],
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
            Some(i) if !i.enabled => btn(icons::PLAY, tr().common.enable, true),
            Some(_) => btn(icons::BLOCK, tr().common.disable, true),
            None => btn(icons::BLOCK, tr().common.disable, false),
        };
        [btn(icons::FOLDER, tr().common.open_location, s.is_some_and(|i| !i.path.is_empty())), toggle]
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
                if let Some(t) = self.table.selected_text(cx.settings.copy_format) {
                    actions::set_clipboard(&t);
                }
                return;
            }
            _ => {}
        }
        let Some(it) = self.selected().cloned() else { return };
        match act {
            ACT_TOGGLE => {
                match it.toggle.set(!it.enabled) {
                    Ok(()) => self.refresh(cx),
                    Err(e) => cx.cmds.push(super::Cmd::Toast((tr().startup.change_failed)(&it.name, &error_text(e)))),
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
        Nav { icon: icons::STARTUP, label: tr().startup.title, bottom: false }
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
                        item(if enabled { tr().common.disable } else { tr().common.enable }, ACT_TOGGLE, true),
                        item(tr().common.open_location, ACT_LOCATION, has_path),
                        item(tr().common.properties, ACT_PROPERTIES, has_path),
                        item(tr().common.copy_row, ACT_COPY, true),
                        item(tr().startup.open_folder, ACT_FOLDER, true),
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
            (tr().startup.info)(self.table.len(), enabled)
        } else {
            (tr().startup.shown)(self.table.shown(), self.table.len())
        };
        let specs = self.buttons();
        let left = self.toolbar.paint(c, bar, &specs);
        paint_title(c, bar, tr().startup.title, &info, left);
        self.table.paint(c, body);
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
    }
}
