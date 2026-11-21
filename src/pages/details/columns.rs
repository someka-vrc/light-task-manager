//! Column definitions of the details table.

use crate::i18n::{tr, Text};
use crate::system::model::Proc;
use std::cmp::Ordering;
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Col {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    WsDelta,
    ActivePrivate,
    Private,
    Arch,
    Description,
    Session,
    CpuTime,
    Cycles,
    Ws,
    PeakWs,
    SharedWs,
    Commit,
    Paged,
    NonPaged,
    PageFaults,
    PfDelta,
    Priority,
    Handles,
    Threads,
    UserObjects,
    GdiObjects,
    IoReads,
    IoWrites,
    IoOther,
    IoReadBytes,
    IoWriteBytes,
    IoOtherBytes,
    Path,
    CmdLine,
    Elevated,
    Package,
    Platform,
    UacVirt,
    Dep,
    Dpi,
    EcoQos,
    Cfg,
    ShadowStack,
    Kind,
    Publisher,
    DiskRate,
    GpuUsage,
    GpuEngine,
    GpuDedicated,
    GpuShared,
    Network,
    MemActivePct,
    MemPrivatePct,
    DiskPct,
    NetPct,
}

/// The 100% marks of the percent columns, refreshed with each snapshot.
#[derive(Clone, Copy, Default)]
pub struct Scale {
    /// Total physical memory in bytes.
    pub mem_total: u64,
    /// Disk bytes per second that count as 100%: the busiest total seen so far.
    pub disk_full: f64,
    /// Bytes per second of all adapters' link speed together.
    pub net_full: f64,
}

pub struct ColDef {
    pub id: Col,
    pub key: &'static str,
    /// Short label for the header.
    label: Text,
    /// Full label for the column picker.
    full: Text,
    pub width: u16,
    pub right: bool,
}

const fn c(
    id: Col,
    key: &'static str,
    label: Text,
    full: Text,
    width: u16,
    right: bool,
) -> ColDef {
    ColDef { id, key, label, full, width, right }
}

impl ColDef {
    /// Short label for the header.
    pub fn label(&self) -> &'static str {
        (self.label)(tr())
    }

    /// Full label for the column picker.
    pub fn full(&self) -> &'static str {
        (self.full)(tr())
    }
}

