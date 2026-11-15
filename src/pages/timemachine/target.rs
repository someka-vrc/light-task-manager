//! What the time machine can show, and which of them the settings turn on.

use super::*;

/// What the time machine can show. Every value is recorded whichever are chosen,
/// so turning one on also shows the past; only the GPU ones cost something to read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Target {
    Cpu,
    CpuDetail,
    Mem,
    Commit,
    Disk,
    Gpu,
    GpuMem,
    Handles,
    Faults,
    Hung,
}

/// The order of the settings dialog: the left column, then the right one.
pub(super) const DIALOG_ORDER: [Target; 10] = [
    Target::Cpu,
    Target::Mem,
    Target::Gpu,
    Target::Disk,
    Target::Commit,
    Target::Handles,
    Target::Hung,
    Target::CpuDetail,
    Target::Faults,
    Target::GpuMem,
];

/// The order of the legend, the tooltip and the table.
pub(super) const SHOW_ORDER: [Target; 10] = [
    Target::Cpu,
    Target::CpuDetail,
    Target::Mem,
    Target::Commit,
    Target::Disk,
    Target::Gpu,
    Target::GpuMem,
    Target::Handles,
    Target::Faults,
    Target::Hung,
];

impl Target {
    /// The name in the settings file.
    pub(super) fn key(self) -> &'static str {
        match self {
            Target::Cpu => "cpu",
            Target::CpuDetail => "cpu_detail",
            Target::Mem => "mem",
            Target::Commit => "commit",
            Target::Disk => "disk",
            Target::Gpu => "gpu",
            Target::GpuMem => "gpu_mem",
            Target::Handles => "handles",
            Target::Faults => "faults",
            Target::Hung => "hung",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Target::Cpu => "CPU",
            Target::CpuDetail => tr().tm.cpu_detail,
            Target::Mem => tr().common.memory,
            Target::Commit => tr().common.commit_mem,
            Target::Disk => tr().common.disk,
            Target::Gpu => "GPU",
            Target::GpuMem => tr().common.gpu_mem,
            Target::Handles => tr().common.handles,
            Target::Faults => tr().common.page_faults,
            Target::Hung => tr().common.not_responding,
        }
    }

    /// Whether the table shows a value of it for each process.
    pub(super) fn per_process(self) -> bool {
        !matches!(self, Target::CpuDetail | Target::Hung)
    }

    pub(super) fn uses_gpu(self) -> bool {
        matches!(self, Target::Gpu | Target::GpuMem)
    }

    /// The heading of its columns in the table, looked up when shown.
    pub(super) fn group(self) -> Text {
        match self {
            Target::Cpu => |_| "CPU (%)",
            Target::Mem => |s| s.tm.mem_mb,
            Target::Commit => |s| s.tm.commit_mb,
            Target::Disk => |s| s.tm.disk_mbs,
            Target::Gpu => |_| "GPU (%)",
            Target::GpuMem => |s| s.tm.gpu_mem_mb,
            Target::Handles => |s| s.common.handles,
            Target::Faults => |s| s.tm.faults_ps,
            Target::CpuDetail | Target::Hung => |_| "",
        }
    }

    /// Technical names for the copy formats: the single-time column, then start, end, change, average and maximum.
    pub(super) fn props(self) -> [&'static str; 6] {
        match self {
            Target::Cpu => ["Sample.cpu", "Sample.cpu_start", "Sample.cpu_end", "Sample.cpu_delta", "Sample.cpu_avg", "Sample.cpu_max"],
            Target::Mem => ["Sample.memory", "Sample.mem_start", "Sample.mem_end", "Sample.mem_delta", "Sample.mem_avg", "Sample.mem_max"],
            Target::Commit => ["Sample.commit", "Sample.commit_start", "Sample.commit_end", "Sample.commit_delta", "Sample.commit_avg", "Sample.commit_max"],
            Target::Disk => ["Sample.disk", "Sample.disk_start", "Sample.disk_end", "Sample.disk_delta", "Sample.disk_avg", "Sample.disk_max"],
            Target::Gpu => ["Sample.gpu", "Sample.gpu_start", "Sample.gpu_end", "Sample.gpu_delta", "Sample.gpu_avg", "Sample.gpu_max"],
            Target::GpuMem => ["Sample.gpu_memory", "Sample.gpu_mem_start", "Sample.gpu_mem_end", "Sample.gpu_mem_delta", "Sample.gpu_mem_avg", "Sample.gpu_mem_max"],
            Target::Handles => ["Sample.handles", "Sample.handles_start", "Sample.handles_end", "Sample.handles_delta", "Sample.handles_avg", "Sample.handles_max"],
            Target::Faults => ["Sample.faults", "Sample.faults_start", "Sample.faults_end", "Sample.faults_delta", "Sample.faults_avg", "Sample.faults_max"],
            Target::CpuDetail | Target::Hung => [""; 6],
        }
    }

    /// Widths of the single-time column and of each range column.
    pub(super) fn widths(self) -> (i32, i32) {
        match self {
            Target::Mem | Target::Commit | Target::GpuMem => (120, 76),
            Target::Disk => (130, 68),
            Target::Faults => (160, 68),
            Target::Handles => (90, 64),
            _ => (90, 60),
        }
    }

    /// The value in the units of the export: ratios from 0 to 1, bytes, counts per second.
    pub(super) fn raw(self, p: &TmProc) -> f64 {
        match self {
            Target::Cpu => p.cpu as f64 / 100.0,
            Target::Mem => p.mem_kb as f64 * 1024.0,
            Target::Commit => p.commit_kb as f64 * 1024.0,
            Target::Disk => p.disk_bps as f64,
            Target::Gpu => p.gpu as f64 / 100.0,
            Target::GpuMem => p.gpu_mem_kb as f64 * 1024.0,
            Target::Handles => p.handles as f64,
            Target::Faults => p.faults as f64,
            Target::CpuDetail | Target::Hung => 0.0,
        }
    }

    /// A raw value in the units of the table: percent, MB, MB per second.
    pub(super) fn shown(self, raw: f64) -> f32 {
        let mb = 1024.0 * 1024.0;
        (match self {
            Target::Cpu | Target::Gpu => raw * 100.0,
            Target::Mem | Target::Commit | Target::GpuMem | Target::Disk => raw / mb,
            _ => raw,
        }) as f32
    }

    pub(super) fn value(self, p: &TmProc) -> f32 {
        self.shown(self.raw(p))
    }

    pub(super) fn decimals(self) -> usize {
        match self {
            Target::Gpu | Target::Handles | Target::Faults => 0,
            _ => 1,
        }
    }

    pub(super) fn text(self, v: f32) -> String {
        format!("{v:.*}", self.decimals())
    }

    /// A signed change; one that rounds to zero is shown without a sign.
    pub(super) fn delta(self, v: f32) -> String {
        let d = self.decimals();
        if v.abs() < 0.5 * 10f32.powi(-(d as i32)) {
            self.text(0.0)
        } else {
            format!("{v:+.*}", d)
        }
    }
}

pub(super) fn enabled(s: &Settings, t: Target) -> bool {
    s.tm_targets.iter().any(|k| k == t.key())
}

pub(super) fn toggle(s: &mut Settings, t: Target) {
    if enabled(s, t) {
        s.tm_targets.retain(|k| k != t.key());
    } else {
        s.tm_targets.push(t.key().to_string());
    }
}

pub(super) fn targets(s: &Settings) -> Vec<Target> {
    SHOW_ORDER.into_iter().filter(|t| enabled(s, *t)).collect()
}

