//! The table of a selected range: what each process, and each group, did over the range.
//!
//! The columns are items (CPU, memory...) times operations (start, end, change, average,
//! minimum, maximum). Which items show and which operations show are chosen separately, so
//! turning "maximum" off drops the maximum of every item. Items that only grow (CPU time,
//! I/O, page faults, cycles) have one column: how much they grew over the range.

use super::columns::{self, Col};
use super::grouping::Group;
use super::timeline::{PRec, Sample};
use crate::i18n::tr;
use crate::system::model::{Proc, Status};
use crate::system::win::SEC;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RItem {
    Name,
    Pid,
    /// Born or died in the range ("+", "-", "+-").
    Mark,
    /// When it was born or died.
    Time,
    /// Seconds not responding.
    Hung,
    Cpu,
    CpuTime,
    Cycles,
    Mem,
    Private,
    Ws,
    Commit,
    Paged,
    NonPaged,
    /// Page faults per second.
    Faults,
    /// Page faults in the range.
    PageFaults,
    Disk,
    IoReadBytes,
    IoWriteBytes,
    IoOtherBytes,
    IoReads,
    IoWrites,
    IoOther,
    Network,
    Gpu,
    GpuMem,
    GpuShared,
    Handles,
    Threads,
    UserObjects,
    GdiObjects,
}

/// How an item is summed up over the range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// One column that is not a value of the samples.
    Single,
    /// A value at each sample: any operation applies.
    Gauge,
    /// A count that only grows: the range shows how much it grew.
    Counter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ROp {
    None,
    Start,
    End,
    Change,
    Avg,
    Min,
    Max,
}

/// The operations a gauge can show, in column order.
pub const OPS: [ROp; 6] = [ROp::Start, ROp::End, ROp::Change, ROp::Avg, ROp::Min, ROp::Max];

impl ROp {
    pub fn key(self) -> &'static str {
        match self {
            ROp::None => "",
            ROp::Start => "start",
            ROp::End => "end",
            ROp::Change => "delta",
            ROp::Avg => "avg",
            ROp::Min => "min",
            ROp::Max => "max",
        }
    }

    pub fn from_key(k: &str) -> Option<ROp> {
        OPS.into_iter().find(|o| o.key() == k).or((k.is_empty()).then_some(ROp::None))
    }

    pub fn label(self) -> &'static str {
        let s = &tr().tm;
        match self {
            ROp::None => "",
            ROp::Start => s.col_start,
            ROp::End => s.col_end,
            ROp::Change => s.col_change,
            ROp::Avg => s.col_avg,
            ROp::Min => s.col_min,
            ROp::Max => s.col_max,
        }
    }

    /// Its bit in `Settings::range_ops`.
    pub fn bit(self) -> u8 {
        match OPS.iter().position(|o| *o == self) {
            Some(i) => 1 << i,
            None => 0,
        }
    }
}

/// A column of the range table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RCol {
    pub item: RItem,
    pub op: ROp,
}

impl RCol {
    pub fn key(self) -> String {
        if self.op == ROp::None {
            self.item.key().to_string()
        } else {
            format!("{}:{}", self.item.key(), self.op.key())
        }
    }

    pub fn from_key(k: &str) -> Option<RCol> {
        let (item, op) = k.split_once(':').unwrap_or((k, ""));
        Some(RCol { item: RItem::from_key(item)?, op: ROp::from_key(op)? })
    }

    /// The heading of the column's own cell: the operation, or the item for a single column.
    pub fn label(self) -> String {
        match self.item.kind() {
            Kind::Single => self.item.label(),
            Kind::Counter => tr().tm.col_amount.to_string(),
            Kind::Gauge => self.op.label().to_string(),
        }
    }

    pub fn right(self) -> bool {
        !matches!(self.item, RItem::Name | RItem::Mark | RItem::Time)
    }

    /// Text columns open ascending, numbers descending.
    pub fn default_descending(self) -> bool {
        !matches!(self.item, RItem::Name | RItem::Pid | RItem::Mark)
    }

    /// The technical name, for the detailed copy format.
    pub fn prop(self) -> String {
        format!("Range.{}", self.key())
    }
}

