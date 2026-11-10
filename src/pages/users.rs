//! The users page: signed-in sessions with their resource use, and the
//! processes of the selected session underneath.

use super::widgets::*;
use super::{icons, Cmd, Cx, Ev, Nav, Page};
use crate::actions;
use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::model::Snapshot;
use crate::settings::Settings;
use crate::theme::THEME;
use std::collections::HashMap;
use std::ptr::null_mut;
use std::sync::Arc;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::RemoteDesktop::*;

static USER_COLUMNS: [Column; 7] = [
    col("ユーザー", 200, false),
    col("セッション ID", 100, true),
    col("状態", 100, false),
    col("プロセス", 80, true),
    col("CPU", 70, true),
    col("メモリ", 110, true),
    col("ディスク", 110, true),
];

static PROC_COLUMNS: [Column; 5] = [
    col("名前", 240, false),
    col("PID", 80, true),
    col("CPU", 70, true),
    col("メモリ", 110, true),
    col("ディスク", 110, true),
];

const ACT_DISCONNECT: u32 = 1;
const ACT_SIGN_OUT: u32 = 2;
const ACT_ACCOUNTS: u32 = 3;
const ACT_COPY: u32 = 4;
const ACT_END: u32 = 5;
const ACT_SHOW: u32 = 6;

#[derive(Clone)]
struct Session {
    id: u32,
    user: String,
    state: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Users,
    Procs,
}

fn sessions() -> Vec<Session> {
    let mut out = Vec::new();
    unsafe {
        let mut info: *mut WTS_SESSION_INFOW = null_mut();
        let mut count = 0u32;
        if WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut info, &mut count) == 0 {
            return out;
        }
        for s in std::slice::from_raw_parts(info, count as usize) {
            let user = query(s.SessionId, WTSUserName);
            if user.is_empty() {
                continue;
            }
            let state = match s.State {
                x if x == WTSActive => "アクティブ",
                x if x == WTSConnected => "接続",
                x if x == WTSDisconnected => "切断",
                x if x == WTSIdle => "アイドル",
                _ => "—",
            };
            out.push(Session { id: s.SessionId, user, state });
        }
        WTSFreeMemory(info.cast());
    }
    out
}

unsafe fn query(session: u32, class: WTS_INFO_CLASS) -> String {
    let mut buf = null_mut();
    let mut bytes = 0u32;
    if WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, session, class, &mut buf, &mut bytes) == 0 || buf.is_null() {
        return String::new();
    }
    let n = (bytes as usize / 2).saturating_sub(1);
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(buf, n));
    WTSFreeMemory(buf.cast());
    s.trim_end_matches('\0').to_string()
}

pub struct UsersPage {
    users: Table,
    procs: Table,
    snap: Arc<Snapshot>,
    sessions: Vec<Session>,
    prev_io: HashMap<(u32, i64), u64>,
    io_rate: HashMap<(u32, i64), f64>,
    visible: bool,
    toolbar: Toolbar,
    menu: Option<Menu>,
    confirm: Option<Confirm>,
    focus: Focus,
    shown_session: Option<u64>,
    jobs: Jobs,
    caption: Rect,
    mouse: (i32, i32),
    pan_target: std::cell::Cell<Option<Focus>>,
}

impl UsersPage {
    pub fn new() -> Self {
        let mut users = Table::new(&USER_COLUMNS, 0, false);
        users.empty_text = "サインインしているユーザーはいません";
        let mut procs = Table::new(&PROC_COLUMNS, 3, true);
        procs.empty_text = "ユーザーを選択するとプロセスを表示します";
        Self {
            users,
            procs,
            snap: Arc::new(Snapshot::default()),
            sessions: Vec::new(),
            prev_io: HashMap::new(),
            io_rate: HashMap::new(),
            visible: false,
            toolbar: Toolbar::default(),
            menu: None,
            confirm: None,
            focus: Focus::Users,
            shown_session: None,
            jobs: Jobs::default(),
            caption: Rect::default(),
            mouse: (-1, -1),
            pan_target: std::cell::Cell::new(None),
        }
    }

