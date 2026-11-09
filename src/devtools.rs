//! Headless developer tools: `--bench <out.txt>` and `--shot <out.bmp> [filter]`.

use crate::core::Core;
use crate::model::{Proc, ProcStatic, Snapshot};
use crate::sampler::{self, Shared};
use crate::settings::Settings;
use std::fmt::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn live_snapshot(shared: &Arc<Shared>) -> Arc<Snapshot> {
    shared.icon_px.store(16, Ordering::Relaxed);
    sampler::start(shared.clone());
    // The second sample carries real CPU numbers and finished enrichment.
    std::thread::sleep(Duration::from_millis(2200));
    let snap = shared.slot.lock().unwrap().take();
    snap.unwrap_or_default()
}

fn inflate(base: &Snapshot, rows: usize) -> Arc<Snapshot> {
    let mut procs: Vec<Proc> = Vec::with_capacity(rows);
    let mut serial = 0u32;
    while procs.len() < rows {
        for p in &base.procs {
            if procs.len() == rows {
                break;
            }
            let mut copy = p.clone();
            serial += 1;
            copy.pid = 100_000 + serial * 4;
            copy.private_ws = copy.private_ws.wrapping_add(serial as u64 * 4096);
            copy.active_private = copy.private_ws;
            copy.stat = Arc::new(ProcStatic {
                name: format!("{}-{serial}", p.stat.name),
                user: p.stat.user.clone(),
                arch: p.stat.arch,
                elevated: p.stat.elevated,
                cmdline: String::new(),
                image: p.stat.image.clone(),
            });
            procs.push(copy);
        }
    }
    Arc::new(Snapshot {
        procs,
        cpu_total: base.cpu_total,
        mem_total: base.mem_total,
        mem_used: base.mem_used,
    })
}

fn stats(label: &str, samples: &mut [f64], out: &mut String) {
    samples.sort_by(f64::total_cmp);
    let avg = samples.iter().sum::<f64>() / samples.len() as f64;
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let _ = writeln!(out, "{label:<34} avg {avg:>8.3} ms   p99 {p99:>8.3} ms   max {:>8.3} ms", samples[samples.len() - 1]);
}

fn timed(mut f: impl FnMut(usize), count: usize) -> Vec<f64> {
    (0..count)
        .map(|i| {
            let start = Instant::now();
            f(i);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect()
}

pub fn bench(path: &str) {
    let shared = Shared::new();
    let live = live_snapshot(&shared);
    let settings = Settings::default();
    let mut out = String::new();
    let _ = writeln!(out, "live processes: {}", live.procs.len());

    for &(w, h, rows, scale) in &[
        (1920, 1080, 5000usize, 1.0f32),
        (3840, 2160, 5000, 1.0),
        (3100, 1900, 5000, 2.5),
    ] {
        let snap = inflate(&live, rows);
        let mut core = Core::new(Settings::default(), shared.clone(), scale, w, h);
        core.page_is_details().unwrap().load_for_test(snap, &settings);
        core.render();
        let _ = writeln!(out, "\n{w}x{h}, {rows} rows, scale {scale}");

        let mut full = timed(|_| core.render(), 300);
        stats("full frame", &mut full, &mut out);

        let max = core.page_is_details().unwrap().max_scroll();
        let mut scroll = timed(
            |i| {
                let y = i as f32 * 37.0 % max;
                core.page_is_details().unwrap().scroll_to(y);
                core.render();
            },
            1000,
        );
        stats("scroll + frame", &mut scroll, &mut out);

        let terms = ["c", "ch", "chr", "chro", "chrom", "chrome", "chrom", "svc", ""];
        let page = core.page_is_details().unwrap();
        let mut filter = timed(|i| page.filter_for_test(terms[i % terms.len()]), 900);
        stats("filter keystroke (no paint)", &mut filter, &mut out);

        let page = core.page_is_details().unwrap();
        let mut sort = timed(|_| page.resort_for_test(&settings), 100);
        stats("sort + filter", &mut sort, &mut out);

        let mut typing = timed(
            |i| {
                core.page_is_details().unwrap().filter_for_test(terms[i % terms.len()]);
                core.render();
            },
            300,
        );
        stats("filter keystroke + frame", &mut typing, &mut out);
    }

    let _ = std::fs::write(path, out);
    std::process::exit(0);
}

pub fn shot(args: &[String]) {
    let path = args.first().map_or("shot.bmp", String::as_str);
    let page: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let shared = Shared::new();
    let live = live_snapshot(&shared);
    let mut core = Core::new(Settings::default(), shared.clone(), 1.0, 1240, 760);
    core.active = page;
    core.on_snapshot(&live);
    core.render();
    core.render();
    let _ = core.canvas.save_bmp(path);
    std::process::exit(0);
}
