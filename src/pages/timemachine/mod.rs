//! The time machine page: keeps the last few minutes of samples, so the system's
//! load and what each process used can be looked up after the fact.
//!
//! Samples are kept in a ring that drops the oldest ones. Selecting a time freezes
//! the view on a copy of the ring (the samples are shared, so only pointers are
//! copied); recording goes on in the ring meanwhile, and resuming shows it again.

mod export;
mod input;
mod paint;
mod table;
mod target;

use super::{Cmd, Cx, Ev, Nav, Page};
use crate::i18n::{tr, Text};
use crate::settings::Settings;
use crate::system::actions;
use crate::system::model::{ImageInfo, ProcStatic, Snapshot, Status};
use crate::system::win::{filetime_now, SEC};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons::{self, glyph};
use crate::ui::keys::*;
use crate::ui::theme::THEME;
use crate::ui::widgets::*;
use export::{Encoding, Format, Kind};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use target::{enabled, targets, toggle, Target, DIALOG_ORDER};
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

/// FILETIME units (100 ns) per second.
/// The view never zooms in further than this many seconds.
const MIN_SPAN_SECONDS: i64 = 10;
const ZOOM_STEP: f64 = 1.25;
/// The longest retention period, in minutes (one day).
const MAX_MINUTES: u32 = 1440;
/// A drag shorter than this many pixels selects a single time.
const CLICK_SLOP: i32 = 4;
const BORN_COLOR: u32 = crate::ui::theme::DARK.born;
const DIED_COLOR: u32 = crate::ui::theme::DARK.died;

const ACT_COPY: u32 = 1;
const ACT_SHOW: u32 = 2;
/// Samples measured in this long after start-up are not recorded: they mostly show
/// this app's own start-up (reading icons and descriptions of every executable).
const WARMUP: Duration = Duration::from_secs(2);

/// Picks one series' value out of a sample.
type SeriesFn = dyn Fn(&Sample) -> Option<f32>;

/// What one process used at one sample.
struct TmProc {
    stat: Arc<ProcStatic>,
    create_time: i64,
    pid: u32,
    cpu: f32,
    mem_kb: u32,
    gpu: f32,
    /// Disk reads and writes, in bytes per second.
    disk_bps: f32,
    commit_kb: u32,
    handles: u32,
    /// Page faults per second.
    faults: f32,
    gpu_mem_kb: u32,
    hung: bool,
}

struct Sample {
    /// FILETIME (UTC).
    time: i64,
    cpu: f32,
    /// Kernel time, in percent of all processors.
    kernel: f32,
    /// Clock speed in MHz, 0 when unknown.
    mhz: f32,
    mem_pct: f32,
    mem_used: u64,
    mem_total: u64,
    commit: u64,
    commit_limit: u64,
    /// Active time per disk, in the page's disk order; NaN where a disk is missing.
    disks: Box<[f32]>,
    /// Utilization per adapter, in the page's adapter order; NaN where an adapter is missing.
    gpus: Box<[f32]>,
    /// Dedicated memory in use per adapter, in bytes; 0 where an adapter is missing.
    gpu_mem: Box<[u64]>,
    handles: u32,
    /// Page faults per second, all processes together.
    faults: f32,
    procs: Box<[TmProc]>,
}

/// A disk or a GPU adapter seen in the samples. Its index in the page's list is fixed.
struct Device {
    id: String,
    name: String,
    /// Dedicated memory of a GPU adapter, 0 when unknown.
    total: u64,
}

fn dim(color: u32) -> u32 {
    let ch = |sh: u32| ((((color >> sh) & 255) * 3 / 5) & 255) << sh;
    ch(16) | ch(8) | ch(0)
}

/// GPU adapters get their own colors up to the third, kept in the cool hues next to each
/// other so they read as one kind; the rest share one.
fn gpu_color(rank: usize) -> u32 {
    match rank {
        0 => THEME.chart_gpu,
        1 => 0xA07CFF,
        2 => 0xF072C8,
        _ => 0xB4B4BE,
    }
}

/// Committed memory is a kind of memory, so it takes the memory color dimmed.
fn commit_color() -> u32 {
    dim(THEME.chart_mem)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    None,
    Single,
    Range,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Switch,
    Settings,
    Export,
    Resume,
}