/// Indexed by `Col as usize`.
pub const ALL: [ColDef; 55] = [
    c(Col::Name, "name", |s| s.common.name, |s| s.common.name, 230, false),
    c(Col::Pid, "pid", |_| "PID", |_| "PID", 72, true),
    c(Col::Status, "status", |s| s.common.status, |s| s.common.status, 80, false),
    c(Col::User, "user", |s| s.common.user_name, |s| s.common.user_name, 130, false),
    c(Col::Cpu, "cpu", |_| "CPU", |_| "CPU", 58, true),
    c(Col::WsDelta, "wsdelta", |s| s.cols.ws_delta, |s| s.cols.ws_delta_full, 100, true),
    c(Col::ActivePrivate, "active", |s| s.cols.active, |s| s.cols.active_full, 140, true),
    c(Col::Private, "private", |s| s.cols.private, |s| s.cols.private_full, 146, true),
    c(Col::Arch, "arch", |s| s.cols.arch, |s| s.cols.arch, 110, false),
    c(Col::Description, "desc", |s| s.common.description, |s| s.common.description, 300, false),
    c(Col::Session, "session", |s| s.cols.session, |s| s.cols.session_full, 82, true),
    c(Col::CpuTime, "cputime", |s| s.common.cpu_time, |s| s.common.cpu_time, 92, true),
    c(Col::Cycles, "cycles", |s| s.cols.cycles, |s| s.cols.cycles, 150, true),
    c(Col::Ws, "ws", |s| s.cols.ws, |s| s.cols.ws, 130, true),
    c(Col::PeakWs, "peakws", |s| s.cols.peak_ws, |s| s.cols.peak_ws_full, 110, true),
    c(Col::SharedWs, "sharedws", |s| s.cols.shared_ws, |s| s.cols.shared_ws_full, 110, true),
    c(Col::Commit, "commit", |s| s.cols.commit, |s| s.cols.commit, 120, true),
    c(Col::Paged, "paged", |s| s.cols.paged, |s| s.cols.paged, 104, true),
    c(Col::NonPaged, "nppool", |s| s.cols.nppool, |s| s.cols.nppool, 96, true),
    c(Col::PageFaults, "pf", |s| s.common.page_faults, |s| s.common.page_faults, 124, true),
    c(Col::PfDelta, "pfdelta", |s| s.cols.pf_delta, |s| s.cols.pf_delta, 86, true),
    c(Col::Priority, "priority", |s| s.cols.priority, |s| s.cols.priority, 96, false),
    c(Col::Handles, "handles", |s| s.common.handles, |s| s.common.handles, 80, true),
    c(Col::Threads, "threads", |s| s.common.threads, |s| s.common.threads, 76, true),
    c(Col::UserObjects, "userobj", |s| s.cols.user_objects, |s| s.cols.user_objects, 150, true),
    c(Col::GdiObjects, "gdiobj", |s| s.cols.gdi_objects, |s| s.cols.gdi_objects, 120, true),
    c(Col::IoReads, "ioread", |s| s.cols.io_reads, |s| s.cols.io_reads, 110, true),
    c(Col::IoWrites, "iowrite", |s| s.cols.io_writes, |s| s.cols.io_writes, 110, true),
    c(Col::IoOther, "ioother", |s| s.cols.io_other, |s| s.cols.io_other, 110, true),
    c(Col::IoReadBytes, "ioreadb", |s| s.cols.io_read_bytes, |s| s.cols.io_read_bytes_full, 130, true),
    c(Col::IoWriteBytes, "iowriteb", |s| s.cols.io_write_bytes, |s| s.cols.io_write_bytes_full, 130, true),
    c(Col::IoOtherBytes, "iootherb", |s| s.cols.io_other_bytes, |s| s.cols.io_other_bytes_full, 130, true),
    c(Col::Path, "path", |s| s.cols.path, |s| s.cols.path, 360, false),
    c(Col::CmdLine, "cmdline", |s| s.cols.cmdline, |s| s.cols.cmdline, 420, false),
    c(Col::Elevated, "elevated", |s| s.cols.elevated, |s| s.cols.elevated, 90, false),
    c(Col::Package, "package", |s| s.cols.package, |s| s.cols.package, 220, false),
    c(Col::Platform, "platform", |s| s.cols.platform, |s| s.cols.platform, 110, false),
    c(Col::UacVirt, "uacvirt", |s| s.cols.uac_virt, |s| s.cols.uac_virt, 110, false),
    c(Col::Dep, "dep", |s| s.cols.dep, |s| s.cols.dep, 120, false),
    c(Col::Dpi, "dpi", |s| s.cols.dpi, |s| s.cols.dpi, 150, false),
    c(Col::EcoQos, "ecoqos", |s| s.cols.eco_qos, |s| s.cols.eco_qos, 90, false),
    c(Col::Cfg, "cfg", |s| s.cols.cfg, |s| s.cols.cfg_full, 130, false),
    c(Col::ShadowStack, "shadow", |s| s.cols.shadow, |s| s.cols.shadow_full, 180, false),
    c(Col::Kind, "kind", |s| s.common.kind, |s| s.common.kind, 150, false),
    c(Col::Publisher, "publisher", |s| s.common.publisher, |s| s.common.publisher, 220, false),
    c(Col::DiskRate, "diskrate", |s| s.common.disk, |s| s.cols.disk_full, 130, true),
    c(Col::GpuUsage, "gpu", |_| "GPU", |_| "GPU", 70, true),
    c(Col::GpuEngine, "gpuengine", |s| s.cols.gpu_engine, |s| s.cols.gpu_engine, 120, false),
    c(Col::GpuDedicated, "gpudedicated", |s| s.common.dedicated_gpu_mem, |s| s.common.dedicated_gpu_mem, 140, true),
    c(Col::GpuShared, "gpushared", |s| s.common.shared_gpu_mem, |s| s.common.shared_gpu_mem, 140, true),
    c(Col::Network, "network", |s| s.cols.network, |s| s.cols.network_full, 150, true),
    c(Col::MemActivePct, "activepct", |s| s.cols.active_pct, |s| s.cols.active_pct_full, 170, true),
    c(Col::MemPrivatePct, "privatepct", |s| s.cols.private_pct, |s| s.cols.private_pct_full, 176, true),
    c(Col::DiskPct, "diskpct", |s| s.cols.disk_pct, |s| s.cols.disk_pct_full, 130, true),
    c(Col::NetPct, "netpct", |s| s.cols.net_pct, |s| s.cols.net_pct_full, 180, true),
];

