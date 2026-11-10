//! The app history page: resource use per executable, accumulated while this
//! app runs and kept across restarts in %APPDATA%\LightTaskManager\history.tsv.
//!
//! Only usage observed by this app is counted: a process already running at
//! start-up contributes from that moment on, so restarts never count twice.

use super::widgets::*;
use super::{icons, Cx, Ev, Nav, Page};
use crate::actions;
use crate::gfx::{Canvas, Rect};
use crate::model::{ImageInfo, Snapshot};
use crate::settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SAVE_SECONDS: u64 = 300;

static COLUMNS: [Column; 7] = [
    col("名前", 240, false),
    col("CPU 時間", 100, true),
    col("ディスク読み取り", 120, true),
    col("ディスク書き込み", 120, true),
    col("その他の I/O", 110, true),
    col("起動回数", 80, true),
    Column { label: "最終使用", width: 110, right: false, descending_first: true },
];

const ACT_COPY: u32 = 1;
const ACT_LOCATION: u32 = 2;
const ACT_FORGET: u32 = 3;
const ACT_CLEAR: u32 = 4;

#[derive(Clone, Default)]
struct Usage {
    name: String,
    cpu: u64,
    read: u64,
    write: u64,
    other: u64,
    launches: u32,
    /// Unix seconds.
    last_seen: u64,
    running: u32,
    image: Option<Arc<ImageInfo>>,
}

/// Counters of a live process the last time it was seen.
struct Last {
    key: String,
    cpu: u64,
    read: u64,
    write: u64,
    other: u64,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn file() -> Option<PathBuf> {
    let dir = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(dir).join("LightTaskManager").join("history.tsv"))
}

fn ago(unix: u64, now: u64) -> String {
    let s = now.saturating_sub(unix);
    if s < 60 {
        "1 分以内".into()
    } else if s < 3600 {
        format!("{} 分前", s / 60)
    } else if s < 86400 {
        format!("{} 時間前", s / 3600)
    } else {
        format!("{} 日前", s / 86400)
    }
}

pub struct HistoryPage {
    table: Table,
    usage: HashMap<String, Usage>,
    last: HashMap<(u32, i64), Last>,
    since: u64,
    started: bool,
    visible: bool,
    dirty_rows: bool,
    saved_at: Instant,
    toolbar: Toolbar,
    menu: Option<Menu>,
    confirm: Option<Confirm>,
    placeholder: Arc<ImageInfo>,
}

impl HistoryPage {
    pub fn new() -> Self {
        let placeholder = Arc::new(ImageInfo { path: String::new(), description: OnceLock::new(), icon: OnceLock::new() });
        let _ = placeholder.icon.set(None);
        let mut page = Self {
            table: Table::new(&COLUMNS, 1, true),
            usage: HashMap::new(),
            last: HashMap::new(),
            since: now_unix(),
            started: false,
            visible: false,
            dirty_rows: true,
            saved_at: Instant::now(),
            toolbar: Toolbar::default(),
            menu: None,
            confirm: None,
            placeholder,
        };
        page.table.empty_text = "記録はまだありません";
        page.load();
        page
    }

