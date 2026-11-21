//! What the graph can plot, and which of them the settings turn on.

use crate::i18n::tr;
use crate::settings::Settings;

/// A line (or a set of lines, one per disk or adapter) of the graph. Every value is
/// recorded whichever are chosen, so turning one on also shows the past; only the GPU
/// ones cost something to read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Cpu,
    CpuDetail,
    Mem,
    Commit,
    Disk,
    Gpu,
    GpuMem,
}

/// The one order of the legend, the tooltip and the settings dialog: by device, each
/// main line followed by its detail.
pub const ORDER: [Target; 7] = [Target::Cpu, Target::CpuDetail, Target::Mem, Target::Commit, Target::Disk, Target::Gpu, Target::GpuMem];

impl Target {
    /// Where the settings dialog puts it: one row per device, the main line on the left
    /// and its detail on the right.
    pub fn cell(self) -> (i32, i32) {
        match self {
            Target::Cpu => (0, 0),
            Target::CpuDetail => (1, 0),
            Target::Mem => (0, 1),
            Target::Commit => (1, 1),
            Target::Disk => (0, 2),
            Target::Gpu => (0, 3),
            Target::GpuMem => (1, 3),
        }
    }

    /// The rows the settings dialog's grid takes.
    pub const ROWS: i32 = 4;

    /// The name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Target::Cpu => "cpu",
            Target::CpuDetail => "cpu_detail",
            Target::Mem => "mem",
            Target::Commit => "commit",
            Target::Disk => "disk",
            Target::Gpu => "gpu",
            Target::GpuMem => "gpu_mem",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Cpu => "CPU",
            Target::CpuDetail => tr().tm.cpu_kernel,
            Target::Mem => tr().common.memory,
            Target::Commit => tr().common.commit_mem,
            Target::Disk => tr().common.disk,
            Target::Gpu => "GPU",
            Target::GpuMem => tr().common.gpu_mem,
        }
    }

    pub fn uses_gpu(self) -> bool {
        matches!(self, Target::Gpu | Target::GpuMem)
    }
}

pub fn enabled(s: &Settings, t: Target) -> bool {
    s.tm_targets.iter().any(|k| k == t.key())
}

pub fn toggle(s: &mut Settings, t: Target) {
    if enabled(s, t) {
        s.tm_targets.retain(|k| k != t.key());
    } else {
        s.tm_targets.push(t.key().to_string());
    }
}

pub fn targets(s: &Settings) -> Vec<Target> {
    ORDER.into_iter().filter(|t| enabled(s, *t)).collect()
}