/// The groups of the column picker, in display order. Each value column is followed by its
/// percent column. Every column appears in exactly one group (see the test below).
pub const GROUPS: [(Text, &[Col]); 6] = [
    (
        |s| s.details.group_basic,
        &[
            Col::Name,
            Col::Pid,
            Col::Status,
            Col::User,
            Col::Session,
            Col::Description,
            Col::Kind,
            Col::Publisher,
            Col::Package,
            Col::Path,
            Col::CmdLine,
            Col::Arch,
            Col::Platform,
            Col::Elevated,
        ],
    ),
    (|_| "CPU", &[Col::Cpu, Col::CpuTime, Col::Cycles, Col::Priority, Col::EcoQos]),
    (
        |s| s.common.memory,
        &[
            Col::ActivePrivate,
            Col::MemActivePct,
            Col::Private,
            Col::MemPrivatePct,
            Col::SharedWs,
            Col::WsDelta,
            Col::Ws,
            Col::PeakWs,
            Col::Commit,
            Col::Paged,
            Col::NonPaged,
            Col::PageFaults,
            Col::PfDelta,
        ],
    ),
    (
        |s| s.details.group_io,
        &[
            Col::DiskRate,
            Col::DiskPct,
            Col::IoReads,
            Col::IoWrites,
            Col::IoOther,
            Col::IoReadBytes,
            Col::IoWriteBytes,
            Col::IoOtherBytes,
            Col::Network,
            Col::NetPct,
            Col::GpuUsage,
            Col::GpuEngine,
            Col::GpuDedicated,
            Col::GpuShared,
        ],
    ),
    (|s| s.details.group_objects, &[Col::Handles, Col::Threads, Col::UserObjects, Col::GdiObjects]),
    (|s| s.details.group_protection, &[Col::UacVirt, Col::Dep, Col::Dpi, Col::Cfg, Col::ShadowStack]),
];

/// All columns in the order of `GROUPS`.
pub fn display_order() -> impl Iterator<Item = Col> {
    GROUPS.iter().flat_map(|(_, cols)| cols.iter().copied())
}

/// The technical name of the property a column shows, `source.field`, for the detailed copy format.
pub fn prop(col: Col) -> &'static str {
    match col {
        Col::Name => "ProcStatic.name",
        Col::Pid => "Proc.pid",
        Col::Status => "Proc.status",
        Col::User => "ProcStatic.user",
        Col::Cpu => "Proc.cpu",
        Col::WsDelta => "Proc.ws_delta",
        Col::ActivePrivate => "Proc.active_private",
        Col::Private => "Proc.private_ws",
        Col::Arch => "ProcStatic.arch",
        Col::Description => "ImageInfo.description",
        Col::Session => "Proc.session",
        Col::CpuTime => "Proc.cpu_time",
        Col::Cycles => "Proc.cycles",
        Col::Ws => "Proc.ws",
        Col::PeakWs => "Proc.peak_ws",
        Col::SharedWs => "Proc.ws - Proc.private_ws",
        Col::Commit => "Proc.commit",
        Col::Paged => "Proc.paged",
        Col::NonPaged => "Proc.nonpaged",
        Col::PageFaults => "Proc.page_faults",
        Col::PfDelta => "Proc.pf_delta",
        Col::Priority => "Proc.base_priority",
        Col::Handles => "Proc.handles",
        Col::Threads => "Proc.threads",
        Col::UserObjects => "Proc.user_objects",
        Col::GdiObjects => "Proc.gdi_objects",
        Col::IoReads => "Proc.io_reads",
        Col::IoWrites => "Proc.io_writes",
        Col::IoOther => "Proc.io_other",
        Col::IoReadBytes => "Proc.io_read_bytes",
        Col::IoWriteBytes => "Proc.io_write_bytes",
        Col::IoOtherBytes => "Proc.io_other_bytes",
        Col::Path => "ImageInfo.path",
        Col::CmdLine => "ProcStatic.cmdline",
        Col::Elevated => "ProcStatic.elevated",
        Col::Package => "ProcStatic.package",
        Col::Platform => "ProcStatic.platform",
        Col::UacVirt => "ProcStatic.protect.uac_virt",
        Col::Dep => "ProcStatic.protect.dep",
        Col::Dpi => "ProcStatic.protect.dpi",
        Col::EcoQos => "Proc.eco_qos",
        Col::Cfg => "ProcStatic.protect.cfg",
        Col::ShadowStack => "ProcStatic.protect.shadow",
        Col::Kind => "Proc.has_window + ProcStatic.company",
        Col::Publisher => "ImageInfo.company",
        Col::DiskRate => "Proc.disk_bps",
        Col::GpuUsage => "Proc.gpu_util",
        Col::GpuEngine => "Proc.gpu_engine",
        Col::GpuDedicated => "Proc.gpu_dedicated",
        Col::GpuShared => "Proc.gpu_shared",
        Col::Network => "Proc.net_bps",
        Col::MemActivePct => "Proc.active_private / mem_total",
        Col::MemPrivatePct => "Proc.private_ws / mem_total",
        Col::DiskPct => "Proc.disk_bps / disk_full",
        Col::NetPct => "Proc.net_bps / net_full",
    }
}

