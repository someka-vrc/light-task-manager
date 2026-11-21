//! Machine-wide counters for the performance page.
//!
//! Everything here avoids GPU APIs: disks and GPU engines come from
//! performance counters (PDH), adapter names from the kernel thunks in
//! gdi32 that the stock Task Manager also uses.

use crate::system::model::*;
use crate::system::win::{from_wide, wide};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Performance::*;
use windows_sys::Win32::System::ProcessStatus::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::GetActiveProcessorCount;

windows_targets::link!("ntdll.dll" "system" fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32);
windows_targets::link!("gdi32.dll" "system" fn D3DKMTOpenAdapterFromLuid(args: *mut OpenAdapterFromLuid) -> i32);
windows_targets::link!("gdi32.dll" "system" fn D3DKMTQueryAdapterInfo(args: *const QueryAdapterInfo) -> i32);
windows_targets::link!("gdi32.dll" "system" fn D3DKMTCloseAdapter(args: *const u32) -> i32);

const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;
const PDH_FMT_NOCAP100: u32 = 0x8000;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const KMTQAITYPE_GETSEGMENTSIZE: u32 = 3;
const KMTQAITYPE_ADAPTERREGISTRYINFO: u32 = 8;
const KMTQAITYPE_ADAPTERTYPE: u32 = 15;
/// `D3DKMT_ADAPTERTYPE.SoftwareDevice`: a renderer in software, or a virtual display driver.
const ADAPTERTYPE_SOFTWARE: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CoreTimes {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    interrupts: u32,
    _pad: u32,
}

#[repr(C)]
struct OpenAdapterFromLuid {
    luid_low: u32,
    luid_high: i32,
    adapter: u32,
}

#[repr(C)]
struct QueryAdapterInfo {
    adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

#[repr(C)]
#[derive(Default)]
struct SegmentSizeInfo {
    dedicated_video: u64,
    dedicated_system: u64,
    shared_system: u64,
}

// ---------------------------------------------------------------- PDH

struct Pdh {
    query: isize,
    counters: Vec<isize>,
}

impl Pdh {
    fn open() -> Option<Self> {
        let mut query = 0;
        (unsafe { PdhOpenQueryW(null(), 0, &mut query) } == 0).then_some(Self { query, counters: Vec::new() })
    }

    /// Returns the counter's slot; a counter that does not exist yields no values.
    fn add(&mut self, path: &str) -> usize {
        let mut h = 0;
        let p = wide(path);
        if unsafe { PdhAddEnglishCounterW(self.query, p.as_ptr(), 0, &mut h) } != 0 {
            h = 0;
        }
        self.counters.push(h);
        self.counters.len() - 1
    }

    fn collect(&self) {
        unsafe { PdhCollectQueryData(self.query) };
    }

    fn value(&self, slot: usize) -> Option<f64> {
        let h = *self.counters.get(slot)?;
        if h == 0 {
            return None;
        }
        let mut v: PDH_FMT_COUNTERVALUE = unsafe { std::mem::zeroed() };
        let ok = unsafe { PdhGetFormattedCounterValue(h, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, null_mut(), &mut v) };
        (ok == 0 && v.CStatus <= 1).then_some(unsafe { v.Anonymous.doubleValue })
    }

    /// Values of every instance of a wildcard counter.
    fn array(&self, slot: usize) -> Vec<(String, f64)> {
        let Some(&h) = self.counters.get(slot) else { return Vec::new() };
        if h == 0 {
            return Vec::new();
        }
        let fmt = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
        let (mut size, mut count) = (0u32, 0u32);
        let status = unsafe { PdhGetFormattedCounterArrayW(h, fmt, &mut size, &mut count, null_mut()) };
        if status != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u64; size as usize / 8 + 1];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if unsafe { PdhGetFormattedCounterArrayW(h, fmt, &mut size, &mut count, items) } != 0 {
            return Vec::new();
        }
        let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
        items
            .iter()
            .filter(|i| i.FmtValue.CStatus <= 1)
            .map(|i| {
                let name = unsafe {
                    let mut n = 0;
                    while *i.szName.add(n) != 0 {
                        n += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(i.szName, n))
                };
                (name, unsafe { i.FmtValue.Anonymous.doubleValue })
            })
            .collect()
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

struct MainCounters {
    pdh: Pdh,
    perf: usize,
    freq: usize,
    modified: usize,
    standby: [usize; 3],
    free: usize,
    disk_idle: usize,
    disk_read: usize,
    disk_write: usize,
    disk_latency: usize,
}

struct GpuCounters {
    pdh: Pdh,
    engine: usize,
    dedicated: usize,
    shared: usize,
}

struct ProcGpuCounters {
    pdh: Pdh,
    engine: usize,
    dedicated: usize,
    shared: usize,
}

// ---------------------------------------------------------------- network

/// Whether the last per-process network reading succeeded. Reading needs administrator rights.
pub static NET_MEASURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Identifies a TCP connection: family (2 = IPv4, 23 = IPv6), local address and port, remote address and port.
pub type ConnKey = (u8, [u8; 16], u32, [u8; 16], u32);

const AF_INET_FAMILY: u32 = 2;
const AF_INET6_FAMILY: u32 = 23;

/// The raw owner-PID table: a count followed by fixed-size rows.
fn tcp_owner_table(family: u32) -> Vec<u8> {
    let mut size = 0u32;
    for _ in 0..4 {
        let mut buf = vec![0u8; size as usize];
        let ptr: *mut c_void = if buf.is_empty() { null_mut() } else { buf.as_mut_ptr().cast() };
        let ret = unsafe { GetExtendedTcpTable(ptr, &mut size, 0, family, TCP_TABLE_OWNER_PID_ALL, 0) };
        if ret == 0 {
            buf.truncate(size as usize);
            return buf;
        }
        if ret != ERROR_INSUFFICIENT_BUFFER {
            break;
        }
    }
    Vec::new()
}

/// Reads the rows of an owner-PID table.
fn tcp_rows<T: Copy>(buf: &[u8]) -> Vec<T> {
    if buf.len() < 4 {
        return Vec::new();
    }
    let count = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let size = std::mem::size_of::<T>();
    (0..count)
        .filter_map(|i| {
            let at = 4 + i * size;
            (at + size <= buf.len()).then(|| unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast::<T>()) })
        })
        .collect()
}

