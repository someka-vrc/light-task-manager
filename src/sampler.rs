//! Background process sampling.
//!
//! One `NtQuerySystemInformation` call returns the counters of every process
//! without opening any of them, so a sample stays cheap. Handles are opened
//! only once per process, when it first appears.

use crate::model::*;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::RemoteDesktop::*;
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

windows_targets::link!("ntdll.dll" "system" fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32);
windows_targets::link!("ntdll.dll" "system" fn NtQueryInformationProcess(h: HANDLE, class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32);
windows_targets::link!("kernel32.dll" "system" fn IsWow64Process2(h: HANDLE, process: *mut u16, native: *mut u16) -> BOOL);
windows_targets::link!("user32.dll" "system" fn PrivateExtractIconsW(file: *const u16, index: i32, cx: i32, cy: i32, icons: *mut HICON, ids: *mut u32, count: u32, flags: u32) -> u32);
windows_targets::link!("kernel32.dll" "system" fn GetPackageFullName(h: HANDLE, len: *mut u32, name: *mut u16) -> i32);

const UNKNOWN: &str = "—";
const WTS_ANY_SESSION: u32 = 0xFFFFFFFE;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;
const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const SYSTEM_PROCESS_ID_INFORMATION: u32 = 88;
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

pub const WM_APP_SNAPSHOT: u32 = WM_APP + 1;
pub const WM_APP_ENRICHED: u32 = WM_APP + 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct NtString {
    len: u16,
    max: u16,
    buf: *mut u16,
}

#[repr(C)]
struct NtProcess {
    next: u32,
    threads: u32,
    private_ws: i64,
    hard_faults: u32,
    thread_peak: u32,
    cycles: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    name: NtString,
    base_priority: i32,
    pid: usize,
    ppid: usize,
    handles: u32,
    session: u32,
    key: usize,
    peak_vm: usize,
    vm: usize,
    page_faults: u32,
    peak_ws: usize,
    ws: usize,
    peak_paged: usize,
    paged: usize,
    peak_nonpaged: usize,
    nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private_pages: usize,
    reads: i64,
    writes: i64,
    others: i64,
    read_bytes: i64,
    write_bytes: i64,
    other_bytes: i64,
}

#[repr(C)]
struct NtThread {
    kernel: i64,
    user: i64,
    create: i64,
    wait_time: u32,
    start: usize,
    pid: usize,
    tid: usize,
    priority: i32,
    base_priority: i32,
    switches: u32,
    state: u32,
    wait_reason: u32,
}

#[repr(C)]
struct NtProcessId {
    pid: usize,
    name: NtString,
}

/// State the UI thread and the sampling thread both touch.
pub struct Shared {
    pub slot: Mutex<Option<Arc<Snapshot>>>,
    /// 0 pauses sampling.
    pub interval_ms: AtomicU32,
    pub minimized: AtomicBool,
    pub want_gui_counts: AtomicBool,
    pub hwnd: AtomicIsize,
    pub wake: AtomicIsize,
    pub icon_px: AtomicI32,
    pub enrich_posted: AtomicBool,
    force: AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        let wake = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        Arc::new(Self {
            slot: Mutex::new(None),
            interval_ms: AtomicU32::new(1000),
            minimized: AtomicBool::new(false),
            want_gui_counts: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            wake: AtomicIsize::new(wake as isize),
            icon_px: AtomicI32::new(16),
            enrich_posted: AtomicBool::new(false),
            force: AtomicBool::new(true),
        })
    }

    /// Makes the sampler take a fresh sample right away.
    pub fn poke(&self) {
        self.force.store(true, Ordering::Release);
        unsafe { SetEvent(self.wake.load(Ordering::Relaxed) as HANDLE) };
    }

    fn post(&self, msg: u32) {
        let hwnd = self.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, msg, 0, 0) };
        }
    }
}

struct Prev {
    cpu_time: u64,
    ws: u64,
    page_faults: u32,
    stat: Arc<ProcStatic>,
    seen: u64,
}