/// Every item, in the order of the column picker.
pub const ITEMS: [RItem; 31] = [
    RItem::Name,
    RItem::Pid,
    RItem::Mark,
    RItem::Time,
    RItem::Hung,
    RItem::Cpu,
    RItem::CpuTime,
    RItem::Cycles,
    RItem::Mem,
    RItem::Private,
    RItem::Ws,
    RItem::Commit,
    RItem::Paged,
    RItem::NonPaged,
    RItem::Faults,
    RItem::PageFaults,
    RItem::Disk,
    RItem::IoReadBytes,
    RItem::IoWriteBytes,
    RItem::IoOtherBytes,
    RItem::IoReads,
    RItem::IoWrites,
    RItem::IoOther,
    RItem::Network,
    RItem::Gpu,
    RItem::GpuMem,
    RItem::GpuShared,
    RItem::Handles,
    RItem::Threads,
    RItem::UserObjects,
    RItem::GdiObjects,
];

/// The groups of the column picker: a heading and the items under it.
pub const PICKER: [(crate::i18n::Text, &[RItem]); 6] = [
    (|s| s.details.group_basic, &[RItem::Name, RItem::Pid, RItem::Mark, RItem::Time, RItem::Hung]),
    (|_| "CPU", &[RItem::Cpu, RItem::CpuTime, RItem::Cycles]),
    (
        |s| s.common.memory,
        &[RItem::Mem, RItem::Private, RItem::Ws, RItem::Commit, RItem::Paged, RItem::NonPaged, RItem::Faults, RItem::PageFaults],
    ),
    (
        |s| s.details.group_io,
        &[RItem::Disk, RItem::IoReadBytes, RItem::IoWriteBytes, RItem::IoOtherBytes, RItem::IoReads, RItem::IoWrites, RItem::IoOther, RItem::Network],
    ),
    (|_| "GPU", &[RItem::Gpu, RItem::GpuMem, RItem::GpuShared]),
    (|s| s.details.group_objects, &[RItem::Handles, RItem::Threads, RItem::UserObjects, RItem::GdiObjects]),
];

enum Unit {
    None,
    Percent,
    Mb,
    MbPerSec,
    KbPerSec,
    Seconds,
    PerSec,
}