fn v4_addr(addr: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&addr.to_ne_bytes());
    out
}

fn enable_v4(row: &MIB_TCPROW_LH) -> bool {
    let rw = TCP_ESTATS_DATA_RW_v0 { EnableCollection: 1 };
    let size = std::mem::size_of::<TCP_ESTATS_DATA_RW_v0>() as u32;
    unsafe { SetPerTcpConnectionEStats(row, TcpConnectionEstatsData, (&rw as *const TCP_ESTATS_DATA_RW_v0).cast(), 0, size, 0) == 0 }
}

fn enable_v6(row: &MIB_TCP6ROW) -> bool {
    let rw = TCP_ESTATS_DATA_RW_v0 { EnableCollection: 1 };
    let size = std::mem::size_of::<TCP_ESTATS_DATA_RW_v0>() as u32;
    unsafe { SetPerTcp6ConnectionEStats(row, TcpConnectionEstatsData, (&rw as *const TCP_ESTATS_DATA_RW_v0).cast(), 0, size, 0) == 0 }
}

/// Cumulative bytes in and out of a connection, once collection is enabled.
fn read_rod(rod: &mut TCP_ESTATS_DATA_ROD_v0, ok: bool) -> Option<u64> {
    ok.then(|| rod.DataBytesIn.wrapping_add(rod.DataBytesOut))
}

fn conn_total_v4(row: &MIB_TCPROW_LH) -> Option<u64> {
    let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32;
    let ret = unsafe {
        GetPerTcpConnectionEStats(row, TcpConnectionEstatsData, null_mut(), 0, 0, null_mut(), 0, 0, (&mut rod as *mut TCP_ESTATS_DATA_ROD_v0).cast(), 0, size)
    };
    read_rod(&mut rod, ret == 0)
}

fn conn_total_v6(row: &MIB_TCP6ROW) -> Option<u64> {
    let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32;
    let ret = unsafe {
        GetPerTcp6ConnectionEStats(row, TcpConnectionEstatsData, null_mut(), 0, 0, null_mut(), 0, 0, (&mut rod as *mut TCP_ESTATS_DATA_ROD_v0).cast(), 0, size)
    };
    read_rod(&mut rod, ret == 0)
}

/// One process's GPU use: the busiest engine, and memory summed over adapters.
#[derive(Default, Clone)]
pub struct ProcGpu {
    pub util: f32,
    /// The adapter and engine type of the busiest engine: its LUID, its type such as "3D", and
    /// a label like "GPU 0 - 3D" (the adapter numbered by LUID).
    pub luid: String,
    pub kind: String,
    pub label: String,
    pub dedicated: u64,
    pub shared: u64,
}

// ---------------------------------------------------------------- sampler

pub struct SysSampler {
    prev_cores: Vec<CoreTimes>,
    main: Option<MainCounters>,
    gpu: Option<GpuCounters>,
    proc_gpu: Option<ProcGpuCounters>,
    /// Last cumulative byte count of each TCP connection.
    conn_prev: HashMap<ConnKey, u64>,
    conn_enabled: HashSet<ConnKey>,
    conn_failed: HashSet<ConnKey>,
    prev_net: HashMap<u32, (u64, u64)>,
    addresses: HashMap<u32, (String, String)>,
    addresses_at: Option<Instant>,
    /// Drive letters per physical disk, by where each volume's data lies; see `letters_by_disk`.
    volumes: HashMap<u32, String>,
    /// The logical drives the letters were read for, and when.
    volumes_at: Option<(u32, Instant)>,
    disks: HashMap<u32, Arc<DiskStatic>>,
    gpu_info: HashMap<String, AdapterInfo>,
    info: Arc<SysStatic>,
    last: Option<Instant>,
}

impl SysSampler {
    pub fn new() -> Self {
        let main = Pdh::open().map(|mut pdh| MainCounters {
            perf: pdh.add("\\Processor Information(_Total)\\% Processor Performance"),
            freq: pdh.add("\\Processor Information(_Total)\\Processor Frequency"),
            modified: pdh.add("\\Memory\\Modified Page List Bytes"),
            standby: [
                pdh.add("\\Memory\\Standby Cache Core Bytes"),
                pdh.add("\\Memory\\Standby Cache Normal Priority Bytes"),
                pdh.add("\\Memory\\Standby Cache Reserve Bytes"),
            ],
            free: pdh.add("\\Memory\\Free & Zero Page List Bytes"),
            disk_idle: pdh.add("\\PhysicalDisk(*)\\% Idle Time"),
            disk_read: pdh.add("\\PhysicalDisk(*)\\Disk Read Bytes/sec"),
            disk_write: pdh.add("\\PhysicalDisk(*)\\Disk Write Bytes/sec"),
            disk_latency: pdh.add("\\PhysicalDisk(*)\\Avg. Disk sec/Transfer"),
            pdh,
        });
        if let Some(m) = &main {
            // Rate counters need a first reading to diff against.
            m.pdh.collect();
        }
        Self {
            prev_cores: Vec::new(),
            main,
            gpu: None,
            proc_gpu: None,
            conn_prev: HashMap::new(),
            conn_enabled: HashSet::new(),
            conn_failed: HashSet::new(),
            prev_net: HashMap::new(),
            addresses: HashMap::new(),
            addresses_at: None,
            volumes: HashMap::new(),
            volumes_at: None,
            disks: HashMap::new(),
            gpu_info: HashMap::new(),
            info: Arc::new(static_info()),
            last: None,
        }
    }