pub struct Sampler {
    shared: Arc<Shared>,
    buf: Vec<u64>,
    prev: HashMap<(u32, i64), Prev>,
    images: HashMap<String, Arc<ImageInfo>>,
    sids: HashMap<Vec<u8>, Arc<str>>,
    devices: Vec<(String, String)>,
    enrich: Sender<Arc<ImageInfo>>,
    last_sample: Option<Instant>,
    ncpu: f64,
    generation: u64,
}

pub fn start(shared: Arc<Shared>) {
    let (tx, rx) = channel();
    let enrich_shared = shared.clone();
    std::thread::Builder::new()
        .name("enrich".into())
        .spawn(move || enrich_loop(rx, enrich_shared))
        .ok();
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut s = Sampler::new(shared.clone(), tx);
            loop {
                let scheduled = !shared.minimized.load(Ordering::Relaxed)
                    && shared.interval_ms.load(Ordering::Relaxed) != 0;
                if shared.force.swap(false, Ordering::AcqRel) || scheduled {
                    publish(&shared, s.sample());
                }
                wait(&shared);
            }
        })
        .ok();
}

fn publish(shared: &Shared, snap: Snapshot) {
    *shared.slot.lock().unwrap() = Some(Arc::new(snap));
    shared.post(WM_APP_SNAPSHOT);
}

fn wait(shared: &Shared) {
    let ms = shared.interval_ms.load(Ordering::Relaxed);
    let timeout = if ms == 0 { INFINITE } else { ms };
    unsafe { WaitForSingleObject(shared.wake.load(Ordering::Relaxed) as HANDLE, timeout) };
}

impl Sampler {
    pub fn new(shared: Arc<Shared>, enrich: Sender<Arc<ImageInfo>>) -> Self {
        let ncpu = unsafe { GetActiveProcessorCount(0xFFFF) }.max(1) as f64;
        Self {
            shared,
            buf: vec![0; 64 * 1024],
            prev: HashMap::with_capacity(1024),
            images: HashMap::with_capacity(512),
            sids: HashMap::new(),
            devices: Vec::new(),
            enrich,
            last_sample: None,
            ncpu,
            generation: 0,
        }
    }

    fn query(&mut self) -> bool {
        loop {
            let mut needed = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    self.buf.as_mut_ptr().cast(),
                    (self.buf.len() * 8) as u32,
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                self.buf.resize(needed as usize / 8 + 16 * 1024, 0);
                continue;
            }
            return status >= 0;
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let mut snap = Snapshot::default();
        if !self.query() {
            return snap;
        }
        let now = Instant::now();
        let elapsed_100ns = self
            .last_sample
            .map_or(0.0, |t| (now - t).as_secs_f64() * 1e7);
        self.last_sample = Some(now);
        self.generation += 1;
        let capacity = elapsed_100ns * self.ncpu;

        let (hung, windowed) = window_owners();
        let want_gui = self.shared.want_gui_counts.load(Ordering::Relaxed);
        let mut wts: Option<HashMap<u32, Arc<str>>> = None;
        let mut idle_pct = 0.0f32;

        let mut raw_list = Vec::with_capacity(512);
        let base = self.buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            let p = unsafe { &*(base.add(offset) as *const NtProcess) };
            raw_list.push(offset);
            if p.next == 0 {
                break;
            }
            offset += p.next as usize;
        }