/// Which samples the export writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    /// The selected time or range.
    Selection,
    /// Everything kept when the view was paused.
    All,
}

/// The export dialog: what to write, and its hit areas.
struct ExportDialog {
    scope: Scope,
    kind: Kind,
    format: Format,
    encoding: Encoding,
    /// The rough size of the file, for the choices it was worked out for.
    estimate: Option<((Scope, Kind, Format, Encoding), u64)>,
    panel: Rect,
    scopes: [Rect; 2],
    radios: [Rect; 3],
    format_box: Rect,
    encoding_box: Rect,
    cancel: Rect,
    ok: Rect,
    /// 0..3 contents, 3 format, 4 encoding, 5 cancel, 6 ok, 7..9 scopes.
    hover: Option<u8>,
    /// An open drop-down: which (3 format, 4 encoding) and its list.
    list: Option<(u8, Menu)>,
}

impl ExportDialog {
    fn new(s: &Settings) -> Self {
        Self {
            scope: Scope::Selection,
            kind: Kind::from_key(&s.tm_export_kind),
            format: Format::from_key(&s.tm_export_format),
            encoding: Encoding::from_key(&s.tm_export_encoding),
            estimate: None,
            panel: Rect::default(),
            scopes: [Rect::default(); 2],
            radios: [Rect::default(); 3],
            format_box: Rect::default(),
            encoding_box: Rect::default(),
            cancel: Rect::default(),
            ok: Rect::default(),
            hover: None,
            list: None,
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<u8> {
        if let Some(i) = self.radios.iter().position(|r| r.contains(x, y)) {
            return Some(i as u8);
        }
        if let Some(i) = self.scopes.iter().position(|r| r.contains(x, y)) {
            return Some(7 + i as u8);
        }
        [(self.format_box, 3), (self.encoding_box, 4), (self.cancel, 5), (self.ok, 6)]
            .into_iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, i)| i)
    }
}

fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1024.0 * 1024.0 {
        (tr().units.approx)(&format!("{:.0} KB", (b / 1024.0).max(1.0)))
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        (tr().units.approx)(&format!("{:.1} MB", b / 1024.0 / 1024.0))
    } else {
        (tr().units.approx)(&format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0))
    }
}

/// The settings dialog's hit areas.
#[derive(Default)]
struct Dialog {
    panel: Rect,
    checks: [Rect; 10],
    /// The retention period's number box and its spinner buttons.
    field: Rect,
    up: Rect,
    down: Rect,
    close: Rect,
    hover: Option<u8>,
    /// What is typed in the number box; applied on Enter, the spinner, or leaving the box.
    text: String,
    focused: bool,
    /// The whole number is selected, so the next digit replaces it.
    fresh: bool,
}

impl Dialog {
    fn new(minutes: u32) -> Self {
        Self { text: minutes.to_string(), focused: true, fresh: true, ..Default::default() }
    }

    /// The typed number within the allowed range, if any is typed.
    fn minutes(&self) -> Option<u32> {
        self.text.parse::<u32>().ok().map(|m| m.clamp(1, MAX_MINUTES))
    }
}