    /// TCP bytes per second of each process, keyed by PID. Needs administrator rights:
    /// without them the per-connection collection cannot be enabled and the map stays empty.
    pub fn proc_net(&mut self, want: bool, seconds: f64) -> HashMap<u32, u64> {
        use std::sync::atomic::Ordering;
        if !want {
            self.conn_prev.clear();
            self.conn_enabled.clear();
            self.conn_failed.clear();
            NET_MEASURED.store(false, Ordering::Relaxed);
            return HashMap::new();
        }
        let mut by_pid: HashMap<u32, u64> = HashMap::new();
        let mut seen: HashSet<ConnKey> = HashSet::new();
        let mut measured = false;

        for r in tcp_rows::<MIB_TCPROW_OWNER_PID>(&tcp_owner_table(AF_INET_FAMILY)) {
            let key: ConnKey = (
                4,
                v4_addr(r.dwLocalAddr),
                r.dwLocalPort,
                v4_addr(r.dwRemoteAddr),
                r.dwRemotePort,
            );
            seen.insert(key);
            let mut row: MIB_TCPROW_LH = unsafe { std::mem::zeroed() };
            row.Anonymous.dwState = r.dwState;
            row.dwLocalAddr = r.dwLocalAddr;
            row.dwLocalPort = r.dwLocalPort;
            row.dwRemoteAddr = r.dwRemoteAddr;
            row.dwRemotePort = r.dwRemotePort;
            if !self.conn_enabled.contains(&key) && !self.conn_failed.contains(&key) {
                if enable_v4(&row) {
                    self.conn_enabled.insert(key);
                } else {
                    self.conn_failed.insert(key);
                }
            }
            if self.conn_enabled.contains(&key) {
                let total = conn_total_v4(&row);
                self.note(key, r.dwOwningPid, total, &mut by_pid, &mut measured);
            }
        }

        for r in tcp_rows::<MIB_TCP6ROW_OWNER_PID>(&tcp_owner_table(AF_INET6_FAMILY)) {
            let key: ConnKey = (6, r.ucLocalAddr, r.dwLocalPort, r.ucRemoteAddr, r.dwRemotePort);
            seen.insert(key);
            let mut row: MIB_TCP6ROW = unsafe { std::mem::zeroed() };
            row.State = r.dwState as MIB_TCP_STATE;
            unsafe {
                std::ptr::copy_nonoverlapping(r.ucLocalAddr.as_ptr(), &mut row.LocalAddr as *mut _ as *mut u8, 16);
                std::ptr::copy_nonoverlapping(r.ucRemoteAddr.as_ptr(), &mut row.RemoteAddr as *mut _ as *mut u8, 16);
            }
            row.dwLocalScopeId = r.dwLocalScopeId;
            row.dwLocalPort = r.dwLocalPort;
            row.dwRemoteScopeId = r.dwRemoteScopeId;
            row.dwRemotePort = r.dwRemotePort;
            if !self.conn_enabled.contains(&key) && !self.conn_failed.contains(&key) {
                if enable_v6(&row) {
                    self.conn_enabled.insert(key);
                } else {
                    self.conn_failed.insert(key);
                }
            }
            if self.conn_enabled.contains(&key) {
                let total = conn_total_v6(&row);
                self.note(key, r.dwOwningPid, total, &mut by_pid, &mut measured);
            }
        }

        self.conn_prev.retain(|k, _| seen.contains(k));
        self.conn_enabled.retain(|k| seen.contains(k));
        self.conn_failed.retain(|k| seen.contains(k));
        NET_MEASURED.store(measured, Ordering::Relaxed);
        by_pid
            .into_iter()
            .map(|(pid, bytes)| (pid, (bytes as f64 / seconds.max(0.001)) as u64))
            .collect()
    }

    fn note(&mut self, key: ConnKey, pid: u32, total: Option<u64>, by_pid: &mut HashMap<u32, u64>, measured: &mut bool) {
        let Some(total) = total else { return };
        *measured = true;
        if let Some(prev) = self.conn_prev.get(&key) {
            *by_pid.entry(pid).or_insert(0) += total.saturating_sub(*prev);
        }
        self.conn_prev.insert(key, total);
    }