        snap.procs.reserve(raw_list.len());
        for offset in raw_list {
            let p = unsafe { &*(self.buf.as_ptr().cast::<u8>().add(offset) as *const NtProcess) };
            let pid = p.pid as u32;
            if p.threads == 0 && pid != 0 {
                continue;
            }
            let threads = unsafe {
                std::slice::from_raw_parts(
                    (p as *const NtProcess).add(1) as *const NtThread,
                    p.threads as usize,
                )
            };
            let suspended = !threads.is_empty()
                && pid > 4
                && threads.iter().all(|t| t.state == 5 && t.wait_reason == 5);
            let (name, create_time) = (nt_string(&p.name), p.create_time);
            let key = (pid, create_time);
            let cpu_time = (p.kernel_time + p.user_time) as u64;
            let ws = p.ws as u64;
            let page_faults = p.page_faults;

            let (stat, cpu, ws_delta, pf_delta) = match self.prev.get(&key) {
                Some(prev) => {
                    let cpu = if capacity > 0.0 {
                        (cpu_time.saturating_sub(prev.cpu_time) as f64 / capacity * 100.0) as f32
                    } else {
                        0.0
                    };
                    (
                        prev.stat.clone(),
                        cpu,
                        ws as i64 - prev.ws as i64,
                        page_faults.saturating_sub(prev.page_faults),
                    )
                }
                None => {
                    let stat = self.discover(pid, name, &mut wts);
                    (stat, 0.0, 0, 0)
                }
            };
            self.prev.insert(
                key,
                Prev {
                    cpu_time,
                    ws,
                    page_faults,
                    stat: stat.clone(),
                    seen: self.generation,
                },
            );
            if pid == 0 {
                idle_pct = cpu;
            }

            let private_ws = p.private_ws as u64;
            let (user_objects, gdi_objects) = if want_gui { gui_counts(pid) } else { (0, 0) };
            let status = if suspended {
                Status::Suspended
            } else if hung.contains(&pid) {
                Status::NotResponding
            } else {
                Status::Running
            };
            snap.procs.push(Proc {
                pid,
                ppid: p.ppid as u32,
                create_time,
                stat,
                status,
                session: p.session,
                has_window: windowed.contains(&pid),
                cpu,
                cpu_time,
                cycles: p.cycles,
                ws,
                peak_ws: p.peak_ws as u64,
                ws_delta,
                private_ws,
                active_private: if suspended { 0 } else { private_ws },
                commit: p.private_pages as u64,
                paged: p.paged as u64,
                nonpaged: p.nonpaged as u64,
                page_faults,
                pf_delta,
                base_priority: p.base_priority,
                handles: p.handles,
                threads: p.threads,
                user_objects,
                gdi_objects,
                io_reads: p.reads as u64,
                io_writes: p.writes as u64,
                io_other: p.others as u64,
                io_read_bytes: p.read_bytes as u64,
                io_write_bytes: p.write_bytes as u64,
                io_other_bytes: p.other_bytes as u64,
            });
        }

        let generation = self.generation;
        self.prev.retain(|_, v| v.seen == generation);
        snap.cpu_total = if capacity > 0.0 {
            (100.0 - idle_pct).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let mut mem: windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX =
            unsafe { std::mem::zeroed() };
        mem.dwLength = std::mem::size_of_val(&mem) as u32;
        unsafe { GlobalMemoryStatusEx(&mut mem) };
        snap.mem_total = mem.ullTotalPhys;
        snap.mem_used = mem.ullTotalPhys - mem.ullAvailPhys;
        snap
    }

    fn discover(
        &mut self,
        pid: u32,
        name: String,
        wts: &mut Option<HashMap<u32, Arc<str>>>,
    ) -> Arc<ProcStatic> {
        let name = match pid {
            0 => "System Idle Process".to_string(),
            _ if name.is_empty() => "(不明)".to_string(),
            _ => name,
        };
        if wts.is_none() {
            *wts = Some(self.wts_users());
        }
        let mut user = wts.as_ref().and_then(|m| m.get(&pid).cloned());

        let mut arch = "";
        let mut elevated = None;
        let mut cmdline = String::new();
        let mut package = None;
        if pid > 4 {
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if !h.is_null() {
                arch = architecture(h);
                cmdline = command_line(h);
                package = package_name(h);
                let mut token = std::ptr::null_mut();
                if unsafe { OpenProcessToken(h, TOKEN_QUERY, &mut token) } != 0 {
                    elevated = token_elevated(token);
                    if user.is_none() {
                        user = self.token_user(token);
                    }
                    unsafe { CloseHandle(token) };
                }
                unsafe { CloseHandle(h) };
            }
        }

        let image = if pid > 4 { self.image_for(pid) } else { None };
        let user = user.unwrap_or_else(|| Arc::from(fallback_user(pid, &name)));
        Arc::new(ProcStatic {
            name,
            user,
            arch: if arch.is_empty() && pid > 4 { UNKNOWN } else { arch },
            elevated,
            cmdline,
            image,
            package,
        })
    }