pub fn def(col: Col) -> &'static ColDef {
    &ALL[col as usize]
}

pub fn by_key(key: &str) -> Option<&'static ColDef> {
    ALL.iter().find(|d| d.key == key)
}

pub fn defaults() -> Vec<(Col, u16)> {
    [
        Col::Name,
        Col::Pid,
        Col::Status,
        Col::User,
        Col::Cpu,
        Col::WsDelta,
        Col::ActivePrivate,
        Col::Private,
        Col::Arch,
        Col::Description,
    ]
    .iter()
    .map(|c| (*c, def(*c).width))
    .collect()
}

fn is_text(col: Col) -> bool {
    matches!(
        col,
        Col::Name | Col::Status | Col::User | Col::Arch | Col::Description | Col::Path
            | Col::CmdLine | Col::Elevated | Col::Package | Col::Platform | Col::UacVirt | Col::Dep
            | Col::Dpi | Col::EcoQos | Col::Cfg | Col::ShadowStack | Col::Kind | Col::Publisher
            | Col::GpuEngine
    )
}

/// Whether the column needs the per-process GPU counters.
pub fn needs_proc_gpu(col: Col) -> bool {
    matches!(col, Col::GpuUsage | Col::GpuEngine | Col::GpuDedicated | Col::GpuShared)
}

/// Whether the column needs the per-process TCP rate.
pub fn needs_proc_net(col: Col) -> bool {
    matches!(col, Col::Network | Col::NetPct)
}

/// Whether the column needs the power throttling state read at every sample.
pub fn needs_eco_qos(col: Col) -> bool {
    col == Col::EcoQos
}

/// Text columns open ascending, numeric columns descending.
pub fn default_descending(col: Col) -> bool {
    !is_text(col) && col != Col::Pid
}

/// What the shown columns need the sampler to read.
pub fn demand(cols: &[(Col, u16)]) -> u32 {
    use crate::system::demand::need;
    cols.iter().fold(0, |n, (c, _)| {
        n | if needs_gui_counts(*c) { need::GUI_COUNTS } else { 0 }
            | if needs_proc_gpu(*c) { need::PROC_GPU } else { 0 }
            | if needs_proc_net(*c) { need::PROC_NET } else { 0 }
            | if needs_eco_qos(*c) { need::ECO_QOS } else { 0 }
    })
}

/// Whether the column's cost requires per-process handle work.
pub fn needs_gui_counts(col: Col) -> bool {
    matches!(col, Col::UserObjects | Col::GdiObjects)
}

fn number(col: Col, p: &Proc) -> i64 {
    match col {
        Col::Pid => p.pid as i64,
        Col::Cpu => (p.cpu * 1000.0) as i64,
        Col::WsDelta => p.ws_delta,
        Col::ActivePrivate => p.active_private as i64,
        Col::Private => p.private_ws as i64,
        Col::Session => p.session as i64,
        Col::CpuTime => p.cpu_time as i64,
        Col::Cycles => p.cycles as i64,
        Col::Ws => p.ws as i64,
        Col::PeakWs => p.peak_ws as i64,
        Col::SharedWs => p.ws.saturating_sub(p.private_ws) as i64,
        Col::Commit => p.commit as i64,
        Col::Paged => p.paged as i64,
        Col::NonPaged => p.nonpaged as i64,
        Col::PageFaults => p.page_faults as i64,
        Col::PfDelta => p.pf_delta as i64,
        Col::Priority => p.base_priority as i64,
        Col::Handles => p.handles as i64,
        Col::Threads => p.threads as i64,
        Col::UserObjects => p.user_objects as i64,
        Col::GdiObjects => p.gdi_objects as i64,
        Col::IoReads => p.io_reads as i64,
        Col::IoWrites => p.io_writes as i64,
        Col::IoOther => p.io_other as i64,
        Col::IoReadBytes => p.io_read_bytes as i64,
        Col::IoWriteBytes => p.io_write_bytes as i64,
        Col::IoOtherBytes => p.io_other_bytes as i64,
        Col::DiskRate => p.disk_bps as i64,
        Col::GpuUsage => (p.gpu_util * 1000.0) as i64,
        Col::GpuDedicated => p.gpu_dedicated as i64,
        Col::GpuShared => p.gpu_shared as i64,
        Col::Network | Col::NetPct => p.net_bps as i64,
        Col::MemActivePct => p.active_private as i64,
        Col::MemPrivatePct => p.private_ws as i64,
        Col::DiskPct => p.disk_bps as i64,
        _ => 0,
    }
}