    /// GPU use of each process, keyed by PID. Empty unless `want` is set.
    pub fn proc_gpu(&mut self, want: bool) -> HashMap<u32, ProcGpu> {
        if !want {
            self.proc_gpu = None;
            return HashMap::new();
        }
        if self.proc_gpu.is_none() {
            self.proc_gpu = Pdh::open().map(|mut pdh| ProcGpuCounters {
                engine: pdh.add("\\GPU Engine(*)\\Utilization Percentage"),
                dedicated: pdh.add("\\GPU Process Memory(*)\\Dedicated Usage"),
                shared: pdh.add("\\GPU Process Memory(*)\\Shared Usage"),
                pdh,
            });
            // The first reading only primes the rate counters, so this sample has no values.
            if let Some(g) = &self.proc_gpu {
                g.pdh.collect();
            }
            return HashMap::new();
        }
        let Some(g) = &self.proc_gpu else { return HashMap::new() };
        g.pdh.collect();

        // Instance names look like "pid_1234_luid_..._eng_0_engtype_3D".
        let pid_of = |name: &str| -> Option<u32> { name.strip_prefix("pid_")?.split('_').next()?.parse().ok() };
        let mut by_pid: HashMap<u32, ProcGpu> = HashMap::new();
        // Per process, adapter and engine type, summed over the engines of that type.
        let mut engines: HashMap<(u32, String, String), f64> = HashMap::new();
        for (name, v) in g.pdh.array(g.engine) {
            let Some(pid) = pid_of(&name) else { continue };
            let Some(luid) = luid_of(&name) else { continue };
            let kind = name.split("engtype_").nth(1).filter(|k| !k.is_empty()).unwrap_or(crate::i18n::tr().common.other).to_string();
            // Software adapters are not GPUs and get no number, as on the performance page.
            if self.gpu_info.entry(luid.to_string()).or_insert_with(|| adapter_info(luid)).software {
                continue;
            }
            register_gpu(luid);
            *engines.entry((pid, luid.to_string(), kind)).or_insert(0.0) += v;
        }
        for ((pid, luid, kind), v) in engines {
            let entry = by_pid.entry(pid).or_default();
            let v = v.clamp(0.0, 100.0) as f32;
            if v > entry.util {
                entry.util = v;
                entry.kind = kind;
                entry.luid = luid;
            }
        }
        for entry in by_pid.values_mut() {
            if !entry.luid.is_empty() {
                entry.label = format!("GPU {} - {}", gpu_number(&entry.luid), entry.kind);
            }
        }
        for (name, v) in g.pdh.array(g.dedicated) {
            if let Some(pid) = pid_of(&name) {
                by_pid.entry(pid).or_default().dedicated += v as u64;
            }
        }
        for (name, v) in g.pdh.array(g.shared) {
            if let Some(pid) = pid_of(&name) {
                by_pid.entry(pid).or_default().shared += v as u64;
            }
        }
        by_pid
    }