    fn image_for(&mut self, pid: u32) -> Option<Arc<ImageInfo>> {
        let nt_path = image_path(pid)?;
        let path = self.to_dos_path(&nt_path)?;
        if let Some(i) = self.images.get(&path) {
            return Some(i.clone());
        }
        let info = Arc::new(ImageInfo {
            path: path.clone(),
            description: OnceLock::new(),
            icon: OnceLock::new(),
        });
        self.images.insert(path, info.clone());
        let _ = self.enrich.send(info.clone());
        Some(info)
    }

    fn to_dos_path(&mut self, nt: &str) -> Option<String> {
        if !nt.starts_with('\\') {
            return Some(nt.to_string());
        }
        for attempt in 0..2 {
            for (device, drive) in &self.devices {
                if let Some(rest) = nt.strip_prefix(device.as_str()) {
                    if rest.starts_with('\\') {
                        return Some(format!("{drive}{rest}"));
                    }
                }
            }
            if attempt == 0 {
                self.devices = dos_devices();
            }
        }
        nt.strip_prefix("\\Device\\Mup").map(|r| format!("\\{r}"))
    }

    fn wts_users(&mut self) -> HashMap<u32, Arc<str>> {
        let mut out = HashMap::new();
        let mut level = 1u32;
        let mut data: *mut u16 = std::ptr::null_mut();
        let mut count = 0u32;
        let ok = unsafe {
            WTSEnumerateProcessesExW(
                WTS_CURRENT_SERVER_HANDLE,
                &mut level,
                WTS_ANY_SESSION,
                &mut data,
                &mut count,
            )
        };
        if ok == 0 {
            return out;
        }
        let items = unsafe {
            std::slice::from_raw_parts(data as *const WTS_PROCESS_INFO_EXW, count as usize)
        };
        for item in items {
            if !item.pUserSid.is_null() {
                if let Some(name) = self.sid_name(item.pUserSid) {
                    out.insert(item.ProcessId, name);
                }
            }
        }
        unsafe { WTSFreeMemoryExW(WTSTypeProcessInfoLevel1, data.cast(), count) };
        out
    }

    fn token_user(&mut self, token: HANDLE) -> Option<Arc<str>> {
        let mut buf = [0u64; 32];
        let mut len = 0;
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), 256, &mut len)
        };
        if ok == 0 {
            return None;
        }
        let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        self.sid_name(user.User.Sid)
    }

    fn sid_name(&mut self, sid: PSID) -> Option<Arc<str>> {
        let len = unsafe { GetLengthSid(sid) } as usize;
        let key = unsafe { std::slice::from_raw_parts(sid as *const u8, len) }.to_vec();
        if let Some(n) = self.sids.get(&key) {
            return Some(n.clone());
        }
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let (mut n, mut d, mut kind) = (256u32, 256u32, 0);
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            )
        };
        if ok == 0 {
            return None;
        }
        let name: Arc<str> = Arc::from(String::from_utf16_lossy(&name[..n as usize]));
        self.sids.insert(key, name.clone());
        Some(name)
    }
}

/// Without admin rights Windows withholds the owner of most session 0
/// processes. Only kernel-owned pseudo processes are certain to be SYSTEM.
fn fallback_user(pid: u32, name: &str) -> &'static str {
    let kernel = pid <= 4 || matches!(name, "Registry" | "Memory Compression" | "Secure System");
    if kernel {
        "SYSTEM"
    } else {
        UNKNOWN
    }
}

fn nt_string(s: &NtString) -> String {
    if s.buf.is_null() || s.len == 0 {
        return String::new();
    }
    let chars = unsafe { std::slice::from_raw_parts(s.buf, s.len as usize / 2) };
    String::from_utf16_lossy(chars)
}