/// "—" for a process whose value could not be read, blank for the system processes.
fn unknown(p: &Proc) -> &'static str {
    if p.pid > 4 {
        "—"
    } else {
        ""
    }
}

fn flag(value: Option<bool>, p: &Proc, yes: &'static str, no: &'static str) -> &'static str {
    match value {
        Some(true) => yes,
        Some(false) => no,
        None => unknown(p),
    }
}

fn kind_label(p: &Proc) -> &'static str {
    let d = &tr().details;
    if p.has_window {
        return d.kind_app;
    }
    if crate::system::sampler::is_kernel_process(p.pid, &p.stat.name) {
        return d.kind_windows;
    }
    match p.stat.company() {
        None => "",
        Some("Microsoft Corporation") if p.stat.system_dir => d.kind_windows,
        Some(_) => d.kind_background,
    }
}

fn text(col: Col, p: &Proc) -> &str {
    let (s, d) = (&tr().common, &tr().details);
    match col {
        Col::Name => &p.stat.name,
        Col::Status => p.status.label(),
        Col::User => &p.stat.user,
        Col::Arch => p.stat.arch,
        Col::Description => p.stat.description(),
        Col::Path => p.stat.path(),
        Col::CmdLine => &p.stat.cmdline,
        Col::Elevated => elevated_label(p),
        Col::Package => p.stat.package.as_deref().unwrap_or(""),
        Col::Platform => match p.stat.platform {
            32 => s.bit32,
            64 => s.bit64,
            _ => "",
        },
        Col::UacVirt => flag(p.stat.protect.uac_virt, p, s.yes, s.no),
        Col::Dep => flag(p.stat.protect.dep, p, s.enabled, s.disabled),
        Col::Dpi => match p.stat.protect.dpi {
            Some(0) => s.no,
            Some(1) => d.dpi_system,
            Some(_) => d.dpi_monitor,
            None => unknown(p),
        },
        Col::EcoQos => flag(p.eco_qos, p, s.enabled, s.disabled),
        Col::Cfg => flag(p.stat.protect.cfg, p, s.enabled, s.disabled),
        Col::ShadowStack => match p.stat.protect.shadow {
            Some(0) => s.disabled,
            Some(1) => d.compat_mode,
            Some(_) => d.strict_mode,
            None => unknown(p),
        },
        Col::Kind => kind_label(p),
        Col::Publisher => p.stat.company().unwrap_or(""),
        Col::GpuEngine => p.gpu_engine.as_str(),
        _ => "",
    }
}

fn elevated_label(p: &Proc) -> &'static str {
    match p.stat.elevated {
        Some(true) => tr().common.yes,
        Some(false) => tr().common.no,
        None if p.pid > 4 => "—",
        None => "",
    }
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

pub fn compare(col: Col, a: &Proc, b: &Proc) -> Ordering {
    if is_text(col) {
        cmp_text(text(col, a), text(col, b))
    } else {
        number(col, a).cmp(&number(col, b))
    }
}

fn grouped(out: &mut String, value: i64) {
    if value < 0 {
        out.push('-');
    }
    let mut digits = [0u8; 24];
    let mut n = value.unsigned_abs();
    let mut i = digits.len();
    let mut count = 0;
    loop {
        if count == 3 {
            i -= 1;
            digits[i] = b',';
            count = 0;
        }
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
        if n == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&digits[i..]).unwrap_or(""));
}

fn kilobytes(out: &mut String, bytes: i64) {
    grouped(out, bytes / 1024);
    out.push_str(" K");
}

fn priority_label(base: i32) -> &'static str {
    let d = &tr().details;
    match base {
        0..=4 => d.prio_low,
        5..=6 => d.prio_below,
        7..=8 => d.prio_normal,
        9..=10 => d.prio_above,
        11..=13 => d.prio_high,
        _ => d.prio_realtime,
    }
}

