//! The graph above the process table: it keeps the last few minutes of samples in memory,
//! so the machine's load can be looked at after the fact, and the table can show any past
//! moment or sum up a range.
//!
//! Samples are kept in a ring that drops the oldest ones. Selecting a time freezes the view
//! on a copy of the ring (the samples are shared, so only pointers are copied); recording
//! goes on in the ring meanwhile, and resuming shows it again.
//!
//! Each sample holds the machine-wide values the graph plots, and a small record per
//! process: the facts that never change live once per process (`Ident`), and the values
//! are packed into small integers (`PRec`).

mod dialog;
pub mod export;
mod graph;
pub mod target;

use crate::i18n::tr;
use crate::pages::{Cmd, Cx, Ev};
use crate::settings::Settings;
use crate::system::model::{Proc, ProcStatic, Snapshot, Status};
use crate::system::win::{days_from_civil, filetime_now, SEC};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons::{self, glyph};
use crate::ui::keys::*;
use crate::ui::theme::THEME;
use crate::ui::widgets::*;
use dialog::{Dialog, OffConfirm};
use export::{Encoding, Format, Kind};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use target::targets;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

/// The view never zooms in further than this many seconds.
const MIN_SPAN_SECONDS: i64 = 10;
const ZOOM_STEP: f64 = 1.25;
/// The longest retention period, in minutes (one day).
pub const MAX_MINUTES: u32 = 1440;
/// A drag shorter than this many pixels selects a single time.
const CLICK_SLOP: i32 = 4;
/// Samples measured in this long after start-up are not recorded: they mostly show
/// this app's own start-up (reading icons and descriptions of every executable).
const WARMUP: Duration = Duration::from_secs(2);
/// No adapter or engine type in a record.
pub const NONE: u8 = u8::MAX;

/// Picks one series' value out of a sample.
type SeriesFn = dyn Fn(&Sample) -> Option<f32>;

/// What never changes about a process, kept once however many samples it is in.
pub struct Ident {
    pub stat: Arc<ProcStatic>,
    pub pid: u32,
    pub ppid: u32,
    pub create_time: i64,
    pub session: u32,
}

/// One process at one sample, packed small.
pub struct PRec {
    pub id: Arc<Ident>,
    pub cycles: u64,
    pub io_reads: u64,
    pub io_writes: u64,
    pub io_other: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub io_other_bytes: u64,
    /// CPU time in hundredths of a second.
    pub cpu_cs: u32,
    pub ws_kb: u32,
    pub peak_ws_kb: u32,
    pub private_kb: u32,
    pub active_kb: u32,
    pub commit_kb: u32,
    pub paged_kb: u32,
    pub nonpaged_kb: u32,
    pub gpu_ded_kb: u32,
    pub gpu_shared_kb: u32,
    pub ws_delta_kb: i32,
    pub page_faults: u32,
    pub pf_delta: u32,
    pub handles: u32,
    pub threads: u32,
    pub disk_kbps: u32,
    pub net_bps: u32,
    pub user_objects: u16,
    pub gdi_objects: u16,
    /// CPU and GPU use in hundredths of a percent.
    pub cpu: u16,
    pub gpu: u16,
    /// The busiest GPU engine: its adapter and its type, as indices into the timeline's
    /// lists, or `NONE`.
    pub gpu_adapter: u8,
    pub gpu_kind: u8,
    /// Bits 0-1 the status, bit 2 a window, bits 3-4 power throttling (0 unknown, 1 off, 2 on).
    pub flags: u8,
    pub priority: i8,
}

impl PRec {
    pub fn status(&self) -> Status {
        match self.flags & 3 {
            1 => Status::Suspended,
            2 => Status::NotResponding,
            _ => Status::Running,
        }
    }