    fn load(&mut self) {
        let Some(text) = file().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.first() == Some(&"since") {
                if let Some(v) = f.get(1).and_then(|v| v.parse().ok()) {
                    self.since = v;
                }
                continue;
            }
            if f.len() < 7 {
                continue;
            }
            let n = |i: usize| f[i].parse::<u64>().unwrap_or(0);
            self.usage.insert(
                f[0].to_lowercase(),
                Usage {
                    name: f[0].to_string(),
                    cpu: n(1),
                    read: n(2),
                    write: n(3),
                    other: n(4),
                    launches: n(5) as u32,
                    last_seen: n(6),
                    running: 0,
                    image: None,
                },
            );
        }
    }

    fn save(&mut self) {
        self.saved_at = Instant::now();
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = format!("since\t{}\n", self.since);
        for u in self.usage.values() {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                u.name.replace('\t', " "),
                u.cpu,
                u.read,
                u.write,
                u.other,
                u.launches,
                u.last_seen
            ));
        }
        let _ = std::fs::write(path, out);
    }

    fn record(&mut self, snap: &Snapshot) {
        let now = now_unix();
        let first = !self.started;
        self.started = true;
        for u in self.usage.values_mut() {
            u.running = 0;
        }
        let mut next = HashMap::with_capacity(snap.procs.len());
        for p in snap.procs.iter().filter(|p| p.pid != 0) {
            let key = p.stat.name.to_lowercase();
            let u = self.usage.entry(key.clone()).or_insert_with(|| Usage { name: p.stat.name.clone(), ..Default::default() });
            u.running += 1;
            u.last_seen = now;
            if u.image.is_none() {
                u.image = p.stat.image.clone();
            }
            let cur = (p.cpu_time, p.io_read_bytes, p.io_write_bytes, p.io_other_bytes);
            match self.last.get(&p.key()) {
                Some(l) => {
                    u.cpu += cur.0.saturating_sub(l.cpu);
                    u.read += cur.1.saturating_sub(l.read);
                    u.write += cur.2.saturating_sub(l.write);
                    u.other += cur.3.saturating_sub(l.other);
                }
                None if !first => {
                    // Started while we were watching: count it from birth.
                    u.launches += 1;
                    u.cpu += cur.0;
                    u.read += cur.1;
                    u.write += cur.2;
                    u.other += cur.3;
                }
                None => {}
            }
            next.insert(p.key(), Last { key, cpu: cur.0, read: cur.1, write: cur.2, other: cur.3 });
        }
        self.last = next;
        self.dirty_rows = true;
        if self.saved_at.elapsed().as_secs() >= SAVE_SECONDS {
            self.save();
        }
    }

    fn rebuild(&mut self) {
        let now = now_unix();
        let rows = self
            .usage
            .iter()
            .map(|(key, u)| {
                let last = if u.running > 0 { "実行中".to_string() } else { ago(u.last_seen, now) };
                let last_key = if u.running > 0 { u64::MAX as f64 } else { u.last_seen as f64 };
                let mut row = Row::new(
                    fnv(key),
                    vec![
                        u.name.clone(),
                        duration_100ns(u.cpu),
                        bytes(u.read),
                        bytes(u.write),
                        bytes(u.other),
                        u.launches.to_string(),
                        last,
                    ],
                    vec![
                        text(&u.name),
                        num(u.cpu as f64),
                        num(u.read as f64),
                        num(u.write as f64),
                        num(u.other as f64),
                        num(u.launches),
                        num(last_key),
                    ],
                );
                row.icon = Some(u.image.clone().unwrap_or_else(|| self.placeholder.clone()));
                row.dim = u.running == 0;
                row
            })
            .collect();
        self.table.set_rows(rows);
        self.dirty_rows = false;
    }

    fn selected_key(&self) -> Option<String> {
        let id = self.table.selected?;
        self.usage.keys().find(|k| fnv(k) == id).cloned()
    }

    fn buttons(&self) -> [ButtonSpec; 1] {
        [btn(icons::DELETE, "使用状況の履歴の削除", !self.usage.is_empty())]
    }

    fn run(&mut self, act: u32, cx: &mut Cx) {
        *cx.dirty = true;
        match act {
            ACT_COPY => {
                if let Some(t) = self.table.selected_text() {
                    actions::set_clipboard(&t);
                }
            }
            ACT_LOCATION => {
                if let Some(path) = self.selected_key().and_then(|k| self.usage[&k].image.as_ref().map(|i| i.path.clone())) {
                    actions::open_location(&path);
                }
            }
            ACT_FORGET => {
                if let Some(k) = self.selected_key() {
                    self.usage.remove(&k);
                    self.last.retain(|_, l| l.key != k);
                    self.rebuild();
                    self.save();
                }
            }
            ACT_CLEAR => {
                self.confirm = Some(Confirm::new(
                    "使用状況の履歴を削除しますか?",
                    "すべてのアプリの記録",
                    &["実行中のプロセスはこれから先の使用量だけが記録されます。"],
                    "削除する",
                    true,
                    ACT_CLEAR,
                ));
            }
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.usage.clear();
        // Keep the current counters as the new baseline.
        self.since = now_unix();
        self.rebuild();
        self.save();
    }
}

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Page for HistoryPage {
    fn nav(&self) -> Nav {
        Nav { icon: icons::HISTORY, label: "アプリの履歴", bottom: false }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.table.set_filter(text);
        *cx.dirty = true;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        self.record(snap);
        if self.visible {
            self.rebuild();
            *cx.dirty = true;
        }
    }

    fn on_show(&mut self, cx: &mut Cx) {
        self.visible = true;
        if self.dirty_rows {
            self.rebuild();
        }
        *cx.dirty = true;
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.visible = false;
        self.menu = None;
        self.confirm = None;
    }

    fn on_exit(&mut self) {
        self.save();
    }

    fn animating(&self) -> bool {
        self.table.animating()
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        self.menu.is_none() && self.confirm.is_none() && self.table.pannable(x, y)
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
                    self.confirm = None;
                    self.clear();
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
                if self.buttons()[0].enabled {
                    self.run(ACT_CLEAR, cx);
                }
                return;
            }
            Ev::Key { vk: VK_C, ctrl: true } => return self.run(ACT_COPY, cx),
            Ev::Key { vk: VK_DELETE, .. } => return self.run(ACT_FORGET, cx),
            _ => {}
        }
        if let TableOut::Context { x, y } = self.table.event(ev, cx.dirty) {
            let has_path = self
                .selected_key()
                .is_some_and(|k| self.usage[&k].image.as_ref().is_some_and(|i| !i.path.is_empty()));
            self.menu = Some(Menu::new(
                x,
                y,
                vec![
                    item("行をコピー", ACT_COPY, true),
                    item("ファイルの場所を開く", ACT_LOCATION, has_path),
                    item("この記録を削除", ACT_FORGET, true),
                ],
            ));
            *cx.dirty = true;
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, _settings: &Settings) {
        let (bar, body) = split(c, area);
        let info = format!(
            "{} アプリ   記録開始: {}   このアプリの実行中に計測",
            self.usage.len(),
            ago(self.since, now_unix())
        );
        let specs = self.buttons();
        let left = self.toolbar.paint(c, bar, &specs);
        paint_title(c, bar, "アプリの履歴", &info, left);
        self.table.paint(c, body);
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
        if let Some(cf) = &mut self.confirm {
            cf.paint(c, area);
        }
    }
}
