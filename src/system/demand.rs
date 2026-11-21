//! What the sampler has to read, declared by whoever needs it.
//!
//! Each page states two sets of needs (`Page::demand`): what it needs only while it
//! is shown and the window is not minimized, and what it needs at all times. The core
//! collects them after every change, and the sampler reads exactly their union, so a
//! costly counter runs only while something uses it. A new consumer only has to
//! declare its needs; nothing else has to know about it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Things the sampler reads only on demand.
pub mod need {
    /// The machine-wide values: CPU, memory, disks, network adapters. Any need samples.
    pub const SYS: u32 = 1 << 0;
    /// The GUI object counts of each process.
    pub const GUI_COUNTS: u32 = 1 << 1;
    /// The GPU counters of the machine (per adapter and engine).
    pub const SYS_GPU: u32 = 1 << 2;
    /// The GPU use of each process.
    pub const PROC_GPU: u32 = 1 << 3;
    /// The TCP rate of each process.
    pub const PROC_NET: u32 = 1 << 4;
    /// The power throttling state of each process, re-read every sample.
    pub const ECO_QOS: u32 = 1 << 5;
    /// The list of processes and what each uses. Without it a sample is machine-wide only.
    pub const PROCS: u32 = 1 << 6;
    /// Everything the window shows live while it is not minimized.
    pub const LIVE: u32 = SYS | PROCS;
}

/// The needs one consumer declares.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Needs {
    /// Needed only while the consumer is shown and the window is not minimized.
    pub shown: u32,
    /// Needed at all times, minimized or not.
    pub always: u32,
}

pub struct Demand {
    /// One entry per consumer (the page index).
    slots: Mutex<Vec<Needs>>,
    /// The consumer that is shown, or `usize::MAX` for none.
    visible: AtomicUsize,
    /// What a consumer that is no page needs at all times: the recording to the database.
    background: AtomicU32,
}

impl Default for Demand {
    fn default() -> Self {
        Self { slots: Mutex::new(Vec::new()), visible: AtomicUsize::new(usize::MAX), background: AtomicU32::new(0) }
    }
}

impl Demand {
    /// Replaces every consumer's needs. Returns true when this adds something to read.
    pub fn set_all(&self, needs: Vec<Needs>, minimized: bool) -> bool {
        let before = self.current(minimized);
        *self.slots.lock().unwrap() = needs;
        self.current(minimized) & !before != 0
    }

    /// Sets which consumer is shown. Returns true when this adds something to read.
    pub fn set_visible(&self, index: Option<usize>, minimized: bool) -> bool {
        let before = self.current(minimized);
        self.visible.store(index.unwrap_or(usize::MAX), Ordering::Relaxed);
        self.current(minimized) & !before != 0
    }

    /// Sets what the recording needs. Returns true when this adds something to read.
    pub fn set_background(&self, needs: u32, minimized: bool) -> bool {
        let before = self.current(minimized);
        self.background.store(needs, Ordering::Relaxed);
        self.current(minimized) & !before != 0
    }

    /// What has to be read now: what the pages and the recording need.
    pub fn current(&self, minimized: bool) -> u32 {
        self.screen(minimized) | self.background.load(Ordering::Relaxed)
    }

    /// What the pages need read now, which decides whether the screen is sampled.
    pub fn screen(&self, minimized: bool) -> u32 {
        let visible = self.visible.load(Ordering::Relaxed);
        let slots = self.slots.lock().unwrap();
        let mut out = if minimized { 0 } else { need::LIVE };
        for (i, n) in slots.iter().enumerate() {
            out |= n.always;
            if !minimized && i == visible {
                out |= n.shown;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_needs_follow_the_visible_page_and_the_window() {
        let d = Demand::default();
        let details = Needs { shown: need::PROC_GPU, always: 0 };
        let machine = Needs { shown: 0, always: need::SYS };
        // Live sampling is there anyway, so this adds nothing to read yet.
        assert!(!d.set_all(vec![details, machine], false));
        // Nothing is shown: only the live sampling and what is always needed.
        assert_eq!(d.current(false), need::LIVE);
        assert!(d.set_visible(Some(0), false));
        assert_eq!(d.current(false), need::LIVE | need::PROC_GPU);
        // Minimized: what is needed only while shown stops.
        assert_eq!(d.current(true), need::SYS);
        // Without anyone needing samples at all times, minimizing stops sampling.
        d.set_all(vec![details, Needs::default()], true);
        assert_eq!(d.current(true), 0);
    }

    #[test]
    fn reports_only_additions() {
        let d = Demand::default();
        d.set_visible(Some(0), false);
        assert!(d.set_all(vec![Needs { shown: need::SYS_GPU, always: 0 }], false));
        assert!(!d.set_all(vec![Needs { shown: need::SYS_GPU, always: 0 }], false));
        assert!(!d.set_all(vec![Needs::default()], false));
    }
}