    fn eco_qos(&self) -> Option<bool> {
        match (self.flags >> 3) & 3 {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }
}

/// The machine at one moment, and every process in it.
pub struct Sample {
    /// FILETIME (UTC).
    pub time: i64,
    /// Seconds the rates were measured over.
    pub secs: f32,
    pub cpu: f32,
    /// Kernel time, in percent of all processors.
    pub kernel: f32,
    /// Clock speed in MHz, 0 when unknown.
    pub mhz: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub commit: u64,
    pub commit_limit: u64,
    /// Active time per disk, in the timeline's disk order; NaN where a disk is missing.
    pub disks: Box<[f32]>,
    /// Utilization per adapter, in the timeline's adapter order; NaN where an adapter is missing.
    pub gpus: Box<[f32]>,
    /// Dedicated memory in use per adapter, in bytes; 0 where an adapter is missing.
    pub gpu_mem: Box<[u64]>,
    pub procs: Box<[PRec]>,
}

impl Sample {
    pub fn mem_pct(&self) -> f32 {
        self.mem_used as f32 / self.mem_total.max(1) as f32 * 100.0
    }

    /// The memory it takes.
    fn bytes(&self) -> usize {
        // The sample, its Arc's counts, and its pointer in the ring.
        std::mem::size_of::<Sample>()
            + 16
            + 8
            + self.procs.len() * std::mem::size_of::<PRec>()
            + self.disks.len() * 4
            + self.gpus.len() * 4
            + self.gpu_mem.len() * 8
    }
}

/// A disk or a GPU adapter seen in the samples. Its index in the timeline's list is fixed.
pub struct Device {
    pub id: String,
    pub name: String,
    /// Dedicated memory of a GPU adapter, 0 when unknown.
    pub total: u64,
}

/// What the page should do after the timeline took an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Not for the timeline.
    Pass,
    /// Taken; nothing for the page to do.
    Taken,
    /// The selection changed (or was cleared): the table shows something else.
    Selection,
    /// The export dialog's choices changed: the page works out the size.
    ExportChanged,
    /// Write the file.
    ExportStart,
}

/// The export dialog: what to write, and its hit areas.
pub struct ExportDialog {
    pub scope: Scope,
    pub kind: Kind,
    pub format: Format,
    pub encoding: Encoding,
    /// The rough size of the file, for the choices it was worked out for.
    pub estimate: Option<((Scope, Kind, Format, Encoding), u64)>,
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

/// Which samples the export writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The selected time or range.
    Selection,
    /// Everything kept when the view was paused.
    All,
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

    /// The choices the size was or should be worked out for.
    pub fn choices(&self) -> (Scope, Kind, Format, Encoding) {
        (self.scope, self.kind, self.format, self.encoding)
    }
}

const ACT_SETTINGS: u32 = 1;
const ACT_OFF: u32 = 2;

pub struct Timeline {
    live: Vec<Arc<Sample>>,
    /// The ring as it was when the view was paused.
    frozen: Option<Vec<Arc<Sample>>>,
    /// Each process seen, while a sample still holds it.
    idents: HashMap<(u32, i64), Arc<Ident>>,
    /// GPU adapters and disks in the order they first appeared; samples index into these.
    pub adapters: Vec<Device>,
    pub disks: Vec<Device>,
    /// GPU engine types, like "3D"; records index into this.
    kinds: Vec<String>,
    /// The memory the samples in the ring take.
    bytes: usize,
    /// Local time minus UTC, in seconds.
    pub offset: i64,
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
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    confirm: Option<OffConfirm>,
    pub export: Option<ExportDialog>,
    /// The bar shown while paused: its buttons (export, resume).
    bar: Toolbar,
    last_procs: usize,
    /// When the app started; see `WARMUP`.
    started: Instant,
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

fn duration_text(seconds: i64) -> String {
    if seconds < 60 {
        (tr().units.seconds)(seconds as u64)
    } else if seconds % 60 == 0 {
        (tr().units.minutes)(seconds as u64 / 60)
    } else {
        (tr().units.min_sec)(seconds as u64 / 60, seconds as u64 % 60)
    }
}

fn kb(b: u64) -> u32 {
    (b / 1024).min(u32::MAX as u64) as u32
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self {
            live: Vec::new(),
            frozen: None,
            idents: HashMap::new(),
            adapters: Vec::new(),
            disks: Vec::new(),
            kinds: Vec::new(),
            bytes: 0,
            offset: local_offset(),
            span: 0,
            view_end: 0,
            sel: None,
            drag: None,
            mouse: (-1, -1),
            plot: Rect::default(),
            menu: None,
            dialog: None,
            confirm: None,
            export: None,
            bar: Toolbar::default(),
            last_procs: 0,
            started: Instant::now(),
        }
    }