    pub fn sample(&mut self, totals: &ProcTotals, want_gpu: bool) -> SysSample {
        let now = Instant::now();
        let elapsed = self.last.map_or(0.0, |t| (now - t).as_secs_f32());
        self.last = Some(now);
        if let Some(m) = &self.main {
            m.pdh.collect();
        }
        // The first reading of freshly opened counters only primes them (and can read 100%),
        // so a sample that opens them reports no GPU values.
        let mut primed = true;
        if want_gpu && self.gpu.is_none() {
            primed = false;
            self.gpu = Pdh::open().map(|mut pdh| {
                let g = GpuCounters {
                    engine: pdh.add("\\GPU Engine(*)\\Utilization Percentage"),
                    dedicated: pdh.add("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                    shared: pdh.add("\\GPU Adapter Memory(*)\\Shared Usage"),
                    pdh,
                };
                g.pdh.collect();
                g
            });
        } else if !want_gpu {
            // These counters are expensive to collect, so only while something needs them.
            self.gpu = None;
        } else if let Some(g) = &self.gpu {
            g.pdh.collect();
        }

        SysSample {
            elapsed,
            cpu: self.cpu(totals),
            mem: self.mem(totals),
            disks: self.disks(),
            nets: self.nets(elapsed),
            gpus: if want_gpu && primed { self.gpus() } else { Vec::new() },
            info: self.info.clone(),
        }
    }

    fn cpu(&mut self, totals: &ProcTotals) -> CpuSample {
        let n = (self.info.logical.max(1)) as usize;
        let mut cur = vec![CoreTimes::default(); n];
        let size = (n * std::mem::size_of::<CoreTimes>()) as u32;
        let mut ret = 0;
        let status = unsafe {
            NtQuerySystemInformation(SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION, cur.as_mut_ptr().cast(), size, &mut ret)
        };
        if status >= 0 {
            cur.truncate(ret as usize / std::mem::size_of::<CoreTimes>());
        } else {
            cur.clear();
        }
        let mut s = CpuSample::default();
        if self.prev_cores.len() == cur.len() {
            let (mut busy_sum, mut kern_sum, mut all_sum) = (0i64, 0i64, 0i64);
            for (a, b) in cur.iter().zip(&self.prev_cores) {
                let idle = a.idle - b.idle;
                let kernel = a.kernel - b.kernel;
                let user = a.user - b.user;
                let all = (kernel + user).max(1);
                let busy = (all - idle).max(0);
                s.cores.push((busy as f32 / all as f32 * 100.0).clamp(0.0, 100.0));
                busy_sum += busy;
                kern_sum += (kernel - idle).max(0);
                all_sum += all;
            }
            s.total = (busy_sum as f32 / all_sum.max(1) as f32 * 100.0).clamp(0.0, 100.0);
            s.kernel = (kern_sum as f32 / all_sum.max(1) as f32 * 100.0).clamp(0.0, 100.0);
        } else {
            s.cores = vec![0.0; cur.len()];
        }
        self.prev_cores = cur;

        if let Some(m) = &self.main {
            if let (Some(perf), Some(freq)) = (m.pdh.value(m.perf), m.pdh.value(m.freq)) {
                s.mhz = (perf * freq / 100.0) as f32;
            }
        }
        s.processes = totals.processes;
        s.threads = totals.threads;
        s.handles = totals.handles;
        s.uptime_ms = unsafe { GetTickCount64() };
        s
    }

    fn mem(&mut self, totals: &ProcTotals) -> MemSample {
        let mut s = MemSample::default();
        let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        ms.dwLength = std::mem::size_of_val(&ms) as u32;
        unsafe { GlobalMemoryStatusEx(&mut ms) };
        s.total = ms.ullTotalPhys;
        s.available = ms.ullAvailPhys;

        let mut pi: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
        pi.cb = std::mem::size_of_val(&pi) as u32;
        if unsafe { K32GetPerformanceInfo(&mut pi, pi.cb) } != 0 {
            let page = pi.PageSize as u64;
            s.commit = pi.CommitTotal as u64 * page;
            s.commit_limit = pi.CommitLimit as u64 * page;
            s.cached = pi.SystemCache as u64 * page;
            s.paged_pool = pi.KernelPaged as u64 * page;
            s.nonpaged_pool = pi.KernelNonpaged as u64 * page;
        }
        if let Some(m) = &self.main {
            let v = |slot| m.pdh.value(slot).unwrap_or(0.0) as u64;
            s.modified = v(m.modified);
            s.standby = m.standby.iter().map(|slot| v(*slot)).sum();
            s.free = v(m.free);
            if s.standby + s.modified > 0 {
                s.cached = s.standby + s.modified;
            }
        }
        s.compressed = totals.compressed;
        s
    }

    fn disks(&mut self) -> Vec<DiskSample> {
        let Some(m) = &self.main else { return Vec::new() };
        let idle = m.pdh.array(m.disk_idle);
        let read: HashMap<String, f64> = m.pdh.array(m.disk_read).into_iter().collect();
        let write: HashMap<String, f64> = m.pdh.array(m.disk_write).into_iter().collect();
        let latency: HashMap<String, f64> = m.pdh.array(m.disk_latency).into_iter().collect();
        // Re-read the letters when a drive comes or goes, and now and then anyway.
        let drives = unsafe { GetLogicalDrives() };
        let stale = self.volumes_at.is_none_or(|(mask, at)| mask != drives || at.elapsed().as_secs() >= 30);
        if stale {
            self.volumes = letters_by_disk(drives);
            self.volumes_at = Some((drives, Instant::now()));
        }
        let mut out = Vec::new();
        for (name, idle) in idle {
            if name == "_Total" {
                continue;
            }
            let mut parts = name.split_whitespace();
            let Some(index) = parts.next().and_then(|n| n.parse::<u32>().ok()) else { continue };
            // The counter names only the partitions of the disk itself.
            let partitions = parts.collect::<Vec<_>>().join(" ");
            let info = self.disks.entry(index).or_insert_with(|| Arc::new(disk_static(index, &partitions))).clone();
            let letters = self.volumes.get(&index).cloned().unwrap_or_else(|| partitions.to_uppercase());
            out.push(DiskSample {
                index,
                letters,
                active: (100.0 - idle as f32).clamp(0.0, 100.0),
                read_bps: read.get(&name).copied().unwrap_or(0.0),
                write_bps: write.get(&name).copied().unwrap_or(0.0),
                response_ms: latency.get(&name).copied().unwrap_or(0.0) * 1000.0,
                info,
            });
        }
        out.sort_by_key(|d| d.index);
        out
    }

    fn nets(&mut self, elapsed: f32) -> Vec<NetSample> {
        let stale = self.addresses_at.is_none_or(|t| t.elapsed().as_secs() >= 30);
        if stale {
            self.addresses = adapter_addresses();
            self.addresses_at = Some(Instant::now());
        }
        let mut out = Vec::new();
        let mut table: *mut MIB_IF_TABLE2 = null_mut();
        if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
            return out;
        }
        let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
        let mut seen = HashMap::new();
        for r in rows {
            let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            let filter = r.InterfaceAndOperStatusFlags._bitfield & 2 != 0;
            let kind = match r.Type {
                6 => crate::i18n::tr().perf.ethernet,
                71 => "Wi-Fi",
                _ => continue,
            };
            if !hardware || filter || r.OperStatus != 1 {
                continue;
            }
            let (rx, tx) = (r.InOctets, r.OutOctets);
            let (recv_bps, send_bps) = match self.prev_net.get(&r.InterfaceIndex) {
                Some((prx, ptx)) if elapsed > 0.0 => (
                    rx.saturating_sub(*prx) as f64 / elapsed as f64,
                    tx.saturating_sub(*ptx) as f64 / elapsed as f64,
                ),
                _ => (0.0, 0.0),
            };
            seen.insert(r.InterfaceIndex, (rx, tx));
            let (ipv4, ipv6) = self.addresses.get(&r.InterfaceIndex).cloned().unwrap_or_default();
            out.push(NetSample {
                index: r.InterfaceIndex,
                kind,
                alias: from_wide(&r.Alias),
                description: from_wide(&r.Description),
                send_bps,
                recv_bps,
                link_bps: r.ReceiveLinkSpeed.max(r.TransmitLinkSpeed),
                ipv4,
                ipv6,
            });
        }
        unsafe { FreeMibTable(table as *const c_void) };
        self.prev_net = seen;
        out.sort_by_key(|n| n.index);
        out
    }