impl RItem {
    pub fn key(self) -> &'static str {
        match self {
            RItem::Name => "name",
            RItem::Pid => "pid",
            RItem::Mark => "mark",
            RItem::Time => "time",
            RItem::Hung => "hung",
            RItem::Cpu => "cpu",
            RItem::CpuTime => "cputime",
            RItem::Cycles => "cycles",
            RItem::Mem => "mem",
            RItem::Private => "private",
            RItem::Ws => "ws",
            RItem::Commit => "commit",
            RItem::Paged => "paged",
            RItem::NonPaged => "nppool",
            RItem::Faults => "faults",
            RItem::PageFaults => "pf",
            RItem::Disk => "disk",
            RItem::IoReadBytes => "ioreadb",
            RItem::IoWriteBytes => "iowriteb",
            RItem::IoOtherBytes => "iootherb",
            RItem::IoReads => "ioread",
            RItem::IoWrites => "iowrite",
            RItem::IoOther => "ioother",
            RItem::Network => "network",
            RItem::Gpu => "gpu",
            RItem::GpuMem => "gpumem",
            RItem::GpuShared => "gpushared",
            RItem::Handles => "handles",
            RItem::Threads => "threads",
            RItem::UserObjects => "userobj",
            RItem::GdiObjects => "gdiobj",
        }
    }

    pub fn from_key(k: &str) -> Option<RItem> {
        ITEMS.into_iter().find(|i| i.key() == k)
    }

    pub fn kind(self) -> Kind {
        match self {
            RItem::Name | RItem::Pid | RItem::Mark | RItem::Time | RItem::Hung => Kind::Single,
            RItem::CpuTime
            | RItem::Cycles
            | RItem::PageFaults
            | RItem::IoReadBytes
            | RItem::IoWriteBytes
            | RItem::IoOtherBytes
            | RItem::IoReads
            | RItem::IoWrites
            | RItem::IoOther => Kind::Counter,
            _ => Kind::Gauge,
        }
    }

    /// The process page's column it shows, for its name.
    fn col(self) -> Option<Col> {
        Some(match self {
            RItem::Name => Col::Name,
            RItem::Pid => Col::Pid,
            RItem::Cpu => Col::Cpu,
            RItem::CpuTime => Col::CpuTime,
            RItem::Cycles => Col::Cycles,
            RItem::Mem => Col::ActivePrivate,
            RItem::Private => Col::Private,
            RItem::Ws => Col::Ws,
            RItem::Commit => Col::Commit,
            RItem::Paged => Col::Paged,
            RItem::NonPaged => Col::NonPaged,
            RItem::PageFaults => Col::PageFaults,
            RItem::Disk => Col::DiskRate,
            RItem::IoReadBytes => Col::IoReadBytes,
            RItem::IoWriteBytes => Col::IoWriteBytes,
            RItem::IoOtherBytes => Col::IoOtherBytes,
            RItem::IoReads => Col::IoReads,
            RItem::IoWrites => Col::IoWrites,
            RItem::IoOther => Col::IoOther,
            RItem::Network => Col::Network,
            RItem::Gpu => Col::GpuUsage,
            RItem::GpuMem => Col::GpuDedicated,
            RItem::GpuShared => Col::GpuShared,
            RItem::Handles => Col::Handles,
            RItem::Threads => Col::Threads,
            RItem::UserObjects => Col::UserObjects,
            RItem::GdiObjects => Col::GdiObjects,
            _ => return None,
        })
    }

    fn unit(self) -> Unit {
        match self {
            RItem::Cpu | RItem::Gpu => Unit::Percent,
            RItem::Mem
            | RItem::Private
            | RItem::Ws
            | RItem::Commit
            | RItem::Paged
            | RItem::NonPaged
            | RItem::GpuMem
            | RItem::GpuShared
            | RItem::IoReadBytes
            | RItem::IoWriteBytes
            | RItem::IoOtherBytes => Unit::Mb,
            RItem::Disk => Unit::MbPerSec,
            RItem::Network => Unit::KbPerSec,
            RItem::CpuTime | RItem::Hung => Unit::Seconds,
            RItem::Faults => Unit::PerSec,
            _ => Unit::None,
        }
    }

    fn base_label(self) -> &'static str {
        match self {
            RItem::Mark => "+-",
            RItem::Time => tr().tm.time,
            RItem::Hung => tr().common.not_responding,
            RItem::Faults => tr().common.page_faults,
            _ => self.col().map_or("", |c| columns::def(c).label()),
        }
    }

    /// The heading over the item's columns, with its unit.
    pub fn label(self) -> String {
        let base = self.base_label();
        let s = &tr().tm;
        let unit = match self.unit() {
            Unit::None => return base.to_string(),
            Unit::Percent => "%",
            Unit::Mb => "MB",
            Unit::MbPerSec => s.mb_per_sec,
            Unit::KbPerSec => s.kb_per_sec,
            Unit::Seconds => s.seconds_unit,
            Unit::PerSec => tr().units.per_sec,
        };
        format!("{base} [{unit}]")
    }

    /// The name in the column picker.
    pub fn full(self) -> String {
        match self.col() {
            Some(c) if self != RItem::Faults => {
                let full = columns::def(c).full();
                match self.unit() {
                    Unit::None => full.to_string(),
                    _ => self.label().replacen(self.base_label(), full, 1),
                }
            }
            _ => self.label(),
        }
    }

    pub fn default_width(self) -> u16 {
        match self {
            RItem::Name => 200,
            RItem::Pid => 70,
            RItem::Mark => 44,
            RItem::Time => 150,
            RItem::Hung => 110,
            // An item that only grows has one column, which also carries the item's name above it.
            RItem::CpuTime | RItem::IoReads | RItem::IoWrites | RItem::IoOther => 110,
            RItem::Cycles | RItem::PageFaults => 130,
            RItem::IoReadBytes | RItem::IoWriteBytes | RItem::IoOtherBytes => 170,
            RItem::Mem | RItem::Private | RItem::Ws | RItem::Commit | RItem::GpuMem | RItem::GpuShared => 76,
            _ => 64,
        }
    }

    fn decimals(self) -> usize {
        match self.unit() {
            Unit::Percent if self == RItem::Gpu => 0,
            Unit::Percent | Unit::Mb | Unit::MbPerSec | Unit::Seconds => 1,
            _ => 0,
        }
    }

    /// The value of a record in base units: percent, bytes, bytes per second, seconds, counts.
    pub fn raw(self, r: &PRec, secs: f32) -> f64 {
        let kb = |v: u32| v as f64 * 1024.0;
        match self {
            RItem::Cpu => r.cpu as f64 / 100.0,
            RItem::CpuTime => r.cpu_cs as f64 / 100.0,
            RItem::Cycles => r.cycles as f64,
            RItem::Mem => kb(r.active_kb),
            RItem::Private => kb(r.private_kb),
            RItem::Ws => kb(r.ws_kb),
            RItem::Commit => kb(r.commit_kb),
            RItem::Paged => kb(r.paged_kb),
            RItem::NonPaged => kb(r.nonpaged_kb),
            RItem::Faults => r.pf_delta as f64 / secs.max(0.001) as f64,
            RItem::PageFaults => r.page_faults as f64,
            RItem::Disk => kb(r.disk_kbps),
            RItem::IoReadBytes => r.io_read_bytes as f64,
            RItem::IoWriteBytes => r.io_write_bytes as f64,
            RItem::IoOtherBytes => r.io_other_bytes as f64,
            RItem::IoReads => r.io_reads as f64,
            RItem::IoWrites => r.io_writes as f64,
            RItem::IoOther => r.io_other as f64,
            RItem::Network => r.net_bps as f64,
            RItem::Gpu => r.gpu as f64 / 100.0,
            RItem::GpuMem => kb(r.gpu_ded_kb),
            RItem::GpuShared => kb(r.gpu_shared_kb),
            RItem::Handles => r.handles as f64,
            RItem::Threads => r.threads as f64,
            RItem::UserObjects => r.user_objects as f64,
            RItem::GdiObjects => r.gdi_objects as f64,
            RItem::Name | RItem::Pid | RItem::Mark | RItem::Time | RItem::Hung => 0.0,
        }
    }

    /// A base value in the table's units.
    pub fn shown(self, raw: f64) -> f64 {
        let mb = 1024.0 * 1024.0;
        match self.unit() {
            Unit::Mb | Unit::MbPerSec => raw / mb,
            Unit::KbPerSec => raw / 1024.0,
            _ => raw,
        }
    }

    pub fn text(self, raw: f64) -> String {
        let v = self.shown(raw);
        let d = self.decimals();
        if d == 0 && v.abs() >= 1000.0 {
            return grouped(v.round() as i64);
        }
        format!("{v:.d$}")
    }

    /// A signed change; one that rounds to zero is shown without a sign.
    pub fn delta(self, raw: f64) -> String {
        let v = self.shown(raw);
        let d = self.decimals();
        if v.abs() < 0.5 * 10f64.powi(-(d as i32)) {
            return format!("{:.d$}", 0.0);
        }
        let body = self.text(raw.abs());
        format!("{}{body}", if v < 0.0 { '-' } else { '+' })
    }
}

