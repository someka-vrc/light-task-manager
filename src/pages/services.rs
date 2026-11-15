//! The services page: every Win32 service with its state, and start / stop / restart.

use super::{Cx, Ev, Nav, Page};
use crate::i18n::tr;
use crate::settings::Settings;
use crate::system::actions;
use crate::system::win::wide;
use crate::ui::gfx::{Canvas, Rect};
use crate::ui::icons;
use crate::ui::keys::*;
use crate::ui::widgets::*;
use std::collections::HashMap;
use std::ptr::null;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Services::*;

const REFRESH_SECONDS: u64 = 2;

static COLUMNS: [Column; 6] = [
    col(|s| s.common.name, "Service.name", 200, false),
    col(|_| "PID", "Service.pid", 72, true),
    col(|s| s.common.description, "Service.display", 260, false),
    col(|s| s.common.status, "Service.state", 90, false),
    col(|s| s.services.startup_type, "Config.start", 150, false),
    col(|s| s.services.group, "Config.group", 150, false),
];

const ACT_START: u32 = 1;
const ACT_STOP: u32 = 2;
const ACT_RESTART: u32 = 3;
const ACT_DETAILS: u32 = 4;
const ACT_COPY: u32 = 5;
const ACT_CONSOLE: u32 = 6;

#[derive(Clone)]
struct Service {
    name: String,
    display: String,
    pid: u32,
    state: u32,
    accepts_stop: bool,
}

#[derive(Clone, Default)]
struct Config {
    /// SERVICE_AUTO_START and so on.
    start: Option<u32>,
    group: String,
}

fn start_label(start: Option<u32>) -> &'static str {
    let s = &tr().services;
    match start {
        Some(SERVICE_AUTO_START) => s.auto,
        Some(SERVICE_DEMAND_START) => s.manual,
        Some(SERVICE_DISABLED) => tr().common.disabled,
        Some(SERVICE_BOOT_START) => s.boot,
        Some(SERVICE_SYSTEM_START) => s.system,
        _ => "—",
    }
}

unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn state_label(state: u32) -> &'static str {
    match state {
        SERVICE_RUNNING => tr().common.running,
        SERVICE_STOPPED => tr().services.stopped,
        SERVICE_START_PENDING => tr().services.starting,
        SERVICE_STOP_PENDING => tr().services.stopping,
        SERVICE_PAUSED => tr().common.paused,
        SERVICE_PAUSE_PENDING => tr().services.pausing,
        SERVICE_CONTINUE_PENDING => tr().services.resuming,
        _ => "—",
    }
}

fn enumerate() -> Vec<Service> {
    let mut out = Vec::new();
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE);
        if scm.is_null() {
            return out;
        }
        let mut buf: Vec<u64> = vec![0; 32 * 1024];
        let mut resume = 0u32;
        loop {
            let (mut needed, mut count) = (0u32, 0u32);
            let ok = EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            );
            let more = ok == 0 && GetLastError() == 234; // ERROR_MORE_DATA
            if ok == 0 && !more {
                break;
            }
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            for it in items {
                let st = &it.ServiceStatusProcess;
                out.push(Service {
                    name: pwstr(it.lpServiceName),
                    display: pwstr(it.lpDisplayName),
                    pid: st.dwProcessId,
                    state: st.dwCurrentState,
                    accepts_stop: st.dwControlsAccepted & SERVICE_ACCEPT_STOP != 0,
                });
            }
            if !more {
                break;
            }
            if needed as usize > buf.len() * 8 {
                buf.resize(needed as usize / 8 + 1, 0);
            }
        }
        CloseServiceHandle(scm);
    }
    out
}

fn query_config(names: &[&str]) -> HashMap<String, Config> {
    let mut out = HashMap::new();
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return out;
        }
        let mut buf: Vec<u64> = vec![0; 1024];
        for name in names {
            let w = wide(name);
            let h = OpenServiceW(scm, w.as_ptr(), SERVICE_QUERY_CONFIG);
            let mut cfg = Config { start: None, group: String::new() };
            if !h.is_null() {
                let mut needed = 0;
                let mut ok = QueryServiceConfigW(h, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed);
                if ok == 0 && needed as usize > buf.len() * 8 {
                    buf.resize(needed as usize / 8 + 1, 0);
                    ok = QueryServiceConfigW(h, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed);
                }
                if ok != 0 {
                    let q = &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                    cfg.start = Some(q.dwStartType);
                    // svchost groups come from "-k <group>" in the image path.
                    let path = pwstr(q.lpBinaryPathName);
                    let lower = path.to_lowercase();
                    if let Some(i) = lower.find(" -k ") {
                        cfg.group = path[i + 4..].split_whitespace().next().unwrap_or("").to_string();
                    }
                }
                CloseServiceHandle(h);
            }
            out.insert(name.to_string(), cfg);
        }
        CloseServiceHandle(scm);
    }
    out
}