    pub fn clock(&self, ft: i64) -> String {
        let s = (ft.div_euclid(SEC) + self.offset).rem_euclid(86400);
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }

    fn data(&self) -> &[Arc<Sample>] {
        match &self.frozen {
            Some(f) => f,
            None => &self.live,
        }
    }


    /// The selected samples: one for a time, several for a range.
    pub fn selection(&self) -> Option<&[Arc<Sample>]> {
        let (a, b) = self.sel?;
        self.frozen.as_ref()?.get(a..=b)
    }

    /// Everything kept when the view was paused.
    pub fn all(&self) -> &[Arc<Sample>] {
        self.frozen.as_deref().unwrap_or(&[])
    }

    /// Whether a modal dialog, a menu or the export dialog is open.
    pub fn overlay_open(&self) -> bool {
        self.menu.is_some() || self.dialog.is_some() || self.confirm.is_some() || self.export.is_some()
    }

    pub fn captures_text(&self) -> bool {
        self.dialog.is_some()
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

    /// Back to the live view. Returns whether anything changed.
    pub fn resume(&mut self) -> bool {
        let was = self.frozen.is_some() || self.sel.is_some();
        self.frozen = None;
        self.sel = None;
        self.drag = None;
        self.menu = None;
        self.export = None;
        self.span = 0;
        was
    }

    /// Drops everything recorded.
    fn discard(&mut self) {
        self.resume();
        self.live.clear();
        self.idents.clear();
        self.adapters.clear();
        self.disks.clear();
        self.kinds.clear();
        self.bytes = 0;
    }

    /// Whether the timeline keeps this snapshot: it is on, and the snapshot is past start-up.
    pub fn wants(&self, snap: &Snapshot, s: &Settings) -> bool {
        if !s.tm_enabled || !snap.has_procs || snap.sys.elapsed <= 0.0 {
            return false;
        }
        // Skip samples whose measured span began during start-up.
        let span_start = Instant::now().checked_sub(Duration::from_secs_f32(snap.sys.elapsed));
        span_start.is_some_and(|t| t >= self.started + WARMUP)
    }

    fn kind_index(&mut self, kind: &str) -> u8 {
        if kind.is_empty() {
            return NONE;
        }
        match self.kinds.iter().position(|k| k == kind) {
            Some(i) => i as u8,
            None if self.kinds.len() < NONE as usize => {
                self.kinds.push(kind.to_string());
                (self.kinds.len() - 1) as u8
            }
            None => NONE,
        }
    }

    fn adapter_index(&mut self, luid: &str) -> u8 {
        if luid.is_empty() {
            return NONE;
        }
        let i = slot(&mut self.adapters, luid, "");
        if i < NONE as usize {
            i as u8
        } else {
            NONE
        }
    }

    fn pack(&mut self, p: &Proc) -> PRec {
        let id = self
            .idents
            .entry(p.key())
            .or_insert_with(|| Arc::new(Ident { stat: p.stat.clone(), pid: p.pid, ppid: p.ppid, create_time: p.create_time, session: p.session }))
            .clone();
        let status = match p.status {
            Status::Running => 0,
            Status::Suspended => 1,
            Status::NotResponding => 2,
        };
        let eco = match p.eco_qos {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        };
        let pct = |v: f32| (v * 100.0).round().clamp(0.0, u16::MAX as f32) as u16;
        PRec {
            id,
            cycles: p.cycles,
            io_reads: p.io_reads,
            io_writes: p.io_writes,
            io_other: p.io_other,
            io_read_bytes: p.io_read_bytes,
            io_write_bytes: p.io_write_bytes,
            io_other_bytes: p.io_other_bytes,
            cpu_cs: (p.cpu_time / 100_000).min(u32::MAX as u64) as u32,
            ws_kb: kb(p.ws),
            peak_ws_kb: kb(p.peak_ws),
            private_kb: kb(p.private_ws),
            active_kb: kb(p.active_private),
            commit_kb: kb(p.commit),
            paged_kb: kb(p.paged),
            nonpaged_kb: kb(p.nonpaged),
            gpu_ded_kb: kb(p.gpu_dedicated),
            gpu_shared_kb: kb(p.gpu_shared),
            ws_delta_kb: (p.ws_delta / 1024).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            page_faults: p.page_faults,
            pf_delta: p.pf_delta,
            handles: p.handles,
            threads: p.threads,
            disk_kbps: kb(p.disk_bps),
            net_bps: p.net_bps.min(u32::MAX as u64) as u32,
            user_objects: p.user_objects.min(u16::MAX as u32) as u16,
            gdi_objects: p.gdi_objects.min(u16::MAX as u32) as u16,
            cpu: pct(p.cpu),
            gpu: pct(p.gpu_util),
            gpu_adapter: self.adapter_index(&p.gpu_luid),
            gpu_kind: self.kind_index(&p.gpu_kind),
            flags: status | (p.has_window as u8) << 2 | eco << 3,
            priority: p.base_priority.clamp(i8::MIN as i32, i8::MAX as i32) as i8,
        }
    }

    /// A record as a process of the process page.
    pub fn unpack(&self, r: &PRec) -> Proc {
        let kind = self.kinds.get(r.gpu_kind as usize).cloned().unwrap_or_default();
        let luid = self.adapters.get(r.gpu_adapter as usize).map(|a| a.id.clone()).unwrap_or_default();
        let engine = if luid.is_empty() || kind.is_empty() {
            String::new()
        } else {
            format!("GPU {} - {kind}", crate::system::sysinfo::gpu_number(&luid))
        };
        let id = &r.id;
        Proc {
            pid: id.pid,
            ppid: id.ppid,
            create_time: id.create_time,
            stat: id.stat.clone(),
            status: r.status(),
            session: id.session,
            has_window: r.flags & 4 != 0,
            cpu: r.cpu as f32 / 100.0,
            cpu_time: r.cpu_cs as u64 * 100_000,
            cycles: r.cycles,
            ws: r.ws_kb as u64 * 1024,
            peak_ws: r.peak_ws_kb as u64 * 1024,
            ws_delta: r.ws_delta_kb as i64 * 1024,
            disk_bps: r.disk_kbps as u64 * 1024,
            gpu_util: r.gpu as f32 / 100.0,
            gpu_engine: engine,
            gpu_luid: luid,
            gpu_kind: kind,
            gpu_dedicated: r.gpu_ded_kb as u64 * 1024,
            gpu_shared: r.gpu_shared_kb as u64 * 1024,
            net_bps: r.net_bps as u64,
            eco_qos: r.eco_qos(),
            private_ws: r.private_kb as u64 * 1024,
            active_private: r.active_kb as u64 * 1024,
            commit: r.commit_kb as u64 * 1024,
            paged: r.paged_kb as u64 * 1024,
            nonpaged: r.nonpaged_kb as u64 * 1024,
            page_faults: r.page_faults,
            pf_delta: r.pf_delta,
            base_priority: r.priority as i32,
            handles: r.handles,
            threads: r.threads,
            user_objects: r.user_objects as u32,
            gdi_objects: r.gdi_objects as u32,
            io_reads: r.io_reads,
            io_writes: r.io_writes,
            io_other: r.io_other,
            io_read_bytes: r.io_read_bytes,
            io_write_bytes: r.io_write_bytes,
            io_other_bytes: r.io_other_bytes,
        }
    }

    /// The process list of a sample, as the process page shows it.
    pub fn snapshot(&self, sample: &Sample) -> Snapshot {
        Snapshot {
            taken: sample.time,
            stale: false,
            procs: sample.procs.iter().map(|r| self.unpack(r)).collect(),
            has_procs: true,
            cpu_total: sample.cpu,
            mem_total: sample.mem_total,
            mem_used: sample.mem_used,
            sys: Default::default(),
        }
    }

    pub fn record(&mut self, snap: &Snapshot, s: &Settings) {
        let time = filetime_now();
        let sys = &snap.sys;
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
        let procs: Vec<PRec> = snap.procs.iter().map(|p| self.pack(p)).collect();
        self.last_procs = procs.len();
        let sample = Sample {
            time,
            secs: sys.elapsed.max(0.001),
            cpu: sys.cpu.total,
            kernel: sys.cpu.kernel,
            mhz: sys.cpu.mhz,
            mem_used: sys.mem.total.saturating_sub(sys.mem.available),
            mem_total: sys.mem.total,
            commit: sys.mem.commit,
            commit_limit: sys.mem.commit_limit,
            disks: disks.into_boxed_slice(),
            gpus: gpus.into_boxed_slice(),
            gpu_mem: gpu_mem.into_boxed_slice(),
            procs: procs.into_boxed_slice(),
        };
        self.bytes += sample.bytes();
        self.live.push(Arc::new(sample));
        self.trim(s);
        self.offset = local_offset();
    }

    /// Drops the samples older than the retention period, and the processes no sample holds.
    fn trim(&mut self, s: &Settings) {
        let Some(last) = self.live.last().map(|x| x.time) else { return };
        let cutoff = last - Self::full_span(s);
        let n = self.live.partition_point(|x| x.time < cutoff);
        if n > 0 {
            for old in self.live.drain(..n) {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
            // A paused view may still hold them; they go with it.
            self.idents.retain(|_, id| Arc::strong_count(id) > 1);
        }
    }

    /// The memory one process takes once, whatever the number of samples.
    fn ident_bytes() -> usize {
        // The Ident, its Arc's counts, and its entry in the map (key, pointer, control byte).
        std::mem::size_of::<Ident>() + 16 + std::mem::size_of::<((u32, i64), usize)>() + 1
    }

    /// The memory the record takes now: samples, and the processes they hold.
    pub fn used(&self) -> usize {
        self.bytes + self.idents.len() * Self::ident_bytes()
    }

    /// The memory the record would take over `minutes`, from what the samples take now.
    pub fn estimate(&self, s: &Settings, minutes: u32) -> f64 {
        let interval = if s.interval_ms == 0 { 1000 } else { s.interval_ms } as f64;
        let samples = minutes as f64 * 60_000.0 / interval;
        let per = if self.live.is_empty() {
            (std::mem::size_of::<Sample>() + 24 + self.last_procs.max(1) * std::mem::size_of::<PRec>()) as f64
        } else {
            self.bytes as f64 / self.live.len() as f64
        };
        // Processes come and go: those running now, and those that came and went so far,
        // at the same pace over the whole period.
        let kept = self.live.len().max(1) as f64;
        let gone = self.idents.len().saturating_sub(self.last_procs) as f64;
        let idents = self.last_procs as f64 + gone * samples / kept;
        samples * per + idents * Self::ident_bytes() as f64
    }

    /// How many samples are kept now.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    // ------------------------------------------------------------ events

    /// Turns the graph on, or asks before turning it off (unless told not to ask).
    pub fn toggle(&mut self, cx: &mut Cx) -> Out {
        *cx.dirty = true;
        if !cx.settings.tm_enabled {
            cx.settings.tm_enabled = true;
            return Out::Taken;
        }
        if cx.settings.tm_confirm_off {
            self.confirm = Some(OffConfirm::default());
            return Out::Taken;
        }
        self.turn_off(cx)
    }

    fn turn_off(&mut self, cx: &mut Cx) -> Out {
        cx.settings.tm_enabled = false;
        self.discard();
        *cx.dirty = true;
        Out::Selection
    }

    pub fn open_settings(&mut self, s: &Settings) {
        self.dialog = Some(Dialog::new(s.tm_minutes));
    }

    /// The menu of the graph button's arrow, below it.
    pub fn open_menu(&mut self, x: i32, y: i32, s: &Settings) {
        let mut items = vec![item(tr().tm.graph_settings, ACT_SETTINGS, true)];
        if s.tm_enabled {
            items.push(item(tr().tm.graph_off, ACT_OFF, true));
        }
        self.menu = Some(Menu::new(x, y, items));
    }

    /// Events for the open dialogs and menus. `Out::Pass` when none is open.
    pub fn overlay_event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        if matches!(ev, Ev::Tick { .. }) {
            return Out::Pass;
        }
        if self.confirm.is_some() {
            return self.confirm_event(ev, cx);
        }
        if self.dialog.is_some() {
            self.dialog_event(ev, cx);
            return Out::Taken;
        }
        if self.export.is_some() {
            return self.export_event(ev, cx);
        }
        if let Some(m) = &mut self.menu {
            match m.event(ev, cx.dirty) {
                MenuOut::Stay => {}
                MenuOut::Close => {
                    self.menu = None;
                    *cx.dirty = true;
                }
                MenuOut::Pick(ACT_SETTINGS) => {
                    self.menu = None;
                    self.open_settings(cx.settings);
                    *cx.dirty = true;
                }
                MenuOut::Pick(_) => {
                    self.menu = None;
                    return self.toggle(cx);
                }
            }
            return Out::Taken;
        }
        Out::Pass
    }

    /// Events on the graph and the paused bar.
    pub fn event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        if !cx.settings.tm_enabled {
            return Out::Pass;
        }
        match *ev {
            Ev::Move { x, y } => {
                let inner = self.plot.inset(1, 1);
                let was = inner.contains(self.mouse.0, self.mouse.1);
                self.mouse = (x, y);
                if self.bar.hover_at(x, y) || was || inner.contains(x, y) {
                    *cx.dirty = true;
                }
            }
            Ev::Leave => {
                self.mouse = (-1, -1);
                self.bar.hover_at(-1, -1);
                *cx.dirty = true;
            }
            Ev::Down { x, y, right: false } => {
                self.mouse = (x, y);
                match self.bar.hit(x, y) {
                    Some(0) => {
                        if self.sel.is_some() {
                            self.export = Some(ExportDialog::new(cx.settings));
                            *cx.dirty = true;
                            return Out::ExportChanged;
                        }
                        return Out::Taken;
                    }
                    Some(_) => {
                        self.resume();
                        *cx.dirty = true;
                        return Out::Selection;
                    }
                    None => {}
                }
            }
            Ev::Down { x, y, right: true } if self.plot.inset(1, 1).contains(x, y) => {
                self.menu = Some(Menu::new(x, y, vec![item(tr().tm.graph_settings, ACT_SETTINGS, true)]));
                *cx.dirty = true;
                return Out::Taken;
            }
            Ev::Key { vk: VK_ESCAPE, .. } if self.frozen.is_some() => {
                self.resume();
                *cx.dirty = true;
                return Out::Selection;
            }
            _ => {}
        }
        self.graph_event(ev, cx)
    }

