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
    pub icon: OnceLock<Option<Icon>>,
}

impl ImageInfo {
    pub fn description(&self) -> &str {
        self.description.get().map(|d| &**d).unwrap_or("")
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
}

impl ProcStatic {
    pub fn path(&self) -> &str {
        self.image.as_ref().map(|i| i.path.as_str()).unwrap_or("")
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
            Status::Running => "実行中",
            Status::Suspended => "中断",
            Status::NotResponding => "応答なし",
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
    /// Percent of total machine capacity.
    pub cpu: f32,
    /// 100ns units.
    pub cpu_time: u64,
    pub cycles: u64,
    pub ws: u64,
    pub peak_ws: u64,
    pub ws_delta: i64,
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
    pub procs: Vec<Proc>,
    pub cpu_total: f32,
    pub mem_total: u64,
    pub mem_used: u64,
}