/// Writes `part` as a percentage of `whole`, or 0% when there is nothing to compare with.
fn percent(out: &mut String, part: f64, whole: f64) {
    let v = if whole > 0.0 { part / whole * 100.0 } else { 0.0 };
    let _ = write!(out, "{v:.1}%");
}

/// The value of a cell for a file: a plain number in base units (ratios from 0 to 1, bytes,
/// bytes per second, seconds), or the text the table shows.
pub fn export_value(col: Col, p: &Proc, scale: &Scale) -> String {
    let ratio = |part: f64, whole: f64| if whole > 0.0 { format!("{}", part / whole) } else { String::new() };
    match col {
        Col::Cpu => format!("{}", p.cpu as f64 / 100.0),
        Col::GpuUsage => format!("{}", p.gpu_util as f64 / 100.0),
        Col::CpuTime => format!("{}", p.cpu_time as f64 / 1e7),
        Col::MemActivePct => ratio(p.active_private as f64, scale.mem_total as f64),
        Col::MemPrivatePct => ratio(p.private_ws as f64, scale.mem_total as f64),
        Col::DiskPct => ratio(p.disk_bps as f64, scale.disk_full),
        Col::NetPct => ratio(p.net_bps as f64, scale.net_full),
        _ if is_text(col) => text(col, p).to_string(),
        Col::Priority => p.base_priority.to_string(),
        _ => number(col, p).to_string(),
    }
}

/// Appends the cell text to `out`.
pub fn cell(col: Col, p: &Proc, scale: &Scale, out: &mut String) {
    match col {
        Col::Cpu => {
            let _ = write!(out, "{:02}", p.cpu.round() as u32);
        }
        Col::CpuTime => {
            let seconds = p.cpu_time / 10_000_000;
            let _ = write!(
                out,
                "{}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            );
        }
        Col::Priority => out.push_str(if p.pid <= 4 { "" } else { priority_label(p.base_priority) }),
        Col::MemActivePct => percent(out, p.active_private as f64, scale.mem_total as f64),
        Col::MemPrivatePct => percent(out, p.private_ws as f64, scale.mem_total as f64),
        Col::DiskPct => percent(out, p.disk_bps as f64, scale.disk_full),
        Col::NetPct => {
            if crate::system::sysinfo::NET_MEASURED.load(std::sync::atomic::Ordering::Relaxed) {
                percent(out, p.net_bps as f64, scale.net_full);
            } else {
                out.push_str(unknown(p));
            }
        }
        Col::WsDelta | Col::ActivePrivate | Col::Private | Col::Ws | Col::PeakWs | Col::SharedWs
        | Col::Commit | Col::Paged | Col::NonPaged => kilobytes(out, number(col, p)),
        Col::Pid | Col::Session => {
            let _ = write!(out, "{}", number(col, p));
        }
        Col::DiskRate => {
            grouped(out, number(col, p) / 1024);
            out.push_str(" KB/s");
        }
        Col::GpuUsage => {
            let _ = write!(out, "{:.0}%", p.gpu_util);
        }
        Col::GpuDedicated | Col::GpuShared => kilobytes(out, number(col, p)),
        Col::Network => {
            if !crate::system::sysinfo::NET_MEASURED.load(std::sync::atomic::Ordering::Relaxed) {
                out.push_str(unknown(p));
            } else {
                grouped(out, number(col, p) / 1024);
                out.push_str(" KB/s");
            }
        }
        _ if is_text(col) => out.push_str(text(col, p)),
        _ => grouped(out, number(col, p)),
    }
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn percent_text() {
        let mut out = String::new();
        percent(&mut out, 50.0, 200.0);
        assert_eq!(out, "25.0%");
        out.clear();
        percent(&mut out, 1.0, 0.0);
        assert_eq!(out, "0.0%");
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn costly_columns_declare_their_needs() {
        use crate::system::demand::need;
        assert_eq!(demand(&defaults()), 0);
        let cols = [(Col::GpuUsage, 100), (Col::NetPct, 100), (Col::EcoQos, 90), (Col::GdiObjects, 80)];
        assert_eq!(demand(&cols), need::PROC_GPU | need::PROC_NET | need::ECO_QOS | need::GUI_COUNTS);
    }

    #[test]
    fn groups_cover_every_column_once() {
        let mut seen: Vec<Col> = display_order().collect();
        assert_eq!(seen.len(), ALL.len());
        seen.sort_by_key(|c| *c as u8);
        seen.dedup();
        assert_eq!(seen.len(), ALL.len());
    }
}
