//! Data shared between the sampling threads and the UI thread.

use std::sync::{Arc, OnceLock};

pub struct Icon {
    pub size: i32,
    /// Premultiplied BGRA, top-down.
    pub px: Vec<u32>,
}

/// Facts derived from an executable file, filled in lazily by the enricher.
pub struct ImageInfo {
    pub path: String,
    pub description: OnceLock<Arc<str>>,
    /// The "CompanyName" version string (publisher).
    pub company: OnceLock<Arc<str>>,
    pub icon: OnceLock<Option<Icon>>,
}

impl ImageInfo {
    pub fn description(&self) -> &str {
        self.description.get().map(|d| &**d).unwrap_or("")
    }
    /// `None` until the enricher has read the version resource.
    pub fn company(&self) -> Option<&str> {
        self.company.get().map(|d| &**d)
    }
    pub fn icon(&self) -> Option<&Icon> {
        self.icon.get().and_then(|i| i.as_ref())
    }
}

/// Per-process facts that never change, resolved once when a process appears.
pub struct ProcStatic {
    pub name: String,
    pub user: Arc<str>,
    pub arch: &'static str,
    pub elevated: Option<bool>,
    pub cmdline: String,
    pub image: Option<Arc<ImageInfo>>,
    /// Package full name of a packaged app.
    pub package: Option<Arc<str>>,
    /// "32 ビット" or "64 ビット".
    /// 32 or 64, or 0 when unknown.
    pub platform: u8,
    /// Whether the image lies in the Windows folder.
    pub system_dir: bool,
    pub protect: Protect,
}

/// Protection settings read from the process at discovery.
#[derive(Clone, Copy, Default)]
pub struct Protect {
    pub uac_virt: Option<bool>,
    pub dep: Option<bool>,
    /// 0 unaware, 1 system aware, 2 per-monitor aware.
    pub dpi: Option<u8>,
    pub cfg: Option<bool>,
    /// 0 off, 1 compatibility mode, 2 strict mode.
    pub shadow: Option<u8>,
}

impl ProcStatic {
    pub fn path(&self) -> &str {
        self.image.as_ref().map(|i| i.path.as_str()).unwrap_or("")
    }
    pub fn company(&self) -> Option<&str> {
        self.image.as_ref().and_then(|i| i.company())
    }
    pub fn description(&self) -> &str {
        self.image.as_ref().map(|i| i.description()).unwrap_or("")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Suspended,
    NotResponding,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Running => crate::i18n::tr().common.running,
            Status::Suspended => crate::i18n::tr().details.suspended,
            Status::NotResponding => crate::i18n::tr().common.not_responding,
        }
    }
}

#[derive(Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub create_time: i64,
    pub stat: Arc<ProcStatic>,
    pub status: Status,
    pub session: u32,
    pub has_window: bool,
    /// Percent of total machine capacity.
    pub cpu: f32,
    /// 100ns units.
    pub cpu_time: u64,
    pub cycles: u64,
    pub ws: u64,
    pub peak_ws: u64,
    pub ws_delta: i64,
    /// Disk bytes (read + write) per second since the previous sample.
    pub disk_bps: u64,
    /// Busiest GPU engine of the process, in percent.
    pub gpu_util: f32,
    /// Type of that engine, such as "3D".
    pub gpu_engine: String,
    pub gpu_dedicated: u64,
    pub gpu_shared: u64,
    /// Network bytes (sent + received) per second; TCP only, needs administrator rights.
    pub net_bps: u64,
    /// Whether power throttling is on; read each sample while its column is shown.
    pub eco_qos: Option<bool>,
    pub private_ws: u64,
    pub active_private: u64,
    pub commit: u64,
    pub paged: u64,
    pub nonpaged: u64,
    pub page_faults: u32,
    pub pf_delta: u32,
    pub base_priority: i32,
    pub handles: u32,
    pub threads: u32,
    pub user_objects: u32,
    pub gdi_objects: u32,
    pub io_reads: u64,
    pub io_writes: u64,
    pub io_other: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub io_other_bytes: u64,
}