fn grouped(v: i64) -> String {
    let s = v.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    if v < 0 {
        out.push('-');
    }
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The default items of the range table.
pub fn default_items() -> Vec<(RItem, u16)> {
    [RItem::Name, RItem::Pid, RItem::Mark, RItem::Time, RItem::Cpu, RItem::Mem]
        .into_iter()
        .map(|i| (i, i.default_width()))
        .collect()
}

/// Start, end, change, average and maximum, as the range table had before minimum came.
pub const DEFAULT_OPS: u8 = 0b10_1111;

/// The columns the items and operations make, with each column's width.
pub fn visible(items: &[(RItem, u16)], ops: u8) -> Vec<(RCol, u16)> {
    let mut out = Vec::new();
    for (item, w) in items {
        match item.kind() {
            Kind::Single => out.push((RCol { item: *item, op: ROp::None }, *w)),
            Kind::Counter => out.push((RCol { item: *item, op: ROp::Change }, *w)),
            Kind::Gauge => {
                for op in OPS.into_iter().filter(|o| ops & o.bit() != 0) {
                    out.push((RCol { item: *item, op }, *w));
                }
            }
        }
    }
    out
}

/// One item of one row over the range.
#[derive(Clone, Copy)]
pub struct Acc {
    start: f64,
    end: f64,
    min: f64,
    max: f64,
    sum: f64,
    n: u32,
}

impl Default for Acc {
    fn default() -> Self {
        Self { start: 0.0, end: 0.0, min: f64::MAX, max: f64::MIN, sum: 0.0, n: 0 }
    }
}

impl Acc {
    fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.start = v;
        }
        self.end = v;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        self.sum += v;
        self.n += 1;
    }

    fn value(&self, op: ROp) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        match op {
            ROp::Start => self.start,
            ROp::End => self.end,
            ROp::Change => self.end - self.start,
            ROp::Avg => self.sum / self.n as f64,
            ROp::Min => self.min,
            ROp::Max => self.max,
            ROp::None => 0.0,
        }
    }
}