fn control(name: &str, act: u32) -> Option<String> {
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return Some(error_text(GetLastError()));
        }
        let access = SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS;
        let w = wide(name);
        let h = OpenServiceW(scm, w.as_ptr(), access);
        if h.is_null() {
            let e = GetLastError();
            CloseServiceHandle(scm);
            return Some(format!("{name}: {}", error_text(e)));
        }
        let mut result = None;
        if act == ACT_STOP || act == ACT_RESTART {
            let mut st: SERVICE_STATUS = std::mem::zeroed();
            if ControlService(h, SERVICE_CONTROL_STOP, &mut st) == 0 {
                let e = GetLastError();
                if !(act == ACT_RESTART && e == 1062) {
                    result = Some((tr().services.stop_failed)(name, &error_text(e)));
                }
            }
            if result.is_none() && act == ACT_RESTART {
                // Wait for the stop to finish before starting again.
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let mut sp: SERVICE_STATUS_PROCESS = std::mem::zeroed();
                    let mut needed = 0;
                    let ok = QueryServiceStatusEx(
                        h,
                        SC_STATUS_PROCESS_INFO,
                        (&mut sp as *mut SERVICE_STATUS_PROCESS).cast(),
                        std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
                        &mut needed,
                    );
                    if ok == 0 || sp.dwCurrentState == SERVICE_STOPPED {
                        break;
                    }
                    if Instant::now() > deadline {
                        result = Some((tr().services.stop_timeout)(name));
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
        if result.is_none() && (act == ACT_START || act == ACT_RESTART) && StartServiceW(h, 0, null()) == 0 {
            result = Some((tr().services.start_failed)(name, &error_text(GetLastError())));
        }
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
        result
    }
}

pub struct ServicesPage {
    table: Table,
    services: Vec<Service>,
    configs: HashMap<String, Config>,
    visible: bool,
    last_refresh: Option<Instant>,
    toolbar: Toolbar,
    menu: Option<Menu>,
    confirm: Option<(Confirm, String, u32)>,
    jobs: Jobs,
    area: Rect,
}

impl ServicesPage {
    pub fn new() -> Self {
        let mut table = Table::new(&COLUMNS, 0, false);
        table.empty_text = |s| s.services.loading;
        Self {
            table,
            services: Vec::new(),
            configs: HashMap::new(),
            visible: false,
            last_refresh: None,
            toolbar: Toolbar::default(),
            menu: None,
            confirm: None,
            jobs: Jobs::default(),
            area: Rect::default(),
        }
    }

    fn refresh(&mut self) {
        self.services = enumerate();
        let missing: Vec<&str> = self
            .services
            .iter()
            .filter(|s| !self.configs.contains_key(&s.name))
            .map(|s| s.name.as_str())
            .collect();
        if !missing.is_empty() {
            let found = query_config(&missing);
            self.configs.extend(found);
        }
        let rows = self
            .services
            .iter()
            .map(|s| {
                let cfg = self.configs.get(&s.name).cloned().unwrap_or_default();
                let pid = if s.pid == 0 { String::new() } else { s.pid.to_string() };
                let mut row = Row::new(
                    hash(&s.name),
                    vec![s.name.clone(), pid, s.display.clone(), state_label(s.state).into(), start_label(cfg.start).into(), cfg.group.clone()],
                    vec![
                        text(&s.name),
                        num(s.pid),
                        text(&s.display),
                        text(state_label(s.state)),
                        text(start_label(cfg.start)),
                        text(&cfg.group),
                    ],
                );
                row.dim = s.state == SERVICE_STOPPED;
                row
            })
            .collect();
        self.table.set_rows(rows);
        self.table.empty_text = |s| s.services.empty;
        self.last_refresh = Some(Instant::now());
    }

    fn selected(&self) -> Option<&Service> {
        let id = self.table.selected?;
        self.services.iter().find(|s| hash(&s.name) == id)
    }

    fn buttons(&self) -> [ButtonSpec; 3] {
        let s = self.selected();
        let stopped = s.is_some_and(|s| s.state == SERVICE_STOPPED);
        let running = s.is_some_and(|s| s.state == SERVICE_RUNNING);
        let stoppable = s.is_some_and(|s| s.accepts_stop);
        [
            btn(icons::PLAY, tr().common.start, stopped),
            btn(icons::STOP, tr().common.stop, running && stoppable),
            btn(icons::REFRESH, tr().common.restart, running && stoppable),
        ]
    }

    fn run(&mut self, act: u32, cx: &mut Cx) {
        let Some(s) = self.selected().cloned() else { return };
        match act {
            ACT_START => self.start_job(s.name.clone(), act, cx),
            ACT_STOP | ACT_RESTART => {
                let t = &tr().services;
                let title = if act == ACT_STOP { t.stop_q } else { t.restart_q };
                let ok = if act == ACT_STOP { t.stop_ok } else { t.restart_ok };
                let c = Confirm::new(title, format!("{} ({})", s.display, s.name), &[t.dependents_note], ok, act == ACT_STOP, act);
                self.confirm = Some((c, s.name.clone(), act));
            }
            ACT_DETAILS => cx.cmds.push(super::Cmd::ShowProcess(s.pid)),
            ACT_COPY => {
                if let Some(t) = self.table.selected_text(cx.settings.copy_format) {
                    actions::set_clipboard(&t);
                }
            }
            ACT_CONSOLE => actions::shell_open("services.msc"),
            _ => {}
        }
        *cx.dirty = true;
    }

    fn start_job(&mut self, name: String, act: u32, cx: &mut Cx) {
        let t = &tr().services;
        let toast = match act {
            ACT_START => t.starting_toast,
            ACT_STOP => t.stopping_toast,
            _ => t.restarting_toast,
        };
        cx.cmds.push(super::Cmd::Toast(toast(&name)));
        self.jobs.spawn(cx.shared, move || control(&name, act));
    }
}

fn hash(s: &str) -> u64 {
    // FNV-1a: stable row ids so the selection survives refreshes.
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Page for ServicesPage {
    fn nav(&self) -> Nav {
        Nav { icon: icons::SERVICES, label: tr().services.title, bottom: false }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.table.set_filter(text);
        *cx.dirty = true;
    }

    fn on_show(&mut self, cx: &mut Cx) {
        self.visible = true;
        self.refresh();
        *cx.dirty = true;
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.visible = false;
        self.menu = None;
        self.confirm = None;
    }

    fn on_snapshot(&mut self, _snap: &std::sync::Arc<crate::system::model::Snapshot>, cx: &mut Cx) {
        let due = self.last_refresh.is_none_or(|t| t.elapsed().as_secs() >= REFRESH_SECONDS);
        if self.visible && due {
            self.refresh();
            *cx.dirty = true;
        }
    }

    fn on_enriched(&mut self, cx: &mut Cx) {
        if self.jobs.drain(cx) && self.visible {
            self.refresh();
            *cx.dirty = true;
        }
    }

    fn animating(&self) -> bool {
        self.table.animating()
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        self.menu.is_none() && self.confirm.is_none() && self.table.pannable(x, y)
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Some((c, name, act)) = &mut self.confirm {
            if matches!(ev, Ev::Tick { .. }) {
                return;
            }
            match c.event(ev, cx.dirty) {
                ConfirmOut::Stay => {}
                ConfirmOut::Cancel => {
                    self.confirm = None;
                    *cx.dirty = true;
                }
                ConfirmOut::Ok => {
                    let (name, act) = (name.clone(), *act);
                    self.confirm = None;
                    self.start_job(name, act, cx);
                    *cx.dirty = true;
                }
            }
            return;
        }
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
                    self.run([ACT_START, ACT_STOP, ACT_RESTART][i], cx);
                }
                return;
            }
            Ev::Key { vk: VK_C, ctrl: true } => {
                self.run(ACT_COPY, cx);
                return;
            }
            Ev::Key { vk: VK_F5, .. } => {
                self.refresh();
                *cx.dirty = true;
                return;
            }
            _ => {}
        }
        match self.table.event(ev, cx.dirty) {
            TableOut::Context { x, y } => {
                let b = self.buttons();
                let has_pid = self.selected().is_some_and(|s| s.pid != 0);
                self.menu = Some(Menu::new(
                    x,
                    y,
                    vec![
                        item(tr().common.start, ACT_START, b[0].enabled),
                        item(tr().common.stop, ACT_STOP, b[1].enabled),
                        item(tr().common.restart, ACT_RESTART, b[2].enabled),
                        item(tr().common.show_in_processes, ACT_DETAILS, has_pid),
                        item(tr().common.copy_row, ACT_COPY, true),
                        item(tr().services.open_console, ACT_CONSOLE, true),
                    ],
                ));
                *cx.dirty = true;
            }
            TableOut::Select | TableOut::Activate | TableOut::None => {}
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, _settings: &Settings) {
        self.area = area;
        let (bar, body) = split(c, area);
        let running = self.services.iter().filter(|s| s.state == SERVICE_RUNNING).count();
        let info = if self.table.shown() == self.table.len() {
            (tr().services.info)(self.table.len(), running)
        } else {
            (tr().services.shown)(self.table.shown(), self.table.len())
        };
        let specs = self.buttons();
        let left = self.toolbar.paint(c, bar, &specs);
        paint_title(c, bar, tr().services.title, &info, left);
        self.table.paint(c, body);
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
        if let Some((cf, _, _)) = &mut self.confirm {
            cf.paint(c, area);
        }
    }
}