impl Proc {
    pub fn key(&self) -> (u32, i64) {
        (self.pid, self.create_time)
    }
}

#[derive(Default)]
pub struct Snapshot {
    /// Empty when the sample is machine-wide only; see `has_procs`.
    pub procs: Vec<Proc>,
    /// Whether processes were sampled. Pages that use `procs` skip samples without them.
    pub has_procs: bool,
    pub cpu_total: f32,
    pub mem_total: u64,
    pub mem_used: u64,
    pub sys: SysSample,
}

/// Totals over all processes that the machine-wide figures need.
#[derive(Default, Clone, Copy)]
pub struct ProcTotals {
    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
    /// The working set of the "Memory Compression" process.
    pub compressed: u64,
}

/// Machine-wide counters for the performance page, taken with each snapshot.
#[derive(Default, Clone)]
pub struct SysSample {
    /// Seconds since the previous sample; 0 on the first one.
    pub elapsed: f32,
    pub cpu: CpuSample,
    pub mem: MemSample,
    pub disks: Vec<DiskSample>,
    pub nets: Vec<NetSample>,
    pub gpus: Vec<GpuSample>,
    pub info: Arc<SysStatic>,
}

#[derive(Default, Clone)]
pub struct CpuSample {
    /// Percent per logical processor.
    pub cores: Vec<f32>,
    pub total: f32,
    pub kernel: f32,
    /// Current clock in MHz, 0 when unknown.
    pub mhz: f32,
    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
    pub uptime_ms: u64,
}

#[derive(Default, Clone)]
pub struct MemSample {
    pub total: u64,
    pub available: u64,
    pub commit: u64,
    pub commit_limit: u64,
    pub cached: u64,
    pub paged_pool: u64,
    pub nonpaged_pool: u64,
    pub compressed: u64,
    pub modified: u64,
    pub standby: u64,
    pub free: u64,
}

#[derive(Default, Clone)]
pub struct DiskSample {
    /// The physical disk number.
    pub index: u32,
    /// Drive letters, e.g. "C: D:".
    pub letters: String,
    pub active: f32,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Average response time in milliseconds.
    pub response_ms: f64,
    pub info: Arc<DiskStatic>,
}

#[derive(Default)]
pub struct DiskStatic {
    pub model: String,
    /// "SSD (NVMe)", "HDD (SATA)" and so on.
    pub kind: String,
    pub capacity: u64,
    pub removable: bool,
}

#[derive(Default, Clone)]
pub struct NetSample {
    pub index: u32,
    /// "Wi-Fi" or "イーサネット".
    pub kind: &'static str,
    pub alias: String,
    pub description: String,
    pub send_bps: f64,
    pub recv_bps: f64,
    /// Link speed in bits per second.
    pub link_bps: u64,
    pub ipv4: String,
    pub ipv6: String,
}

#[derive(Default, Clone)]
pub struct GpuSample {
    /// The adapter LUID as written in performance counter instance names.
    pub luid: String,
    pub name: String,
    /// The busiest engine, in percent.
    pub util: f32,
    /// Engine type and percent, busiest first.
    pub engines: Vec<(String, f32)>,
    pub dedicated: u64,
    pub dedicated_total: u64,
    pub shared: u64,
    pub shared_total: u64,
}

/// Facts about the machine that do not change while the app runs.
#[derive(Default)]
pub struct SysStatic {
    pub cpu_name: String,
    pub base_mhz: u32,
    pub sockets: u32,
    pub cores: u32,
    pub logical: u32,
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    pub installed_mem: u64,
    pub mem_type: String,
    pub mem_speed: u32,
    pub slots_used: u32,
    pub slots_total: u32,
    pub form_factor: String,
}