    fn gpus(&mut self) -> Vec<GpuSample> {
        let Some(g) = &self.gpu else { return Vec::new() };
        // Sum each engine across processes, then report the busiest engine of each type.
        let mut engines: HashMap<(String, String), (String, f64)> = HashMap::new();
        for (name, v) in g.pdh.array(g.engine) {
            let Some(luid) = luid_of(&name) else { continue };
            let eng = between(&name, "_eng_", "_").unwrap_or("0").to_string();
            let kind = name.split("engtype_").nth(1).unwrap_or("").to_string();
            let e = engines.entry((luid.to_string(), eng)).or_insert((kind, 0.0));
            e.1 += v;
        }
        let mut by_luid: HashMap<String, GpuSample> = HashMap::new();
        for ((luid, _), (kind, v)) in engines {
            let gs = by_luid.entry(luid.clone()).or_insert_with(|| GpuSample { luid, ..Default::default() });
            let v = v.clamp(0.0, 100.0) as f32;
            let label = if kind.is_empty() { crate::i18n::tr().common.other.to_string() } else { kind };
            match gs.engines.iter_mut().find(|(k, _)| *k == label) {
                Some(e) => e.1 = e.1.max(v),
                None => gs.engines.push((label, v)),
            }
        }
        for (name, v) in g.pdh.array(g.dedicated) {
            if let Some(gs) = luid_of(&name).and_then(|l| by_luid.get_mut(l)) {
                gs.dedicated += v as u64;
            }
        }
        for (name, v) in g.pdh.array(g.shared) {
            if let Some(gs) = luid_of(&name).and_then(|l| by_luid.get_mut(l)) {
                gs.shared += v as u64;
            }
        }
        let mut out: Vec<GpuSample> = by_luid.into_values().collect();
        for gs in &mut out {
            gs.engines.sort_by(|a, b| engine_rank(&a.0).cmp(&engine_rank(&b.0)).then(a.0.cmp(&b.0)));
            gs.util = gs.engines.iter().map(|e| e.1).fold(0.0, f32::max);
            let info = self.gpu_info.entry(gs.luid.clone()).or_insert_with(|| adapter_info(&gs.luid)).clone();
            gs.name = info.name;
            gs.dedicated_total = info.dedicated;
            gs.shared_total = info.shared;
            if info.software {
                gs.engines.clear();
            }
        }
        // Software adapters (the basic render driver, virtual display drivers) are not GPUs,
        // and Task Manager leaves them out too; so do adapters without engine counters.
        out.retain(|g| !g.engines.is_empty());
        for g in &out {
            register_gpu(&g.luid);
        }
        out.sort_by(|a, b| a.luid.cmp(&b.luid));
        out
    }
}

/// The LUIDs of the GPUs (not software adapters) seen so far in this run.
static GPU_LUIDS: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Notes a GPU. Only real adapters are passed in: those that `gpus` keeps.
fn register_gpu(luid: &str) {
    let mut set = GPU_LUIDS.lock().unwrap_or_else(|e| e.into_inner());
    if !set.contains(luid) {
        set.insert(luid.to_string());
    }
}

/// The number of a GPU in "GPU 0", "GPU 1"... everywhere in the app: its rank by LUID among the
/// GPUs seen in this run. An adapter that goes away keeps its number, so past samples still match.
pub fn gpu_number(luid: &str) -> usize {
    register_gpu(luid);
    let set = GPU_LUIDS.lock().unwrap_or_else(|e| e.into_inner());
    set.iter().position(|l| l == luid).unwrap_or(0)
}

fn engine_rank(kind: &str) -> u8 {
    match kind {
        "3D" => 0,
        "Copy" => 1,
        "VideoEncode" => 2,
        "VideoDecode" => 3,
        "Compute_0" | "Compute" => 4,
        _ => 9,
    }
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let rest = &s[i..];
    Some(rest.find(end).map_or(rest, |j| &rest[..j]))
}

/// "pid_1_luid_0x00000000_0x0000D1B1_phys_0_eng_0_engtype_3D" -> "0x00000000_0x0000D1B1".
fn luid_of(name: &str) -> Option<&str> {
    between(name, "luid_", "_phys")
}

#[derive(Clone, Default)]
struct AdapterInfo {
    name: String,
    dedicated: u64,
    shared: u64,
    software: bool,
}

fn adapter_info(luid: &str) -> AdapterInfo {
    let unknown = || AdapterInfo { name: "GPU".into(), ..Default::default() };
    let mut parts = luid.split('_');
    let parse = |s: Option<&str>| s.and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let (Some(high), Some(low)) = (parse(parts.next()), parse(parts.next())) else {
        return unknown();
    };
    let mut open = OpenAdapterFromLuid { luid_low: low, luid_high: high as i32, adapter: 0 };
    if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) } < 0 {
        return unknown();
    }
    let mut reg = vec![0u16; 260 * 4];
    let q = QueryAdapterInfo {
        adapter: open.adapter,
        kind: KMTQAITYPE_ADAPTERREGISTRYINFO,
        data: reg.as_mut_ptr().cast(),
        size: (reg.len() * 2) as u32,
    };
    let name = if unsafe { D3DKMTQueryAdapterInfo(&q) } >= 0 { from_wide(&reg[..260]) } else { String::new() };
    let mut seg = SegmentSizeInfo::default();
    let q = QueryAdapterInfo {
        adapter: open.adapter,
        kind: KMTQAITYPE_GETSEGMENTSIZE,
        data: (&mut seg as *mut SegmentSizeInfo).cast(),
        size: std::mem::size_of::<SegmentSizeInfo>() as u32,
    };
    let mut kind = 0u32;
    let type_query = QueryAdapterInfo {
        adapter: open.adapter,
        kind: KMTQAITYPE_ADAPTERTYPE,
        data: (&mut kind as *mut u32).cast(),
        size: 4,
    };
    unsafe {
        D3DKMTQueryAdapterInfo(&q);
        if D3DKMTQueryAdapterInfo(&type_query) < 0 {
            kind = 0;
        }
        D3DKMTCloseAdapter(&open.adapter);
    }
    let name = if name.trim().is_empty() { "GPU".to_string() } else { name.trim().to_string() };
    AdapterInfo { name, dedicated: seg.dedicated_video, shared: seg.shared_system, software: kind & ADAPTERTYPE_SOFTWARE != 0 }
}

// ---------------------------------------------------------------- disks

#[repr(C)]
struct PropertyQuery {
    property: u32,
    query: u32,
    extra: [u8; 4],
}