    fn rebuild(&mut self) {
        let mut agg: HashMap<u32, (u32, f32, u64, f64)> = HashMap::new();
        for p in self.snap.procs.iter().filter(|p| p.pid > 4) {
            let e = agg.entry(p.session).or_default();
            e.0 += 1;
            e.1 += p.cpu;
            e.2 += p.private_ws;
            e.3 += self.io_rate.get(&p.key()).copied().unwrap_or(0.0);
        }
        let rows = self
            .sessions
            .iter()
            .map(|s| {
                let (n, cpu, mem, io) = agg.get(&s.id).copied().unwrap_or_default();
                let mut row = Row::new(
                    s.id as u64,
                    vec![
                        s.user.clone(),
                        s.id.to_string(),
                        s.state.into(),
                        n.to_string(),
                        format!("{cpu:.0}%"),
                        bytes(mem),
                        bytes_per_sec(io),
                    ],
                    vec![text(&s.user), num(s.id), text(s.state), num(n), num(cpu), num(mem as f64), num(io)],
                );
                row.dim = s.state == "切断";
                row
            })
            .collect();
        self.users.set_rows(rows);
        self.rebuild_procs();
    }

    fn rebuild_procs(&mut self) {
        let Some(sid) = self.users.selected else {
            self.procs.set_rows(Vec::new());
            self.shown_session = None;
            return;
        };
        if self.shown_session != Some(sid) {
            self.procs.selected = None;
            self.shown_session = Some(sid);
        }
        let rows = self
            .snap
            .procs
            .iter()
            .filter(|p| p.pid > 4 && p.session as u64 == sid)
            .map(|p| {
                let io = self.io_rate.get(&p.key()).copied().unwrap_or(0.0);
                let mut row = Row::new(
                    ((p.pid as u64) << 32) ^ (p.create_time as u64 & 0xFFFF_FFFF),
                    vec![
                        p.stat.name.clone(),
                        p.pid.to_string(),
                        format!("{:.0}%", p.cpu),
                        bytes(p.private_ws),
                        bytes_per_sec(io),
                    ],
                    vec![text(&p.stat.name), num(p.pid), num(p.cpu), num(p.private_ws as f64), num(io)],
                );
                row.icon = p.stat.image.clone();
                row
            })
            .collect();
        self.procs.set_rows(rows);
    }

    fn selected_session(&self) -> Option<&Session> {
        let id = self.users.selected?;
        self.sessions.iter().find(|s| s.id as u64 == id)
    }

    fn selected_proc(&self) -> Option<&crate::model::Proc> {
        let id = self.procs.selected?;
        self.snap.procs.iter().find(|p| ((p.pid as u64) << 32) ^ (p.create_time as u64 & 0xFFFF_FFFF) == id)
    }

    fn buttons(&self) -> [ButtonSpec; 2] {
        let s = self.selected_session();
        [
            btn(icons::DISCONNECT, "切断", s.is_some_and(|s| s.state == "アクティブ" || s.state == "接続")),
            btn(icons::SIGN_OUT, "サインアウト", s.is_some()),
        ]
    }