    /// Hands the file to the window, which asks where to save it.
    pub fn save(&mut self, job: export::Job, cx: &mut Cx) {
        let Some(d) = self.export.take() else { return };
        cx.settings.tm_export_kind = d.kind.key().into();
        cx.settings.tm_export_format = d.format.key().into();
        cx.settings.tm_export_encoding = d.encoding.key().into();
        cx.cmds.push(Cmd::Save(crate::pages::SaveRequest {
            name: job.file_name(),
            kind: d.format.label(),
            ext: d.format.key(),
            write: Box::new(move |w| job.write(w)),
        }));
        *cx.dirty = true;
    }

    fn export_event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        let Some(d) = &mut self.export else { return Out::Pass };
        // An open drop-down takes the input first.
        if let Some((which, m)) = &mut d.list {
            let which = *which;
            match m.event(ev, cx.dirty) {
                MenuOut::Stay => {}
                MenuOut::Close => {
                    d.list = None;
                    *cx.dirty = true;
                }
                MenuOut::Pick(id) => {
                    d.list = None;
                    if which == 3 {
                        d.format = Format::ALL[id as usize];
                    } else {
                        d.encoding = Encoding::ALL[id as usize];
                    }
                    *cx.dirty = true;
                    return Out::ExportChanged;
                }
            }
            return Out::Taken;
        }
        match *ev {
            Ev::Move { x, y } => {
                let h = d.hit(x, y);
                if h != d.hover {
                    d.hover = h;
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } => {
                *cx.dirty = true;
                match d.hit(x, y) {
                    Some(i @ 0..=2) => {
                        d.kind = Kind::ALL[i as usize];
                        return Out::ExportChanged;
                    }
                    Some(i @ 7..=8) => {
                        d.scope = if i == 7 { Scope::Selection } else { Scope::All };
                        return Out::ExportChanged;
                    }
                    Some(3) => {
                        let items = Format::ALL.iter().enumerate().map(|(i, f)| item(f.label(), i as u32, true)).collect();
                        d.list = Some((3, Menu::new(d.format_box.x, d.format_box.b() + 2, items)));
                    }
                    Some(4) => {
                        let items = Encoding::ALL.iter().enumerate().map(|(i, e)| item(e.label(), i as u32, true)).collect();
                        d.list = Some((4, Menu::new(d.encoding_box.x, d.encoding_box.b() + 2, items)));
                    }
                    Some(5) => self.export = None,
                    Some(6) => return Out::ExportStart,
                    _ if !d.panel.contains(x, y) => self.export = None,
                    _ => {}
                }
            }
            Ev::Key { vk: VK_ESCAPE, .. } => {
                self.export = None;
                *cx.dirty = true;
            }
            Ev::Key { vk: VK_RETURN, .. } => return Out::ExportStart,
            _ => {}
        }
        Out::Taken
    }