fn storage_query(h: HANDLE, property: u32, out: &mut [u8]) -> bool {
    let q = PropertyQuery { property, query: 0, extra: [0; 4] };
    let mut ret = 0;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&q as *const PropertyQuery).cast(),
            std::mem::size_of::<PropertyQuery>() as u32,
            out.as_mut_ptr().cast(),
            out.len() as u32,
            &mut ret,
            null_mut(),
        ) != 0
    }
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn ascii_at(b: &[u8], at: usize) -> String {
    if at == 0 || at >= b.len() {
        return String::new();
    }
    let end = b[at..].iter().position(|c| *c == 0).map_or(b.len(), |e| at + e);
    String::from_utf8_lossy(&b[at..end]).trim().to_string()
}

/// IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS
const IOCTL_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

/// Drive letters per physical disk, from where each volume's data lies, as Task Manager
/// lists them: a virtual drive (a cloud drive, a mounted image) goes with the disk that
/// holds its data. Network drives are left out. Letters come last to first, like "H: G: F:".
fn letters_by_disk(drives: u32) -> HashMap<u32, String> {
    let mut by_disk: HashMap<u32, Vec<char>> = HashMap::new();
    for i in 0..26u8 {
        if drives & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i) as char;
        // DRIVE_REMOTE
        if unsafe { GetDriveTypeW(wide(&format!("{letter}:\\")).as_ptr()) } == 4 {
            continue;
        }
        let path = wide(&format!(r"\\.\{letter}:"));
        let h = unsafe { CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            continue;
        }
        // VOLUME_DISK_EXTENTS: a count, then 24-byte extents that start with the disk number.
        let mut buf = [0u8; 8 + 24 * 16];
        let mut ret = 0;
        let ok = unsafe {
            DeviceIoControl(h, IOCTL_VOLUME_DISK_EXTENTS, null(), 0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut ret, null_mut())
        };
        unsafe { CloseHandle(h) };
        if ok == 0 {
            continue;
        }
        let count = (read_u32(&buf, 0) as usize).min(16);
        for e in 0..count {
            let disk = read_u32(&buf, 8 + e * 24);
            let list = by_disk.entry(disk).or_default();
            if !list.contains(&letter) {
                list.push(letter);
            }
        }
    }
    by_disk
        .into_iter()
        .map(|(disk, mut letters)| {
            letters.sort_unstable_by(|a, b| b.cmp(a));
            (disk, letters.iter().map(|l| format!("{l}:")).collect::<Vec<_>>().join(" "))
        })
        .collect()
}

fn disk_static(index: u32, letters: &str) -> DiskStatic {
    let mut d = DiskStatic::default();
    let path = wide(&format!("\\\\.\\PhysicalDrive{index}"));
    let h = unsafe {
        CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut())
    };
    let mut bus = 0;
    let mut seek_penalty = None;
    if h != INVALID_HANDLE_VALUE {
        let mut buf = vec![0u8; 1024];
        if storage_query(h, 0, &mut buf) {
            // STORAGE_DEVICE_DESCRIPTOR
            d.removable = buf[10] != 0;
            let vendor = ascii_at(&buf, read_u32(&buf, 12) as usize);
            let product = ascii_at(&buf, read_u32(&buf, 16) as usize);
            d.model = if vendor.is_empty() || product.starts_with(&vendor) { product } else { format!("{vendor} {product}") };
            bus = read_u32(&buf, 28);
        }
        let mut sp = [0u8; 12];
        if storage_query(h, 7, &mut sp) {
            seek_penalty = Some(sp[8] != 0);
        }
        let mut geo = [0u8; 256];
        let mut ret = 0;
        let ok = unsafe {
            DeviceIoControl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, null(), 0, geo.as_mut_ptr().cast(), geo.len() as u32, &mut ret, null_mut())
        };
        if ok != 0 {
            d.capacity = u64::from_le_bytes(geo[24..32].try_into().unwrap());
        }
        unsafe { CloseHandle(h) };
    }
    if d.capacity == 0 {
        // Fall back to the sum of the volumes.
        for l in letters.split_whitespace() {
            let root = wide(&format!("{l}\\"));
            let mut total = 0u64;
            if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), null_mut(), &mut total, null_mut()) } != 0 {
                d.capacity += total;
            }
        }
    }
    let bus_name = match bus {
        1 => "SCSI",
        3 => "ATA",
        7 => "USB",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        14 | 15 => crate::i18n::tr().perf.virtual_disk,
        16 => crate::i18n::tr().perf.storage_spaces,
        17 => "NVMe",
        _ => "",
    };
    d.kind = if d.removable || bus == 7 {
        crate::i18n::tr().perf.removable.to_string()
    } else {
        let media = match seek_penalty {
            Some(true) => "HDD",
            Some(false) => "SSD",
            None => crate::i18n::tr().common.disk,
        };
        if bus_name.is_empty() { media.to_string() } else { format!("{media} ({bus_name})") }
    };
    d
}

// ---------------------------------------------------------------- network

fn adapter_addresses() -> HashMap<u32, (String, String)> {
    let mut out = HashMap::new();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        buf = vec![0u64; size as usize / 8 + 1];
        let r = unsafe { GetAdaptersAddresses(0, flags, null(), buf.as_mut_ptr().cast(), &mut size) };
        if r == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if r != 0 {
            return out;
        }
        break;
    }
    let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !a.is_null() {
        let ad = unsafe { &*a };
        let index = unsafe { ad.Anonymous1.Anonymous.IfIndex };
        let (mut v4, mut v6) = (String::new(), String::new());
        let mut u = ad.FirstUnicastAddress;
        while !u.is_null() {
            let ua = unsafe { &*u };
            let sa = ua.Address.lpSockaddr as *const u8;
            if !sa.is_null() {
                let family = unsafe { *(sa as *const u16) };
                if family == 2 && v4.is_empty() {
                    let b = unsafe { std::slice::from_raw_parts(sa.add(4), 4) };
                    v4 = format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]);
                } else if family == 23 && v6.is_empty() {
                    let b = unsafe { std::slice::from_raw_parts(sa.add(8), 16) };
                    v6 = ipv6(b);
                }
            }
            u = ua.Next;
        }
        out.insert(index, (v4, v6));
        a = ad.Next;
    }
    out
}