/// Births, deaths and time not responding of one process in the range.
#[derive(Clone, Copy, Default)]
pub struct Life {
    /// Not there at the first sample of the range.
    pub born: bool,
    /// Gone by the last sample of the range.
    pub died: bool,
    /// Its creation time, or the first sample it was seen at.
    pub born_at: i64,
    /// The first sample it was missing from.
    pub died_at: i64,
    /// Time spent not responding, in FILETIME units.
    pub hung: i64,
}

/// A row of the range table: a process, or a group's totals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Proc(u32),
    Group(u32),
}

/// Everything the range table shows besides the processes themselves. Rows are in the
/// order of the process list `build` returns.
pub struct RangeData {
    pub life: Vec<Life>,
    /// The value items, in the order of `accs`' rows.
    pub items: Vec<RItem>,
    /// Per process, then per item.
    accs: Vec<Acc>,
    /// Per group, then per item; filled by `group`.
    group_accs: Vec<Acc>,
    group_life: Vec<Life>,
    range: Vec<Arc<Sample>>,
    /// The process of each record: by the record's identity.
    index: HashMap<*const super::timeline::Ident, u32>,
}

impl RangeData {
    /// Sums up a range of samples for the given items. Also returns one process per process
    /// seen in the range, as at its last sample there.
    pub fn build(range: &[Arc<Sample>], items: &[RItem], to_proc: impl Fn(&PRec) -> Proc) -> (RangeData, Vec<Proc>) {
        let items: Vec<RItem> = items.iter().copied().filter(|i| i.kind() != Kind::Single).collect();
        let k = items.len();
        let mut index: HashMap<*const super::timeline::Ident, u32> = HashMap::new();
        let mut last: Vec<(usize, usize)> = Vec::new();
        let mut first: Vec<usize> = Vec::new();
        let mut life: Vec<Life> = Vec::new();
        let mut accs: Vec<Acc> = Vec::new();
        for (si, sample) in range.iter().enumerate() {
            // A hung sample counts for the time since the one before it.
            let step = match si {
                0 if range.len() > 1 => range[1].time - range[0].time,
                0 => 0,
                _ => sample.time - range[si - 1].time,
            };
            // The idle "process" is the processors' free time, not a load: it would top every column.
            for (ri, r) in sample.procs.iter().enumerate().filter(|(_, r)| r.id.pid != 0) {
                let key = Arc::as_ptr(&r.id);
                let p = *index.entry(key).or_insert_with(|| {
                    last.push((si, ri));
                    first.push(si);
                    life.push(Life::default());
                    accs.extend(std::iter::repeat_n(Acc::default(), k));
                    (last.len() - 1) as u32
                }) as usize;
                last[p] = (si, ri);
                for (j, item) in items.iter().enumerate() {
                    accs[p * k + j].add(item.raw(r, sample.secs));
                }
                if r.status() == Status::NotResponding {
                    life[p].hung += step;
                }
            }
        }
        let n = range.len();
        let mut procs = Vec::with_capacity(last.len());
        for (p, (si, ri)) in last.iter().enumerate() {
            let r = &range[*si].procs[*ri];
            procs.push(to_proc(r));
            let l = &mut life[p];
            l.born = first[p] > 0;
            l.died = si + 1 < n;
            l.born_at = if r.id.create_time > 0 { r.id.create_time } else { range[first[p]].time };
            l.died_at = if l.died { range[si + 1].time } else { 0 };
        }
        (RangeData { life, items, accs, group_accs: Vec::new(), group_life: Vec::new(), range: range.to_vec(), index }, procs)
    }