fn image_path(pid: u32) -> Option<String> {
    let mut chars = vec![0u16; 2048];
    let mut info = NtProcessId {
        pid: pid as usize,
        name: NtString {
            len: 0,
            max: (chars.len() * 2) as u16,
            buf: chars.as_mut_ptr(),
        },
    };
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_ID_INFORMATION,
            (&mut info as *mut NtProcessId).cast(),
            std::mem::size_of::<NtProcessId>() as u32,
            std::ptr::null_mut(),
        )
    };
    if status < 0 || info.name.len == 0 {
        return None;
    }
    Some(nt_string(&info.name))
}

fn dos_devices() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = [letter as u16, b':' as u16, 0];
        let mut target = [0u16; 512];
        let n = unsafe { QueryDosDeviceW(drive.as_ptr(), target.as_mut_ptr(), 512) } as usize;
        if n > 0 {
            let end = target[..n].iter().position(|c| *c == 0).unwrap_or(n);
            out.push((
                String::from_utf16_lossy(&target[..end]),
                format!("{}:", letter as char),
            ));
        }
    }
    out
}

fn architecture(h: HANDLE) -> &'static str {
    let (mut process, mut native) = (0u16, 0u16);
    if unsafe { IsWow64Process2(h, &mut process, &mut native) } == 0 {
        return "";
    }
    match if process == 0 { native } else { process } {
        0x8664 => "x64",
        0xAA64 => "ARM64",
        0x014C => "x86",
        0x01C4 => "ARM",
        _ => "",
    }
}

fn command_line(h: HANDLE) -> String {
    let mut buf = vec![0u64; 512];
    let mut ret = 0u32;
    let mut status = unsafe {
        NtQueryInformationProcess(
            h,
            PROCESS_COMMAND_LINE_INFORMATION,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut ret,
        )
    };
    if status == STATUS_INFO_LENGTH_MISMATCH && ret > 0 {
        buf.resize(ret as usize / 8 + 8, 0);
        status = unsafe {
            NtQueryInformationProcess(
                h,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut ret,
            )
        };
    }
    if status < 0 {
        return String::new();
    }
    nt_string(unsafe { &*(buf.as_ptr() as *const NtString) })
}

fn package_name(h: HANDLE) -> Option<Arc<str>> {
    let mut name = [0u16; 128];
    let mut len = name.len() as u32;
    if unsafe { GetPackageFullName(h, &mut len, name.as_mut_ptr()) } != 0 || len == 0 {
        return None;
    }
    Some(Arc::from(String::from_utf16_lossy(&name[..len as usize - 1])))
}

fn token_elevated(token: HANDLE) -> Option<bool> {
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut e as *mut TOKEN_ELEVATION).cast(),
            4,
            &mut len,
        )
    };
    (ok != 0).then_some(e.TokenIsElevated != 0)
}

fn gui_counts(pid: u32) -> (u32, u32) {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return (0, 0);
        }
        let out = (
            GetGuiResources(h, GR_USEROBJECTS),
            GetGuiResources(h, GR_GDIOBJECTS),
        );
        CloseHandle(h);
        out
    }
}

/// Processes with a hung window, and processes with any visible window.
fn window_owners() -> (HashSet<u32>, HashSet<u32>) {
    unsafe extern "system" fn visit(hwnd: HWND, lp: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd) != 0 {
            let (hung, windowed) = &mut *(lp as *mut (HashSet<u32>, HashSet<u32>));
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            windowed.insert(pid);
            if IsHungAppWindow(hwnd) != 0 {
                hung.insert(pid);
            }
        }
        1
    }
    let mut sets = (HashSet::new(), HashSet::new());
    unsafe { EnumWindows(Some(visit), &mut sets as *mut _ as LPARAM) };
    sets
}