    fn run(&mut self, act: u32, cx: &mut Cx) {
        *cx.dirty = true;
        match act {
            ACT_ACCOUNTS => return actions::shell_open("ms-settings:otherusers"),
            ACT_COPY => {
                let t = match self.focus {
                    Focus::Users => self.users.selected_text(),
                    Focus::Procs => self.procs.selected_text(),
                };
                if let Some(t) = t {
                    actions::set_clipboard(&t);
                }
                return;
            }
            ACT_SHOW => {
                if let Some(p) = self.selected_proc() {
                    cx.cmds.push(Cmd::ShowProcess(p.pid));
                }
                return;
            }
            ACT_END => {
                if let Some(p) = self.selected_proc() {
                    self.confirm = Some(Confirm::new(
                        "タスクを終了しますか?",
                        p.stat.name.clone(),
                        &["このプロセスを終了します。", "保存されていないデータは失われます。"],
                        "終了する",
                        true,
                        ACT_END,
                    ));
                }
                return;
            }
            _ => {}
        }
        let Some(s) = self.selected_session().cloned() else { return };
        let (title, ok, note) = if act == ACT_DISCONNECT {
            ("ユーザーを切断しますか?", "切断する", "アプリは実行されたままになります。")
        } else {
            ("サインアウトしますか?", "サインアウト", "保存されていないデータは失われます。")
        };
        self.confirm = Some(Confirm::new(title, format!("{} (セッション {})", s.user, s.id), &[note], ok, act == ACT_SIGN_OUT, act));
    }

    fn confirmed(&mut self, act: u32, cx: &mut Cx) {
        if act == ACT_END {
            let Some(p) = self.selected_proc() else { return };
            if let Err(code) = actions::terminate(p.pid, p.create_time) {
                cx.cmds.push(Cmd::Toast(format!("{} を終了できませんでした。{}", p.stat.name, error_text(code))));
            }
            cx.shared.poke();
            return;
        }
        let Some(s) = self.selected_session().cloned() else { return };
        self.jobs.spawn(cx.shared, move || unsafe {
            let ok = if act == ACT_DISCONNECT {
                WTSDisconnectSession(WTS_CURRENT_SERVER_HANDLE, s.id, 1)
            } else {
                WTSLogoffSession(WTS_CURRENT_SERVER_HANDLE, s.id, 1)
            };
            (ok == 0).then(|| format!("{} を操作できませんでした。{}", s.user, error_text(GetLastError())))
        });
    }
}

impl Page for UsersPage {
    fn nav(&self) -> Nav {
        Nav { icon: icons::PEOPLE, label: "ユーザー", bottom: false }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.procs.set_filter(text);
        *cx.dirty = true;
    }

    fn on_show(&mut self, cx: &mut Cx) {
        self.visible = true;
        self.sessions = sessions();
        if self.users.selected.is_none() {
            let me = unsafe { windows_sys::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() };
            self.users.selected = Some(me as u64);
        }
        self.rebuild();
        *cx.dirty = true;
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.visible = false;
        self.menu = None;
        self.confirm = None;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        let dt = snap.sys.elapsed.max(0.001) as f64;
        let mut io = HashMap::with_capacity(snap.procs.len());
        self.io_rate.clear();
        for p in &snap.procs {
            let total = p.io_read_bytes + p.io_write_bytes;
            if let Some(prev) = self.prev_io.get(&p.key()) {
                self.io_rate.insert(p.key(), total.saturating_sub(*prev) as f64 / dt);
            }
            io.insert(p.key(), total);
        }
        self.prev_io = io;
        self.snap = snap.clone();
        if self.visible {
            self.sessions = sessions();
            self.rebuild();
            *cx.dirty = true;
        }
    }

    fn on_enriched(&mut self, cx: &mut Cx) {
        if self.jobs.drain(cx) {
            cx.shared.poke();
        }
        *cx.dirty = true;
    }