    /// Sums up each group: its members' values added at each sample, then that series over
    /// the range. So a group's maximum is the heaviest moment of the whole group.
    pub fn group(&mut self, groups: &[Group]) {
        let k = self.items.len();
        let g = groups.len();
        let mut of = vec![u32::MAX; self.life.len()];
        for (gi, group) in groups.iter().enumerate() {
            for m in &group.members {
                of[*m as usize] = gi as u32;
            }
        }
        self.group_accs = vec![Acc::default(); g * k];
        self.group_life = vec![Life::default(); g];
        let mut sums = vec![0.0f64; g * k];
        let mut present = vec![false; g];
        for sample in self.range.iter() {
            sums.iter_mut().for_each(|v| *v = 0.0);
            present.iter_mut().for_each(|v| *v = false);
            for r in sample.procs.iter() {
                let Some(&p) = self.index.get(&Arc::as_ptr(&r.id)) else { continue };
                let gi = of[p as usize];
                if gi == u32::MAX {
                    continue;
                }
                let gi = gi as usize;
                present[gi] = true;
                for (j, item) in self.items.iter().enumerate() {
                    if item.kind() == Kind::Gauge {
                        sums[gi * k + j] += item.raw(r, sample.secs);
                    }
                }
            }
            for gi in 0..g {
                if present[gi] {
                    for j in 0..k {
                        if self.items[j].kind() == Kind::Gauge {
                            self.group_accs[gi * k + j].add(sums[gi * k + j]);
                        }
                    }
                }
            }
        }
        // Counters: the members' growth added up.
        for (gi, group) in groups.iter().enumerate() {
            for (j, item) in self.items.iter().enumerate() {
                if item.kind() == Kind::Counter {
                    let total: f64 = group.members.iter().map(|m| self.accs[*m as usize * k + j].value(ROp::Change)).sum();
                    self.group_accs[gi * k + j] = Acc { start: 0.0, end: total, min: 0.0, max: total, sum: total, n: 1 };
                }
            }
            let l = &mut self.group_life[gi];
            l.born = group.members.iter().all(|m| self.life[*m as usize].born);
            l.died = group.members.iter().all(|m| self.life[*m as usize].died);
            l.hung = group.members.iter().map(|m| self.life[*m as usize].hung).max().unwrap_or(0);
            l.born_at = group.members.iter().map(|m| self.life[*m as usize].born_at).min().unwrap_or(0);
            l.died_at = group.members.iter().map(|m| self.life[*m as usize].died_at).max().unwrap_or(0);
        }
    }

    pub fn life(&self, w: Which) -> Life {
        match w {
            Which::Proc(i) => self.life[i as usize],
            Which::Group(g) => self.group_life.get(g as usize).copied().unwrap_or_default(),
        }
    }

    /// The value of a column in base units; None for columns that are not values.
    pub fn value(&self, w: Which, col: RCol) -> Option<f64> {
        let j = self.items.iter().position(|i| *i == col.item)?;
        let k = self.items.len();
        let acc = match w {
            Which::Proc(i) => self.accs.get(i as usize * k + j)?,
            Which::Group(g) => self.group_accs.get(g as usize * k + j)?,
        };
        Some(acc.value(col.op))
    }

    /// The sort key of a column.
    pub fn key(&self, w: Which, col: RCol) -> f64 {
        let l = self.life(w);
        match col.item {
            RItem::Mark => (l.born as u8 * 2 + l.died as u8) as f64,
            RItem::Time => match (l.born, l.died) {
                (true, _) => l.born_at as f64,
                (false, true) => l.died_at as f64,
                _ => 0.0,
            },
            RItem::Hung => l.hung as f64,
            _ => self.value(w, col).unwrap_or(0.0),
        }
    }