fn enrich_loop(rx: Receiver<Arc<ImageInfo>>, shared: Arc<Shared>) {
    while let Ok(image) = rx.recv() {
        let wide: Vec<u16> = image.path.encode_utf16().chain([0]).collect();
        let _ = image.description.set(Arc::from(file_description(&wide)));
        let _ = image
            .icon
            .set(load_icon(&wide, shared.icon_px.load(Ordering::Relaxed)));
        if !shared.enrich_posted.swap(true, Ordering::AcqRel) {
            shared.post(WM_APP_ENRICHED);
        }
    }
}

fn file_description(path: &[u16]) -> String {
    const FILE_VER_GET_LOCALISED: u32 = 1;
    unsafe {
        let mut dummy = 0;
        let size =
            GetFileVersionInfoSizeExW(FILE_VER_GET_LOCALISED, path.as_ptr(), &mut dummy) as usize;
        if size == 0 {
            return String::new();
        }
        let mut data = vec![0u8; size];
        if GetFileVersionInfoExW(
            FILE_VER_GET_LOCALISED,
            path.as_ptr(),
            0,
            size as u32,
            data.as_mut_ptr().cast(),
        ) == 0
        {
            return String::new();
        }
        let query: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
        if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut ptr, &mut len) == 0 || len < 4
        {
            return String::new();
        }
        let pairs = std::slice::from_raw_parts(ptr as *const [u16; 2], len as usize / 4);
        let japanese = pairs.iter().find(|p| p[0] == 0x0411);
        for pair in japanese.into_iter().chain(pairs.iter()) {
            let key = format!(
                "\\StringFileInfo\\{:04x}{:04x}\\FileDescription\0",
                pair[0], pair[1]
            );
            let key: Vec<u16> = key.encode_utf16().collect();
            if VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut ptr, &mut len) != 0
                && len > 1
            {
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let end = chars.iter().position(|c| *c == 0).unwrap_or(chars.len());
                return String::from_utf16_lossy(&chars[..end]).trim().to_string();
            }
        }
        String::new()
    }
}

fn load_icon(path: &[u16], size: i32) -> Option<Icon> {
    unsafe {
        let mut icon: HICON = std::ptr::null_mut();
        let mut id = 0;
        let n = PrivateExtractIconsW(path.as_ptr(), 0, size, size, &mut icon, &mut id, 1, 0);
        if n == 0 || n == u32::MAX || icon.is_null() {
            return None;
        }
        let out = icon_pixels(icon, size);
        DestroyIcon(icon);
        out
    }
}

unsafe fn icon_pixels(icon: HICON, size: i32) -> Option<Icon> {
    let mut info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }
    let hdc = CreateCompatibleDC(std::ptr::null_mut());
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = size;
    bmi.bmiHeader.biHeight = -size;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut px = vec![0u32; (size * size) as usize];
    let lines = GetDIBits(
        hdc,
        info.hbmColor,
        0,
        size as u32,
        px.as_mut_ptr().cast(),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    let mut ok = lines == size;
    if ok {
        if px.iter().any(|p| p >> 24 != 0) {
            for p in &mut px {
                *p = premultiply(*p);
            }
        } else {
            let mut mask = vec![0u32; (size * size) as usize];
            ok = GetDIBits(
                hdc,
                info.hbmMask,
                0,
                size as u32,
                mask.as_mut_ptr().cast(),
                &mut bmi,
                DIB_RGB_COLORS,
            ) == size;
            for (p, m) in px.iter_mut().zip(&mask) {
                *p = if *m & 0xFFFFFF != 0 { 0 } else { *p | 0xFF000000 };
            }
        }
    }
    DeleteDC(hdc);
    if !info.hbmColor.is_null() {
        DeleteObject(info.hbmColor);
    }
    DeleteObject(info.hbmMask);
    ok.then_some(Icon { size, px })
}

fn premultiply(p: u32) -> u32 {
    let a = p >> 24;
    let scale = |c: u32| (c * a + 127) / 255;
    (a << 24) | (scale((p >> 16) & 255) << 16) | (scale((p >> 8) & 255) << 8) | scale(p & 255)
}