    fn animating(&self) -> bool {
        self.users.animating() || self.procs.animating()
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        if self.menu.is_some() || self.confirm.is_some() {
            return false;
        }
        // Remember which table the drag started on; pan events carry no position.
        let target = if self.users.pannable(x, y) {
            Some(Focus::Users)
        } else if self.procs.pannable(x, y) {
            Some(Focus::Procs)
        } else {
            None
        };
        self.pan_target.set(target);
        target.is_some()
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Some(c) = &mut self.confirm {
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
                    let act = c.tag;
                    self.confirm = None;
                    *cx.dirty = true;
                    self.confirmed(act, cx);
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
                self.mouse = (x, y);
                if self.toolbar.hover_at(x, y) {
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right } => {
                self.mouse = (x, y);
                if !right {
                    if let Some(i) = self.toolbar.hit(x, y) {
                        if self.buttons()[i].enabled {
                            self.run([ACT_DISCONNECT, ACT_SIGN_OUT][i], cx);
                        }
                        return;
                    }
                }
                if self.users.contains(x, y) {
                    self.focus = Focus::Users;
                } else if self.procs.contains(x, y) {
                    self.focus = Focus::Procs;
                }
            }
            Ev::Key { vk: VK_C, ctrl: true } => return self.run(ACT_COPY, cx),
            Ev::Key { vk: VK_DELETE, .. } if self.focus == Focus::Procs => return self.run(ACT_END, cx),
            Ev::Key { vk: VK_F5, .. } => return cx.shared.poke(),
            _ => {}
        }
        // Mouse moves go to both tables so hover clears; keys to the focused one; the wheel to the one under the mouse.
        let (to_users, to_procs) = match *ev {
            Ev::Key { .. } => (self.focus == Focus::Users, self.focus == Focus::Procs),
            Ev::Wheel { .. } => (self.users.contains(self.mouse.0, self.mouse.1), self.procs.contains(self.mouse.0, self.mouse.1)),
            Ev::Pan { .. } | Ev::Fling { .. } => {
                let t = self.pan_target.get();
                (t == Some(Focus::Users), t == Some(Focus::Procs))
            }
            _ => (true, true),
        };
        if to_users {
            match self.users.event(ev, cx.dirty) {
                TableOut::Select => {
                    self.rebuild_procs();
                    *cx.dirty = true;
                }
                TableOut::Context { x, y } => {
                    self.rebuild_procs();
                    let b = self.buttons();
                    self.menu = Some(Menu::new(
                        x,
                        y,
                        vec![
                            item("切断", ACT_DISCONNECT, b[0].enabled),
                            item("サインアウト", ACT_SIGN_OUT, b[1].enabled),
                            item("行をコピー", ACT_COPY, true),
                            item("ユーザー アカウントの管理", ACT_ACCOUNTS, true),
                        ],
                    ));
                    *cx.dirty = true;
                }
                _ => {}
            }
        }
        if to_procs {
            match self.procs.event(ev, cx.dirty) {
                TableOut::Context { x, y } => {
                    self.menu = Some(Menu::new(
                        x,
                        y,
                        vec![
                            item("タスクの終了", ACT_END, true),
                            item("プロセス ページで表示", ACT_SHOW, true),
                            item("行をコピー", ACT_COPY, true),
                        ],
                    ));
                    *cx.dirty = true;
                }
                TableOut::Activate => self.run(ACT_SHOW, cx),
                _ => {}
            }
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, _settings: &Settings) {
        let t = THEME;
        let (bar, body) = split(c, area);
        let info = format!("{} ユーザー", self.sessions.len());
        let specs = self.buttons();
        let left = self.toolbar.paint(c, bar, &specs);
        paint_title(c, bar, "ユーザー", &info, left);

        let rows = self.users.len().clamp(2, 6) as i32;
        let top_h = (c.px(32) + rows * c.px(28) + c.px(4)).min(body.h / 2);
        self.users.paint(c, rect(body.x, body.y, body.w, top_h));
        let cap_y = body.y + top_h + c.px(8);
        self.caption = rect(body.x + c.px(10), cap_y, body.w - c.px(20), c.px(28));
        let caption = match self.selected_session() {
            Some(s) => format!("{} のプロセス  ({})", s.user, self.procs.shown()),
            None => "プロセス".into(),
        };
        c.fill(rect(body.x, cap_y - c.px(4), body.w, 1), t.border);
        c.text(&caption, Font::Semi, self.caption, t.text, Align::Left);
        let lower = rect(body.x, cap_y + c.px(30), body.w, body.b() - cap_y - c.px(30));
        self.procs.paint(c, lower);
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
        if let Some(cf) = &mut self.confirm {
            cf.paint(c, area);
        }
    }
}