    fn graph_event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        let inner = self.plot.inset(1, 1);
        let slop = if cx.touch { CLICK_SLOP * 3 } else { CLICK_SLOP };
        match *ev {
            Ev::Down { x, y, right: false } if inner.contains(x, y) => {
                self.freeze(cx.settings);
                let t = self.time_at(x, cx.settings);
                if let Some(i) = Self::nearest(self.data(), t) {
                    self.drag = Some((i, x));
                    self.sel = Some((i, i));
                }
                *cx.dirty = true;
                Out::Taken
            }
            Ev::Move { x, .. } if self.drag.is_some() => {
                let (anchor, x0) = self.drag.unwrap();
                if (x - x0).abs() >= slop {
                    let t = self.time_at(x, cx.settings);
                    if let Some(i) = Self::nearest(self.data(), t) {
                        self.sel = Some((anchor.min(i), anchor.max(i)));
                    }
                }
                *cx.dirty = true;
                Out::Taken
            }
            Ev::Up { .. } if self.drag.is_some() => {
                self.drag = None;
                *cx.dirty = true;
                Out::Selection
            }
            Ev::Wheel { dx, dy, ctrl } if inner.contains(self.mouse.0, self.mouse.1) => {
                let full = Self::full_span(cx.settings);
                if ctrl {
                    if dy == 0.0 {
                        return Out::Taken;
                    }
                    // Wheel down zooms in, up zooms out, around the time under the mouse.
                    if dy < 0.0 {
                        self.freeze(cx.settings);
                    }
                    if self.frozen.is_none() {
                        return Out::Taken;
                    }
                    let (start, end) = self.view(cx.settings);
                    let anchor = self.time_at(self.mouse.0, cx.settings);
                    let frac = (anchor - start) as f64 / (end - start).max(1) as f64;
                    let span = ((end - start) as f64 * ZOOM_STEP.powf(dy as f64)) as i64;
                    let span = span.clamp(Self::min_span(cx.settings), full);
                    self.span = span;
                    self.view_end = anchor + ((1.0 - frac) * span as f64) as i64;
                } else if self.frozen.is_some() {
                    let (start, end) = self.view(cx.settings);
                    if end - start < full {
                        let notches = dx + dy;
                        self.view_end = end - (notches as f64 * (end - start) as f64 / 10.0) as i64;
                        self.span = end - start;
                    }
                }
                *cx.dirty = true;
                Out::Taken
            }
            _ => Out::Pass,
        }
    }

    /// Selects samples as a click or a drag on the graph would.
    #[cfg(test)]
    pub fn select_for_test(&mut self, first: usize, last: usize, s: &Settings) {
        self.freeze(s);
        self.sel = Some((first, last));
    }

    /// Whether the mouse is over the plot, where a drag selects.
    pub fn over_plot(&self, x: i32, y: i32) -> bool {
        self.plot.inset(1, 1).contains(x, y)
    }

    /// The paused bar's height, or 0 while live.
    pub fn bar_height(&self, c: &Canvas) -> i32 {
        if self.frozen.is_some() {
            c.px(48)
        } else {
            0
        }
    }

    /// The bar shown while paused: a hint, export and resume.
    pub fn paint_bar(&mut self, c: &mut Canvas, area: Rect) {
        if self.frozen.is_none() {
            self.bar.rects.clear();
            return;
        }
        let specs = [btn(icons::SAVE, tr().common.export, self.sel.is_some()), btn(icons::PLAY, tr().tm.resume, true)];
        let used = self.bar.paint(c, area, &specs);
        if self.sel.is_none() {
            let hint = rect(area.x + c.px(20), area.y, (area.w - used - c.px(40)).max(0), area.h);
            c.text(tr().tm.pick_point, Font::Body, hint, THEME.text_dim, Align::Left);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::system::model::Protect;

    pub fn stat(name: &str) -> Arc<ProcStatic> {
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

    thread_local! {
        static IDENTS: std::cell::RefCell<HashMap<u32, Arc<Ident>>> = std::cell::RefCell::new(HashMap::new());
    }

    /// A record of process `pid` (the same identity each time) with a CPU use and CPU time in hundredths of a second.
    pub fn rec(pid: u32, cpu: f32, cpu_cs: u32) -> PRec {
        let id = IDENTS.with(|m| {
            m.borrow_mut()
                .entry(pid)
                .or_insert_with(|| Arc::new(Ident { stat: stat(&format!("p{pid}")), pid, ppid: 0, create_time: pid as i64 * SEC, session: 1 }))
                .clone()
        });
        PRec {
            id,
            cycles: 0,
            io_reads: 0,
            io_writes: 0,
            io_other: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            io_other_bytes: 0,
            cpu_cs,
            ws_kb: 0,
            peak_ws_kb: 0,
            private_kb: 0,
            active_kb: 1024,
            commit_kb: 2048,
            paged_kb: 0,
            nonpaged_kb: 0,
            gpu_ded_kb: 0,
            gpu_shared_kb: 0,
            ws_delta_kb: 0,
            page_faults: 0,
            pf_delta: 0,
            handles: 100 + pid,
            threads: 1,
            disk_kbps: 2048,
            net_bps: 0,
            user_objects: 0,
            gdi_objects: 0,
            cpu: (cpu * 100.0) as u16,
            gpu: 0,
            gpu_adapter: NONE,
            gpu_kind: NONE,
            flags: 0,
            priority: 8,
        }
    }

    pub fn sample(time: i64, procs: Vec<PRec>) -> Arc<Sample> {
        Arc::new(Sample {
            time,
            secs: 1.0,
            cpu: 25.0,
            kernel: 5.0,
            mhz: 4000.0,
            mem_used: 8 << 30,
            mem_total: 16 << 30,
            commit: 10 << 30,
            commit_limit: 20 << 30,
            disks: vec![12.5].into_boxed_slice(),
            gpus: vec![40.0].into_boxed_slice(),
            gpu_mem: vec![1 << 30].into_boxed_slice(),
            procs: procs.into_boxed_slice(),
        })
    }

    pub fn proc_of(r: &PRec) -> Proc {
        Timeline::new().unpack(r)
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 12), 20769);
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn records_unpack_to_what_was_packed() {
        let mut tl = Timeline::new();
        let mut p = proc_of(&rec(7, 0.0, 0));
        p.status = Status::NotResponding;
        p.has_window = true;
        p.eco_qos = Some(true);
        p.cpu = 12.34;
        p.cpu_time = 123_400_000;
        p.cycles = 1 << 40;
        p.ws = 300 << 20;
        p.active_private = 100 << 20;
        p.ws_delta = -(4 << 20);
        p.disk_bps = 10 << 20;
        p.net_bps = 12345;
        p.io_read_bytes = 5 << 40;
        p.gpu_util = 45.5;
        p.gpu_luid = "0x1".into();
        p.gpu_kind = "3D".into();
        p.base_priority = 13;
        let r = tl.pack(&p);
        let q = tl.unpack(&r);
        assert!(q.status == Status::NotResponding && q.has_window && q.eco_qos == Some(true));
        assert!((q.cpu - 12.34).abs() < 0.01 && (q.gpu_util - 45.5).abs() < 0.01);
        assert_eq!(
            (q.cpu_time, q.cycles, q.ws, q.active_private, q.ws_delta, q.disk_bps, q.net_bps, q.io_read_bytes, q.base_priority),
            (123_400_000, 1 << 40, 300 << 20, 100 << 20, -(4 << 20), 10 << 20, 12345, 5 << 40, 13)
        );
        assert_eq!((q.gpu_luid.as_str(), q.gpu_kind.as_str()), ("0x1", "3D"));
        assert!(q.gpu_engine.ends_with(" - 3D"));
        // A second record of the same process shares its identity.
        let again = tl.pack(&p);
        assert!(Arc::ptr_eq(&r.id, &again.id));
    }

    #[test]
    fn a_record_is_small() {
        assert!(std::mem::size_of::<PRec>() <= 144, "{}", std::mem::size_of::<PRec>());
    }
}