    pub fn mark(&self, w: Which) -> &'static str {
        let l = self.life(w);
        match (l.born, l.died) {
            (true, true) => "+-",
            (true, false) => "+",
            (false, true) => "-",
            (false, false) => "",
        }
    }

    /// The text of a value column (not name, PID, time).
    pub fn text(&self, w: Which, col: RCol) -> String {
        match col.item {
            RItem::Mark => self.mark(w).to_string(),
            RItem::Hung => {
                let secs = self.life(w).hung as f64 / SEC as f64;
                if secs > 0.0 {
                    format!("{secs:.0}")
                } else {
                    String::new()
                }
            }
            _ => match self.value(w, col) {
                Some(v) if col.op == ROp::Change && col.item.kind() == Kind::Gauge => col.item.delta(v),
                Some(v) => col.item.text(v),
                None => String::new(),
            },
        }
    }

    /// A cell for a file: plain numbers in base units (ratios from 0 to 1, bytes, seconds).
    pub fn export(&self, p: &Proc, col: RCol, w: Which, offset: i64) -> String {
        use super::timeline::export::{iso, number};
        let l = self.life(w);
        match col.item {
            RItem::Name => p.stat.name.clone(),
            RItem::Pid => p.pid.to_string(),
            RItem::Mark => self.mark(w).to_string(),
            RItem::Time => match (l.born, l.died) {
                (true, _) => iso(l.born_at, offset),
                (false, true) => iso(l.died_at, offset),
                _ => String::new(),
            },
            RItem::Hung => number(l.hung as f64 / SEC as f64, 1),
            RItem::Cpu | RItem::Gpu => number(self.value(w, col).unwrap_or(f64::NAN) / 100.0, 6),
            _ => number(self.value(w, col).unwrap_or(f64::NAN), 2),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::details::timeline::tests::{rec, sample};

    fn data(range: &[Arc<Sample>], items: &[RItem]) -> (RangeData, Vec<Proc>) {
        RangeData::build(range, items, crate::pages::details::timeline::tests::proc_of)
    }

    #[test]
    fn ops_and_counters() {
        let range = vec![
            sample(10 * SEC, vec![rec(1, 10.0, 100), rec(2, 5.0, 0)]),
            sample(11 * SEC, vec![rec(1, 30.0, 150), rec(3, 7.0, 0)]),
            sample(12 * SEC, vec![rec(1, 20.0, 400), rec(3, 9.0, 0)]),
        ];
        let (d, procs) = data(&range, &[RItem::Name, RItem::Cpu, RItem::CpuTime]);
        let p = |pid: u32| procs.iter().position(|p| p.pid == pid).unwrap() as u32;
        let cpu = |op| RCol { item: RItem::Cpu, op };
        let w = Which::Proc(p(1));
        assert_eq!(d.text(w, cpu(ROp::Start)), "10.0");
        assert_eq!(d.text(w, cpu(ROp::End)), "20.0");
        assert_eq!(d.text(w, cpu(ROp::Change)), "+10.0");
        assert_eq!(d.text(w, cpu(ROp::Avg)), "20.0");
        assert_eq!(d.text(w, cpu(ROp::Min)), "10.0");
        assert_eq!(d.text(w, cpu(ROp::Max)), "30.0");
        // CPU time grew from 1.00 s to 4.00 s.
        assert_eq!(d.text(w, RCol { item: RItem::CpuTime, op: ROp::Change }), "3.0");
        assert_eq!(d.mark(Which::Proc(p(2))), "-");
        assert_eq!(d.mark(Which::Proc(p(3))), "+");
        assert_eq!(d.life(Which::Proc(p(2))).died_at, 11 * SEC);
        // Average only over the samples where the process existed.
        assert_eq!(d.text(Which::Proc(p(3)), cpu(ROp::Avg)), "8.0");
    }

    #[test]
    fn a_group_sums_at_each_sample_before_summing_up() {
        // 1 peaks at the first sample, 2 at the second: the group's peak is not 30 + 30.
        let range = vec![
            sample(10 * SEC, vec![rec(1, 30.0, 0), rec(2, 10.0, 0)]),
            sample(11 * SEC, vec![rec(1, 10.0, 0), rec(2, 30.0, 0)]),
        ];
        let (mut d, procs) = data(&range, &[RItem::Cpu]);
        let members: Vec<u32> = (0..procs.len() as u32).collect();
        let summary = procs[0].clone();
        d.group(&[Group { id: crate::pages::details::grouping::GroupId::Tree(1, 1), members, summary }]);
        assert_eq!(d.text(Which::Group(0), RCol { item: RItem::Cpu, op: ROp::Max }), "40.0");
        assert_eq!(d.text(Which::Group(0), RCol { item: RItem::Cpu, op: ROp::Min }), "40.0");
    }

    #[test]
    fn columns_follow_items_and_ops() {
        let items = vec![(RItem::Name, 200), (RItem::Cpu, 60), (RItem::CpuTime, 70)];
        let cols = visible(&items, ROp::Max.bit() | ROp::Min.bit());
        let keys: Vec<String> = cols.iter().map(|(c, _)| c.key()).collect();
        assert_eq!(keys, ["name", "cpu:min", "cpu:max", "cputime:delta"]);
        assert_eq!(RCol::from_key("cpu:max"), Some(RCol { item: RItem::Cpu, op: ROp::Max }));
        assert_eq!(RCol::from_key("pid"), Some(RCol { item: RItem::Pid, op: ROp::None }));
    }
}