pub struct TimeMachinePage {
    live: Vec<Arc<Sample>>,
    /// The ring as it was when the view was paused.
    frozen: Option<Vec<Arc<Sample>>>,
    /// GPU adapters and disks in the order they first appeared; samples index into these.
    adapters: Vec<Device>,
    disks: Vec<Device>,
    /// Local time minus UTC, in seconds.
    offset: i64,
    /// The width of the view in FILETIME units, when paused.
    span: i64,
    /// The right edge of the view, when paused.
    view_end: i64,
    /// Selected samples, as indices into the frozen ring (first, last).
    sel: Option<(usize, usize)>,
    /// The sample a press started on, and the x it started at.
    drag: Option<(usize, i32)>,
    mouse: (i32, i32),
    plot: Rect,
    switch_rect: Rect,
    settings_rect: Rect,
    resume: Toolbar,
    hover: Hover,
    table: Table,
    /// What the table's columns were built for.
    built: Option<(Mode, Vec<Target>)>,
    /// Row id to PID, for "show in details".
    pids: HashMap<u64, u32>,
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    export: Option<ExportDialog>,
    export_rect: Rect,
    /// The search box's terms, lowercase.
    filter: Vec<String>,
    last_procs: usize,
    /// When the app started; see `WARMUP`.
    started: Instant,
    visible: bool,
    placeholder: Arc<ImageInfo>,
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn system_seconds(s: &SYSTEMTIME) -> i64 {
    days_from_civil(s.wYear as i64, s.wMonth as i64, s.wDay as i64) * 86400
        + s.wHour as i64 * 3600
        + s.wMinute as i64 * 60
        + s.wSecond as i64
}

/// Local time minus UTC in seconds, rounded to the minute.
fn local_offset() -> i64 {
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    let d = system_seconds(&local) - system_seconds(&utc);
    (d as f64 / 60.0).round() as i64 * 60
}

fn group_thousands(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The index of a device in the list, adding it when it is new.
fn slot(list: &mut Vec<Device>, id: &str, name: &str) -> usize {
    match list.iter().position(|d| d.id == id) {
        Some(i) => {
            if list[i].name.is_empty() {
                list[i].name = name.to_string();
            }
            i
        }
        None => {
            list.push(Device { id: id.to_string(), name: name.to_string(), total: 0 });
            list.len() - 1
        }
    }
}

/// What one process did over a range of samples, in the export's units.
struct RangeStat {
    stat: Arc<ProcStatic>,
    pid: u32,
    create_time: i64,
    /// Not there at the first sample of the range.
    born: bool,
    /// Gone by the last sample of the range.
    died: bool,
    /// Its creation time, or the first sample it was seen at.
    born_at: i64,
    /// The first sample it was missing from.
    died_at: i64,
    start: Vec<f64>,
    end: Vec<f64>,
    sum: Vec<f64>,
    max: Vec<f64>,
    n: u32,
    /// Time spent not responding, in FILETIME units.
    hung: i64,
}

impl RangeStat {
    /// The average over the samples where the process existed.
    fn avg(&self, j: usize) -> f64 {
        self.sum[j] / self.n.max(1) as f64
    }
}

/// Per process over the range: first, last, average and largest of each target's value.
fn range_stats(range: &[Arc<Sample>], values: &[Target]) -> Vec<RangeStat> {
    struct Acc<'a> {
        p: &'a TmProc,
        first: usize,
        last: usize,
        stat: RangeStat,
    }
    let k = values.len();
    let mut accs: HashMap<(u32, i64), Acc> = HashMap::new();
    for (i, sample) in range.iter().enumerate() {
        // A hung sample counts for the time since the one before it.
        let step = match i {
            0 if range.len() > 1 => range[1].time - range[0].time,
            0 => 0,
            _ => sample.time - range[i - 1].time,
        };
        for p in sample.procs.iter() {
            let a = accs.entry((p.pid, p.create_time)).or_insert_with(|| Acc {
                p,
                first: i,
                last: i,
                stat: RangeStat {
                    stat: p.stat.clone(),
                    pid: p.pid,
                    create_time: p.create_time,
                    born: false,
                    died: false,
                    born_at: 0,
                    died_at: 0,
                    start: vec![0.0; k],
                    end: vec![0.0; k],
                    sum: vec![0.0; k],
                    max: vec![f64::MIN; k],
                    n: 0,
                    hung: 0,
                },
            });
            let st = &mut a.stat;
            for (j, t) in values.iter().enumerate() {
                let v = t.raw(p);
                if st.n == 0 {
                    st.start[j] = v;
                }
                st.end[j] = v;
                st.sum[j] += v;
                st.max[j] = st.max[j].max(v);
            }
            if p.hung {
                st.hung += step;
            }
            a.last = i;
            st.n += 1;
        }
    }
    let n = range.len();
    accs.into_values()
        .map(|a| {
            let mut st = a.stat;
            st.born = a.first > 0;
            st.died = a.last + 1 < n;
            st.born_at = if a.p.create_time > 0 { a.p.create_time } else { range[a.first].time };
            st.died_at = if st.died { range[a.last + 1].time } else { 0 };
            st
        })
        .collect()
}

fn row_id(pid: u32, create_time: i64) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in pid.to_le_bytes().iter().chain(create_time.to_le_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn duration_text(seconds: i64) -> String {
    if seconds < 60 {
        (tr().units.seconds)(seconds as u64)
    } else if seconds % 60 == 0 {
        (tr().units.minutes)(seconds as u64 / 60)
    } else {
        (tr().units.min_sec)(seconds as u64 / 60, seconds as u64 % 60)
    }
}

impl TimeMachinePage {
    pub fn new() -> Self {
        let placeholder = Arc::new(ImageInfo { path: String::new(), description: OnceLock::new(), company: OnceLock::new(), icon: OnceLock::new() });
        let _ = placeholder.icon.set(None);
        let mut table = Table::new(&[], 0, false);
        table.shrink_first = true;
        Self {
            live: Vec::new(),
            frozen: None,
            adapters: Vec::new(),
            disks: Vec::new(),
            offset: local_offset(),
            span: 0,
            view_end: 0,
            sel: None,
            drag: None,
            mouse: (-1, -1),
            plot: Rect::default(),
            switch_rect: Rect::default(),
            settings_rect: Rect::default(),
            resume: Toolbar::default(),
            hover: Hover::None,
            table,
            built: None,
            pids: HashMap::new(),
            menu: None,
            dialog: None,
            export: None,
            export_rect: Rect::default(),
            filter: Vec::new(),
            last_procs: 0,
            started: Instant::now(),
            visible: false,
            placeholder,
        }
    }

    fn clock(&self, ft: i64) -> String {
        let s = (ft.div_euclid(SEC) + self.offset).rem_euclid(86400);
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }

    fn data(&self) -> &[Arc<Sample>] {
        match &self.frozen {
            Some(f) => f,
            None => &self.live,
        }
    }

    fn full_span(s: &Settings) -> i64 {
        s.tm_minutes.max(1) as i64 * 60 * SEC
    }

    fn min_span(s: &Settings) -> i64 {
        (MIN_SPAN_SECONDS * SEC).min(Self::full_span(s))
    }

    /// The time range the graph shows: always the retention period while live.
    fn view(&self, s: &Settings) -> (i64, i64) {
        let full = Self::full_span(s);
        let last = self.data().last().map_or_else(filetime_now, |x| x.time);
        if self.frozen.is_none() {
            return (last - full, last);
        }
        let span = self.span.clamp(Self::min_span(s), full);
        let end = self.view_end.clamp(last - full + span, last);
        (end - span, end)
    }

    fn time_at(&self, x: i32, s: &Settings) -> i64 {
        let (start, end) = self.view(s);
        let inner = self.plot.inset(1, 1);
        let f = ((x - inner.x) as f64 / inner.w.max(1) as f64).clamp(0.0, 1.0);
        start + ((end - start) as f64 * f) as i64
    }

    fn x_at(&self, t: i64, s: &Settings) -> i32 {
        let (start, end) = self.view(s);
        let inner = self.plot.inset(1, 1);
        inner.x + ((t - start) as f64 / (end - start).max(1) as f64 * inner.w as f64).round() as i32
    }

    /// The sample nearest to a time.
    fn nearest(data: &[Arc<Sample>], t: i64) -> Option<usize> {
        if data.is_empty() {
            return None;
        }
        let j = data.partition_point(|s| s.time < t);
        if j == 0 {
            return Some(0);
        }
        if j == data.len() {
            return Some(data.len() - 1);
        }
        Some(if t - data[j - 1].time <= data[j].time - t { j - 1 } else { j })
    }

    fn freeze(&mut self, s: &Settings) {
        if self.frozen.is_none() {
            self.frozen = Some(self.live.clone());
            self.span = Self::full_span(s);
            self.view_end = self.live.last().map_or(0, |x| x.time);
        }
    }

    fn resume_live(&mut self, cx: &mut Cx) {
        self.frozen = None;
        self.sel = None;
        self.drag = None;
        self.menu = None;
        self.span = 0;
        self.update_table(cx.settings);
        *cx.dirty = true;
    }

    fn record(&mut self, snap: &Snapshot, s: &Settings) {
        let time = filetime_now();
        let sys = &snap.sys;
        let seconds = sys.elapsed.max(0.001);
        let mut gpus = vec![f32::NAN; self.adapters.len()];
        let mut gpu_mem = vec![0u64; self.adapters.len()];
        for g in &sys.gpus {
            let i = slot(&mut self.adapters, &g.luid, &g.name);
            if g.dedicated_total > 0 {
                self.adapters[i].total = g.dedicated_total;
            }
            gpus.resize(self.adapters.len(), f32::NAN);
            gpu_mem.resize(self.adapters.len(), 0);
            gpus[i] = g.util;
            gpu_mem[i] = g.dedicated;
        }
        let mut disks = vec![f32::NAN; self.disks.len()];
        for d in &sys.disks {
            let letters = if d.letters.is_empty() { String::new() } else { format!(" ({})", d.letters) };
            let i = slot(&mut self.disks, &d.index.to_string(), &(tr().common.disk_n)(d.index, &letters));
            disks.resize(self.disks.len(), f32::NAN);
            disks[i] = d.active;
        }
        let used = sys.mem.total.saturating_sub(sys.mem.available);
        let kb = |b: u64| (b / 1024).min(u32::MAX as u64) as u32;
        let procs: Vec<TmProc> = snap
            .procs
            .iter()
            .filter(|p| p.pid != 0)
            .map(|p| TmProc {
                stat: p.stat.clone(),
                create_time: p.create_time,
                pid: p.pid,
                cpu: p.cpu,
                mem_kb: kb(p.active_private),
                gpu: p.gpu_util,
                disk_bps: p.disk_bps as f32,
                commit_kb: kb(p.commit),
                handles: p.handles,
                faults: p.pf_delta as f32 / seconds,
                gpu_mem_kb: kb(p.gpu_dedicated),
                hung: p.status == Status::NotResponding,
            })
            .collect();
        self.last_procs = procs.len();
        let faults = procs.iter().map(|p| p.faults).sum();
        self.live.push(Arc::new(Sample {
            time,
            cpu: sys.cpu.total,
            kernel: sys.cpu.kernel,
            mhz: sys.cpu.mhz,
            mem_pct: used as f32 / sys.mem.total.max(1) as f32 * 100.0,
            mem_used: used,
            mem_total: sys.mem.total,
            commit: sys.mem.commit,
            commit_limit: sys.mem.commit_limit,
            disks: disks.into_boxed_slice(),
            gpus: gpus.into_boxed_slice(),
            gpu_mem: gpu_mem.into_boxed_slice(),
            handles: sys.cpu.handles,
            faults,
            procs: procs.into_boxed_slice(),
        }));
        self.trim(s);
        self.offset = local_offset();
    }

    /// Drops the samples older than the retention period.
    fn trim(&mut self, s: &Settings) {
        let Some(last) = self.live.last().map(|x| x.time) else { return };
        let cutoff = last - Self::full_span(s);
        let n = self.live.partition_point(|x| x.time < cutoff);
        if n > 0 {
            self.live.drain(..n);
        }
    }

    /// Bytes the ring would take at the current process count and update speed.
    fn estimate(&self, s: &Settings, minutes: u32) -> f64 {
        let interval = if s.interval_ms == 0 { 1000 } else { s.interval_ms } as f64;
        let samples = minutes as f64 * 60_000.0 / interval;
        let procs = self.last_procs.max(1) as f64;
        let per = std::mem::size_of::<Sample>() as f64
            + 32.0
            + procs * std::mem::size_of::<TmProc>() as f64
            + self.adapters.len() as f64 * 12.0
            + self.disks.len() as f64 * 4.0;
        samples * per
    }

    // ------------------------------------------------------------ table

    // ------------------------------------------------------------ export

    /// What to write, taken from what the page shows now.
    fn export_job(&self, s: &Settings, scope: Scope, kind: Kind, format: Format, encoding: Encoding) -> Option<export::Job> {
        let (a, b) = self.sel?;
        let data = self.frozen.as_ref()?;
        let range = match scope {
            Scope::Selection => data.get(a..=b)?.to_vec(),
            Scope::All => data.clone(),
        };
        // The table on screen is the selection's; the whole record gets an order of its own.
        let order = (scope == Scope::Selection).then(|| self.table.visible_ids());
        let adapters = self.shown_adapters().into_iter().map(|(i, rank)| (i, rank, self.adapters[i].total)).collect();
        let mut disks: Vec<(u32, usize)> = self.disks.iter().enumerate().filter_map(|(i, d)| Some((d.id.parse().ok()?, i))).collect();
        disks.sort_unstable();
        Some(export::Job {
            kind,
            format,
            encoding,
            targets: targets(s),
            range,
            order,
            terms: self.filter.clone(),
            offset: self.offset,
            disks,
            adapters,
        })
    }

    // ------------------------------------------------------------ painting

    // ------------------------------------------------------------ input
}

impl Page for TimeMachinePage {
    fn nav(&self) -> Nav {
        Nav { icon: icons::RECENT, label: tr().tm.title, bottom: false }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    /// While on it records wherever the user is, minimized too, and GPU use when asked.
    fn demand(&self, s: &Settings) -> crate::system::demand::Needs {
        use crate::system::demand::need;
        let gpu = targets(s).iter().any(|t| t.uses_gpu());
        let always = match (s.tm_enabled, gpu) {
            (false, _) => 0,
            (true, false) => need::SYS | need::PROCS,
            (true, true) => need::SYS | need::PROCS | need::SYS_GPU | need::PROC_GPU,
        };
        crate::system::demand::Needs { shown: 0, always }
    }

    fn captures_text(&self) -> bool {
        self.dialog.is_some()
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.table.set_filter(text);
        self.filter = text.to_lowercase().split_whitespace().map(String::from).collect();
        *cx.dirty = true;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        if !cx.settings.tm_enabled || !snap.has_procs || snap.sys.elapsed <= 0.0 {
            return;
        }
        // Skip samples whose measured span began during start-up.
        let span_start = Instant::now().checked_sub(Duration::from_secs_f32(snap.sys.elapsed));
        if span_start.is_none_or(|t| t < self.started + WARMUP) {
            return;
        }
        self.record(snap, cx.settings);
        if self.visible && self.frozen.is_none() {
            *cx.dirty = true;
        }
    }

    fn on_show(&mut self, cx: &mut Cx) {
        self.visible = true;
        *cx.dirty = true;
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.visible = false;
        self.menu = None;
        self.dialog = None;
        self.export = None;
        self.drag = None;
    }

    fn animating(&self) -> bool {
        self.table.animating()
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        self.menu.is_none() && self.dialog.is_none() && self.export.is_none() && self.table.pannable(x, y)
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if self.dialog.is_some() && !matches!(ev, Ev::Tick { .. }) {
            self.dialog_event(ev, cx);
            return;
        }
        if self.export.is_some() && !matches!(ev, Ev::Tick { .. }) {
            self.export_event(ev, cx);
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
                let was = self.plot.inset(1, 1).contains(self.mouse.0, self.mouse.1);
                self.mouse = (x, y);
                self.update_hover(cx);
                if was || self.plot.inset(1, 1).contains(x, y) {
                    *cx.dirty = true;
                }
            }
            Ev::Leave => {
                self.mouse = (-1, -1);
                self.update_hover(cx);
                *cx.dirty = true;
            }
            Ev::Down { x, y, right: false } => {
                self.mouse = (x, y);
                if self.switch_rect.contains(x, y) {
                    return self.toggle(cx);
                }
                if self.settings_rect.contains(x, y) {
                    self.dialog = Some(Dialog::new(cx.settings.tm_minutes));
                    *cx.dirty = true;
                    return;
                }
                if self.export_rect.contains(x, y) {
                    if self.sel.is_some() {
                        self.export = Some(ExportDialog::new(cx.settings));
                        self.refresh_estimate(cx.settings);
                        *cx.dirty = true;
                    }
                    return;
                }
                if self.resume.hit(x, y).is_some() {
                    return self.resume_live(cx);
                }
            }
            Ev::Key { vk: VK_ESCAPE, .. } if self.frozen.is_some() => return self.resume_live(cx),
            Ev::Key { vk: VK_C, ctrl: true } => return self.run(ACT_COPY, cx),
            Ev::Key { vk: VK_F5, .. } => {
                cx.shared.poke();
                return;
            }
            _ => {}
        }
        if self.graph_event(ev, cx) {
            return;
        }
        if !cx.settings.tm_enabled || self.sel.is_none() {
            return;
        }
        match self.table.event(ev, cx.dirty) {
            TableOut::Context { x, y } => {
                let alive = self.selected_alive();
                self.menu = Some(Menu::new(x, y, vec![item(tr().common.copy_row, ACT_COPY, true), item(tr().tm.show_details, ACT_SHOW, alive)]));
                *cx.dirty = true;
            }
            TableOut::Activate if self.selected_alive() => self.run(ACT_SHOW, cx),
            _ => {}
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, s: &Settings) {
        let t = THEME;
        let (bar, body) = split(c, area);
        self.paint_toolbar(c, bar, s);
        if !s.tm_enabled {
            self.plot = Rect::default();
            let msg = rect(body.x, body.y + c.px(40), body.w, c.px(30));
            c.text(tr().tm.off_title, Font::Semi, msg, t.text, Align::Center);
            c.text(
                tr().tm.off_note,
                Font::Body,
                rect(body.x, msg.b() + c.px(4), body.w, c.px(26)),
                t.text_dim,
                Align::Center,
            );
            return;
        }
        let gh = (body.h * 2 / 5).clamp(c.px(180), c.px(340)).min(body.h);
        let block = rect(body.x + c.px(8), body.y, body.w - c.px(16), gh);
        self.paint_graph(c, block, s);
        let table = rect(body.x, block.b() + c.px(12), body.w, body.b() - block.b() - c.px(12));
        if table.h > c.px(40) {
            if self.sel.is_none() {
                c.text(tr().tm.pick_point, Font::Body, table, t.text_dim, Align::Center);
            } else {
                self.table.paint(c, table);
                self.paint_time_tooltip(c, area);
            }
        }
        // Over the table: the tooltip sits below the pointer and may run past the graph.
        self.paint_hover(c, area, s, &targets(s));
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
        self.paint_dialog(c, area, s);
        self.paint_export(c, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::model::Protect;

    fn stat(name: &str) -> Arc<ProcStatic> {
        Arc::new(ProcStatic {
            name: name.into(),
            user: Arc::from("u"),
            arch: "x64",
            elevated: None,
            cmdline: String::new(),
            image: None,
            package: None,
            platform: 64,
            system_dir: false,
            protect: Protect::default(),
        })
    }

    fn proc(pid: u32, cpu: f32, hung: bool) -> TmProc {
        TmProc {
            stat: stat(&format!("p{pid}")),
            create_time: pid as i64 * SEC,
            pid,
            cpu,
            mem_kb: 1024,
            gpu: 0.0,
            disk_bps: 2.0 * 1024.0 * 1024.0,
            commit_kb: 2048,
            handles: 100 + pid,
            faults: 0.0,
            gpu_mem_kb: 0,
            hung,
        }
    }

    fn sample_with(time: i64, procs: Vec<TmProc>) -> Arc<Sample> {
        Arc::new(Sample {
            time,
            cpu: 0.0,
            kernel: 0.0,
            mhz: 0.0,
            mem_pct: 0.0,
            mem_used: 0,
            mem_total: 0,
            commit: 0,
            commit_limit: 0,
            disks: Box::new([]),
            gpus: Box::new([]),
            gpu_mem: Box::new([]),
            handles: 0,
            faults: 0.0,
            procs: procs.into_boxed_slice(),
        })
    }

    fn sample(time: i64, procs: Vec<(u32, f32)>) -> Arc<Sample> {
        sample_with(time, procs.into_iter().map(|(pid, cpu)| proc(pid, cpu, false)).collect())
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 11, 12), 20769);
    }

    #[test]
    fn numbers() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(1234), "1,234");
        assert_eq!(group_thousands(1234567), "1,234,567");
        assert_eq!(Target::Cpu.delta(-0.01), "0.0");
        assert_eq!(Target::Cpu.delta(1.25), "+1.2");
        assert_eq!(Target::Mem.delta(-3.0), "-3.0");
    }

    #[test]
    fn range_marks_births_deaths_and_stats() {
        // 1 lives throughout, 2 dies after the first sample, 3 is born at the second.
        let data = vec![
            sample(10 * SEC, vec![(1, 10.0), (2, 5.0)]),
            sample(11 * SEC, vec![(1, 30.0), (3, 7.0)]),
            sample(12 * SEC, vec![(1, 20.0), (3, 9.0)]),
        ];
        let mut page = TimeMachinePage::new();
        let rows = page.range_rows(&data, &[Target::Cpu]);
        let row = |pid: u32| rows.iter().find(|r| r.cells[1] == pid.to_string()).unwrap();
        // Columns: name, PID, +-, time, start, end, delta, avg, max.
        assert_eq!(row(1).cells[2], "");
        assert_eq!(&row(1).cells[4..], ["10.0", "20.0", "+10.0", "20.0", "30.0"]);
        assert_eq!(row(2).cells[2], "-");
        assert_eq!(row(2).cells[3], page.clock(11 * SEC));
        assert_eq!(row(3).cells[2], "+");
        assert_eq!(row(3).cells[3], page.clock(3 * SEC));
        // Average and maximum only over the samples where the process existed.
        assert_eq!(&row(3).cells[4..], ["7.0", "9.0", "+2.0", "8.0", "9.0"]);
        assert_eq!(row(2).colors, vec![(2, DIED_COLOR)]);
        assert_eq!(row(3).colors, vec![(2, BORN_COLOR)]);
    }

    #[test]
    fn range_shows_new_targets_and_time_not_responding() {
        // 1 stops responding for the last two samples.
        let data = vec![
            sample_with(10 * SEC, vec![proc(1, 1.0, false)]),
            sample_with(11 * SEC, vec![proc(1, 1.0, true)]),
            sample_with(13 * SEC, vec![proc(1, 1.0, true)]),
        ];
        let mut page = TimeMachinePage::new();
        let rows = page.range_rows(&data, &[Target::Disk, Target::Commit, Target::Handles, Target::Hung]);
        let r = &rows[0];
        // name, PID, +-, time, disk x5, commit x5, handles x5, hung seconds.
        assert_eq!(r.cells.len(), 4 + 15 + 1);
        assert_eq!(&r.cells[4..9], ["2.0", "2.0", "0.0", "2.0", "2.0"]);
        assert_eq!(r.cells[9], "2.0");
        assert_eq!(r.cells[14], "101");
        // One second, then two: three seconds not responding.
        assert_eq!(r.cells[19], "3");
        assert_eq!(r.colors, vec![(19, DIED_COLOR)]);
    }

    #[test]
    fn adapters_are_numbered_by_luid_and_gone_ones_drop_out() {
        let mut page = TimeMachinePage::new();
        // Seen first "b", then "a": numbered by LUID, "a" is GPU 0.
        page.adapters = vec![
            Device { id: "b".into(), name: String::new(), total: 0 },
            Device { id: "a".into(), name: String::new(), total: 0 },
            Device { id: "c".into(), name: String::new(), total: 0 },
        ];
        let mut s = sample(10 * SEC, vec![]);
        // "b" and "a" have values; "c" is gone from every kept sample.
        Arc::get_mut(&mut s).unwrap().gpus = vec![10.0, 20.0, f32::NAN].into_boxed_slice();
        page.live = vec![s];
        assert_eq!(page.shown_adapters(), vec![(1, 0), (0, 1)]);
    }

    #[test]
    fn targets_follow_the_settings_keys() {
        let mut s = Settings::default();
        assert_eq!(targets(&s), vec![Target::Cpu, Target::Mem]);
        toggle(&mut s, Target::GpuMem);
        toggle(&mut s, Target::CpuDetail);
        toggle(&mut s, Target::Mem);
        assert_eq!(targets(&s), vec![Target::Cpu, Target::CpuDetail, Target::GpuMem]);
        assert!(targets(&s).iter().any(|t| t.uses_gpu()));
    }

    #[test]
    fn nearest_sample() {
        let data = vec![sample(10 * SEC, vec![]), sample(12 * SEC, vec![])];
        assert_eq!(TimeMachinePage::nearest(&data, 0), Some(0));
        assert_eq!(TimeMachinePage::nearest(&data, 10 * SEC + SEC / 2), Some(0));
        assert_eq!(TimeMachinePage::nearest(&data, 11 * SEC + SEC / 2), Some(1));
        assert_eq!(TimeMachinePage::nearest(&data, 99 * SEC), Some(1));
        assert_eq!(TimeMachinePage::nearest(&[], 0), None);
    }
}