fn ipv6(b: &[u8]) -> String {
    let g: Vec<u16> = (0..8).map(|i| u16::from_be_bytes([b[i * 2], b[i * 2 + 1]])).collect();
    // Compress the longest run of zero groups.
    let (mut best, mut best_len, mut i) = (usize::MAX, 0, 0);
    while i < 8 {
        if g[i] == 0 {
            let start = i;
            while i < 8 && g[i] == 0 {
                i += 1;
            }
            if i - start > best_len && i - start > 1 {
                best = start;
                best_len = i - start;
            }
        } else {
            i += 1;
        }
    }
    let hex = |s: &[u16]| s.iter().map(|v| format!("{v:x}")).collect::<Vec<_>>().join(":");
    if best == usize::MAX {
        hex(&g)
    } else {
        format!("{}::{}", hex(&g[..best]), hex(&g[best + best_len..]))
    }
}

// ---------------------------------------------------------------- static info

fn reg_string(key: HKEY, sub: &str, value: &str) -> Option<String> {
    let (sub, value) = (wide(sub), wide(value));
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(key, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    (r == 0).then(|| from_wide(&buf).trim().to_string())
}

fn reg_dword(key: HKEY, sub: &str, value: &str) -> Option<u32> {
    let (sub, value) = (wide(sub), wide(value));
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(key, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&mut v as *mut u32).cast(), &mut size)
    };
    (r == 0).then_some(v)
}

fn static_info() -> SysStatic {
    let mut s = SysStatic::default();
    let cpu_key = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";
    s.cpu_name = reg_string(HKEY_LOCAL_MACHINE, cpu_key, "ProcessorNameString").unwrap_or_default();
    s.base_mhz = reg_dword(HKEY_LOCAL_MACHINE, cpu_key, "~MHz").unwrap_or(0);
    s.logical = unsafe { GetActiveProcessorCount(0xFFFF) };

    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationAll, null_mut(), &mut len) };
    if len > 0 {
        let mut buf = vec![0u64; len as usize / 8 + 1];
        if unsafe { GetLogicalProcessorInformationEx(RelationAll, buf.as_mut_ptr().cast(), &mut len) } != 0 {
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len as usize) };
            let mut at = 0usize;
            while at + 8 <= bytes.len() {
                let rel = read_u32(bytes, at);
                let size = read_u32(bytes, at + 4) as usize;
                if size == 0 {
                    break;
                }
                match rel {
                    0 => s.cores += 1,
                    3 => s.sockets += 1,
                    2 => {
                        let level = bytes[at + 8];
                        let cache = read_u32(bytes, at + 12) as u64;
                        match level {
                            1 => s.l1 += cache,
                            2 => s.l2 += cache,
                            3 => s.l3 += cache,
                            _ => {}
                        }
                    }
                    _ => {}
                }
                at += size;
            }
        }
    }

    let mut kb = 0u64;
    if unsafe { GetPhysicallyInstalledSystemMemory(&mut kb) } != 0 {
        s.installed_mem = kb * 1024;
    }
    smbios_memory(&mut s);
    s
}

/// Reads the memory devices (type 17) from the SMBIOS tables.
fn smbios_memory(s: &mut SysStatic) {
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, null_mut(), 0) };
    if size == 0 {
        return;
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr(), size) } != size {
        return;
    }
    let table_len = read_u32(&buf, 4) as usize;
    let data = &buf[8..(8 + table_len).min(buf.len())];
    let mut at = 0;
    while at + 4 <= data.len() {
        let kind = data[at];
        let len = data[at + 1] as usize;
        if len < 4 || at + len > data.len() {
            break;
        }
        let f = &data[at..at + len];
        if kind == 17 && len >= 0x17 {
            s.slots_total += 1;
            let size = u16::from_le_bytes([f[0x0C], f[0x0D]]);
            if size != 0 && size != 0xFFFF {
                s.slots_used += 1;
                if s.form_factor.is_empty() {
                    s.form_factor = match f[0x0E] {
                        0x09 => "DIMM",
                        0x0D => "SODIMM",
                        0x0F => "FB-DIMM",
                        0x0B => "Row of chips",
                        _ => "",
                    }
                    .to_string();
                }
                if s.mem_type.is_empty() {
                    s.mem_type = match f[0x12] {
                        0x12 => "DDR",
                        0x13 => "DDR2",
                        0x18 => "DDR3",
                        0x1A => "DDR4",
                        0x1B => "LPDDR",
                        0x1C => "LPDDR2",
                        0x1D => "LPDDR3",
                        0x1E => "LPDDR4",
                        0x22 => "DDR5",
                        0x23 => "LPDDR5",
                        _ => "",
                    }
                    .to_string();
                }
                let speed = u16::from_le_bytes([f[0x15], f[0x16]]) as u32;
                let configured = if len >= 0x22 { u16::from_le_bytes([f[0x20], f[0x21]]) as u32 } else { 0 };
                let v = if configured != 0 && configured != 0xFFFF { configured } else { speed };
                if v != 0xFFFF {
                    s.mem_speed = s.mem_speed.max(v);
                }
            }
        }
        // Skip the formatted part, then the string set ending in two zeros.
        let mut p = at + len;
        while p + 1 < data.len() && !(data[p] == 0 && data[p + 1] == 0) {
            p += 1;
        }
        at = p + 2;
        if kind == 127 {
            break;
        }
    }
}
