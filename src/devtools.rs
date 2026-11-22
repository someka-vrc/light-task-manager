//! Headless developer tools: `--bench <out.txt>`, `--shot <out.bmp> [page] [expand] [extra samples]`
//! and `--tray-icons <out.bmp>`.

use crate::app::core::Core;
use crate::settings::Settings;
use crate::system::model::{Proc, ProcStatic, Snapshot};
use crate::system::sampler::{self, Shared};
use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn live_snapshot(shared: &Arc<Shared>) -> Arc<Snapshot> {
    shared.icon_px.store(16, Ordering::Relaxed);
    sampler::start(shared.clone(), None);
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
                package: p.stat.package.clone(),
                platform: p.stat.platform,
                system_dir: p.stat.system_dir,
                protect: p.stat.protect,
            });
            procs.push(copy);
        }
    }
    Arc::new(Snapshot {
        taken: base.taken,
        stale: base.stale,
        procs,
        has_procs: true,
        cpu_total: base.cpu_total,
        mem_total: base.mem_total,
        mem_used: base.mem_used,
        sys: base.sys.clone(),
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

/// Writes the notification area icons to a BMP, to judge how they look: one row per icon size,
/// the plain icon and the bar graph at a few levels, on a dark and on a light taskbar, each
/// at its real size and then enlarged.
pub fn tray_icons(path: &str) {
    use crate::app::icon::{app_pixels, bar_heights, graph_pixels, Levels};
    const ZOOM: i32 = 6;
    const SIZES: [i32; 4] = [16, 20, 24, 32];
    let levels = [
        None,
        Some(Levels { cpu: 0.0, mem: 0.0, gpu: None }),
        Some(Levels { cpu: 3.0, mem: 45.0, gpu: Some(8.0) }),
        Some(Levels { cpu: 60.0, mem: 75.0, gpu: None }),
        Some(Levels { cpu: 100.0, mem: 100.0, gpu: Some(100.0) }),
    ];
    let backgrounds = [0x202020u32, 0xF3F3F3];
    let cell = 32 * ZOOM + 16;
    let real = 48;
    let cols = 1 + levels.len() as i32;
    let w = backgrounds.len() as i32 * cols * (cell + real);
    let h = SIZES.len() as i32 * (cell + 8);
    let mut img = vec![0u32; (w * h) as usize];
    for (bi, bg) in backgrounds.iter().enumerate() {
        let x0 = bi as i32 * cols * (cell + real);
        for x in x0..x0 + cols * (cell + real) {
            for y in 0..h {
                img[(y * w + x) as usize] = *bg;
            }
        }
        for (si, size) in SIZES.iter().enumerate() {
            for col in 0..cols {
                let px = if col == 0 { app_pixels(*size) } else { graph_pixels(*size, bar_heights(levels[col as usize - 1], *size)) };
                let (cx, cy) = (x0 + col * (cell + real), si as i32 * (cell + 8) + 8);
                // Premultiplied pixels over the background.
                let over = |src: u32, dst: u32| {
                    let a = src >> 24;
                    let ch = |s: u32| (((src >> s) & 255) + ((dst >> s) & 255) * (255 - a) / 255).min(255) << s;
                    ch(16) | ch(8) | ch(0)
                };
                for y in 0..*size {
                    for x in 0..*size {
                        let p = px[(y * size + x) as usize];
                        let at = ((cy + y) * w + cx + 8 + x) as usize;
                        img[at] = over(p, img[at]);
                        for dy in 0..ZOOM {
                            for dx in 0..ZOOM {
                                let at = ((cy + y * ZOOM + dy) * w + cx + real + x * ZOOM + dx) as usize;
                                img[at] = over(p, img[at]);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::with_capacity(img.len() * 4 + 54);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((img.len() * 4 + 54) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(-h).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for p in img {
        out.extend_from_slice(&(p | 0xFF000000).to_le_bytes());
    }
    let _ = std::fs::write(path, out);
}

pub fn shot(args: &[String]) {
    let path = args.first().map_or("shot.bmp", String::as_str);
    let page: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let shared = Shared::new();
    let extra = &args[2.min(args.len())..];
    // `lang=en|ja|zh-Hans|ko` picks the language; by default it follows Windows, as the app does.
    let lang = extra.iter().find_map(|a| a.strip_prefix("lang=")).and_then(crate::i18n::Lang::from_key);
    crate::i18n::set(crate::i18n::resolve(lang));
    // `from_start` hands the pages every sample from the first one, as the app does at start-up.
    let from_start = extra.iter().any(|a| a == "from_start");
    let live = if from_start {
        shared.icon_px.store(16, Ordering::Relaxed);
        sampler::start(shared.clone(), None);
        Arc::new(Snapshot::default())
    } else {
        live_snapshot(&shared)
    };
    // `tm_gpu` records GPU use in the graph from the start.
    // `tm_off` turns the graph off.
    let settings = Settings {
        // `tm=key,key` picks the graph's lines.
        tm_targets: if let Some(list) = extra.iter().find_map(|a| a.strip_prefix("tm=")) {
            list.split(',').map(String::from).collect()
        } else if extra.iter().any(|a| a == "tm_gpu") {
            vec!["cpu".into(), "mem".into(), "gpu".into()]
        } else {
            Settings::default().tm_targets
        },
        tm_enabled: !extra.iter().any(|a| a == "tm_off"),
        // `view=flat|group|tree` picks the process page's view mode.
        view: match extra.iter().find_map(|a| a.strip_prefix("view=")) {
            Some("flat") => crate::settings::ViewMode::Flat,
            Some("tree") => crate::settings::ViewMode::Tree,
            _ => crate::settings::ViewMode::Group,
        },
        // `rec_failed` shows the recording as unable to write; `rec_dir=path` picks its folder.
        rec_dir: extra.iter().find_map(|a| a.strip_prefix("rec_dir=")).unwrap_or("").to_string(),
        rec_running: extra.iter().any(|a| a == "rec_failed").then(|| std::path::PathBuf::from(r"X:\no\history")),
        ..Default::default()
    };
    // `size=w,h` sets the window size (default 1240x760).
    let size = extra
        .iter()
        .find_map(|a| a.strip_prefix("size="))
        .and_then(|v| v.split_once(','))
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        .unwrap_or((1240, 760));
    if extra.iter().any(|a| a == "rec_failed") {
        crate::system::recorder::FAILED.store(true, Ordering::Relaxed);
    }
    let mut core = Core::new(settings, shared.clone(), 1.0, size.0, size.1);
    core.navigate(page);
    core.on_snapshot(&live);
    // A few more samples give the performance graphs some history.
    let samples = extra.iter().find_map(|a| a.parse::<u32>().ok()).unwrap_or(0);
    // `minimized` takes those samples as if the window were minimized.
    if extra.iter().any(|a| a == "minimized") {
        shared.minimized.store(true, Ordering::Relaxed);
    }
    let describe = |s: &Snapshot| {
        let busiest = s.procs.iter().filter(|p| p.pid != 0).max_by(|a, b| a.cpu.total_cmp(&b.cpu));
        let (name, top) = busiest.map_or(("-", 0.0), |p| (p.stat.name.as_str(), p.cpu));
        let gpus: Vec<String> = s.sys.gpus.iter().map(|g| format!("{:.0}%", g.util)).collect();
        format!(
            "has_procs={} procs={} processes={} cpu={:.1}% busiest={name} {top:.1}% gpus=[{}]",
            s.has_procs,
            s.procs.len(),
            s.sys.cpu.processes,
            s.sys.cpu.total,
            gpus.join(" ")
        )
    };
    // `verbose` prints every sample.
    let verbose = extra.iter().any(|a| a == "verbose");
    let mut received = 0;
    let mut last: Option<Arc<Snapshot>> = None;
    for _ in 0..samples {
        std::thread::sleep(Duration::from_millis(1050));
        let next = shared.slot.lock().unwrap().take();
        if let Some(snap) = next {
            received += 1;
            if verbose {
                println!("sample {received}: {}", describe(&snap));
            }
            core.on_snapshot(&snap);
            last = Some(snap);
        }
    }
    println!("samples received: {received}/{samples}");
    if let Some(s) = &last {
        println!("last sample: {}", describe(s));
    }
    // `restore` takes one more sample as if the window came back from minimized.
    if extra.iter().any(|a| a == "restore") {
        shared.minimized.store(false, Ordering::Relaxed);
        shared.poke();
        std::thread::sleep(Duration::from_millis(400));
        if let Some(snap) = shared.slot.lock().unwrap().take() {
            println!("after restore: {}", describe(&snap));
            core.on_snapshot(&snap);
        }
    }
    println!("needs: {:#08b}", shared.needs());
    if extra.iter().any(|a| a == "expand") {
        core.page_is_details().unwrap().expand_all_for_test();
    }
    // `lifetime` takes one more update in which two processes end and one starts.
    if extra.iter().any(|a| a == "lifetime") {
        let settings = Settings { view: core.settings.view, ..Default::default() };
        core.page_is_details().unwrap().simulate_lifetime_for_test(&settings);
    }
    // Input to replay before the picture: type=text  click=x,y  rclick=x,y  drag=x1,y1,x2,y2  move=x,y  zoom=x,y,notches (Ctrl+wheel).
    // Pages learn where things are while painting, so paint once first.
    core.render();
    for a in extra {
        let Some((kind, rest)) = a.split_once('=') else { continue };
        if kind == "filter" {
            core.set_filter(rest.to_string());
            core.render();
            continue;
        }
        if kind == "type" {
            for ch in rest.chars() {
                core.event(crate::pages::Ev::Char { ch });
            }
            core.render();
            continue;
        }
        let n: Vec<i32> = rest.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        let mut fire = |ev: crate::pages::Ev| {
            core.event(ev);
        };
        use crate::pages::Ev;
        match (kind, n.as_slice()) {
            ("click", [x, y]) => {
                fire(Ev::Move { x: *x, y: *y });
                fire(Ev::Down { x: *x, y: *y, right: false });
                fire(Ev::Up { x: *x });
            }
            ("rclick", [x, y]) => {
                fire(Ev::Move { x: *x, y: *y });
                fire(Ev::Down { x: *x, y: *y, right: true });
                fire(Ev::Up { x: *x });
            }
            ("drag", [x1, y1, x2, y2]) => {
                fire(Ev::Move { x: *x1, y: *y1 });
                fire(Ev::Down { x: *x1, y: *y1, right: false });
                fire(Ev::Move { x: *x2, y: *y2 });
                fire(Ev::Up { x: *x2 });
            }
            ("move", [x, y]) => fire(Ev::Move { x: *x, y: *y }),
            ("key", [vk]) => fire(Ev::Key { vk: *vk as u16, ctrl: false }),
            ("wheel", [x, y, dx, dy]) => {
                fire(Ev::Move { x: *x, y: *y });
                fire(Ev::Wheel { dx: *dx as f32, dy: *dy as f32, ctrl: false });
            }
            ("zoom", [x, y, notches]) => {
                fire(Ev::Move { x: *x, y: *y });
                fire(Ev::Wheel { dx: 0.0, dy: *notches as f32, ctrl: true });
            }
            _ => {}
        }
        core.render();
    }
    core.render();
    core.render();
    let _ = core.canvas.save_bmp(path);
    std::process::exit(0);
}
