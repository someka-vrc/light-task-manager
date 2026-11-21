//! The performance page: a device list with sparklines on the left and the
//! selected device's graphs and figures on the right.

use super::{Cx, Ev, Nav, Page};
use crate::i18n::tr;
use crate::settings::Settings;
use crate::system::model::{Snapshot, SysSample};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons;
use crate::ui::keys::{VK_DOWN, VK_F5, VK_UP};
use crate::ui::theme::THEME;
use crate::ui::widgets::{bits_per_sec, bytes, bytes_per_sec, uptime};
use std::collections::HashMap;
use std::sync::Arc;

const HISTORY: usize = 60;
const LIST_W: i32 = 250;
const ENTRY_H: i32 = 72;

#[derive(Default)]
struct Hist {
    a: Vec<f32>,
    b: Vec<f32>,
    c: Vec<f32>,
    cores: Vec<Vec<f32>>,
    engines: Vec<(String, Vec<f32>)>,
}

/// One sparkline of a device thumbnail: its history, its color, and whether it is filled.
type ThumbLine<'a> = (&'a [f32], u32, bool);

fn push(v: &mut Vec<f32>, x: f32) {
    if v.len() >= HISTORY {
        v.remove(0);
    }
    v.push(x);
}

#[derive(Clone, Copy, PartialEq)]
enum Hover {
    None,
    Entry(usize),
    Toggle(usize),
}

struct Entry {
    id: String,
    title: String,
    sub: String,
    value: String,
    color: u32,
}

pub struct PerformancePage {
    snap: Arc<Snapshot>,
    hist: HashMap<String, Hist>,
    selected: String,
    list_scroll: i32,
    hover: Hover,
    entry_rects: Vec<Rect>,
    list_rect: Rect,
    toggle_rects: [Rect; 2],
    cpu_logical: bool,
    mouse: (i32, i32),
}

impl PerformancePage {
    pub fn new() -> Self {
        Self {
            snap: Arc::new(Snapshot::default()),
            hist: HashMap::new(),
            selected: "cpu".into(),
            list_scroll: 0,
            hover: Hover::None,
            entry_rects: Vec::new(),
            list_rect: Rect::default(),
            toggle_rects: [Rect::default(); 2],
            cpu_logical: false,
            mouse: (-1, -1),
        }
    }

    fn record(&mut self, s: &SysSample) {
        let mut live: Vec<String> = vec!["cpu".into(), "mem".into()];

        let h = self.hist.entry("cpu".into()).or_default();
        push(&mut h.a, s.cpu.total);
        push(&mut h.b, s.cpu.kernel);
        h.cores.resize_with(s.cpu.cores.len(), Vec::new);
        for (i, v) in s.cpu.cores.iter().enumerate() {
            push(&mut h.cores[i], *v);
        }

        let m = &s.mem;
        let h = self.hist.entry("mem".into()).or_default();
        let used = m.total.saturating_sub(m.available);
        push(&mut h.a, used as f32 / m.total.max(1) as f32 * 100.0);

        for d in &s.disks {
            let id = format!("disk:{}", d.index);
            let h = self.hist.entry(id.clone()).or_default();
            push(&mut h.a, d.active);
            push(&mut h.b, d.read_bps as f32);
            push(&mut h.c, d.write_bps as f32);
            live.push(id);
        }
        for n in &s.nets {
            let id = format!("net:{}", n.index);
            let h = self.hist.entry(id.clone()).or_default();
            push(&mut h.a, n.send_bps as f32);
            push(&mut h.b, n.recv_bps as f32);
            live.push(id);
        }
        for g in &s.gpus {
            let id = format!("gpu:{}", g.luid);
            let h = self.hist.entry(id.clone()).or_default();
            push(&mut h.a, g.util);
            push(&mut h.b, g.dedicated as f32);
            push(&mut h.c, g.shared as f32);
            for (name, v) in &g.engines {
                match h.engines.iter_mut().find(|(n, _)| n == name) {
                    Some((_, series)) => push(series, *v),
                    None => {
                        let mut series = Vec::new();
                        push(&mut series, *v);
                        h.engines.push((name.clone(), series));
                    }
                }
            }
            for (name, series) in &mut h.engines {
                if !g.engines.iter().any(|(n, _)| n == name) {
                    push(series, 0.0);
                }
            }
            live.push(id);
        }
        // GPU history survives while an adapter is briefly missing from the counters.
        self.hist.retain(|k, _| k.starts_with("gpu:") || live.contains(k));
    }

    fn entries(&self) -> Vec<Entry> {
        let t = THEME;
        let s = &self.snap.sys;
        let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
        let mut out = Vec::new();
        let speed = if s.cpu.mhz > 0.0 { format!("  {:.2} GHz", s.cpu.mhz / 1000.0) } else { String::new() };
        out.push(Entry {
            id: "cpu".into(),
            title: "CPU".into(),
            sub: format!("{:.0}%{speed}", s.cpu.total),
            value: String::new(),
            color: t.chart_cpu,
        });
        let used = s.mem.total.saturating_sub(s.mem.available);
        out.push(Entry {
            id: "mem".into(),
            title: tr().common.memory.into(),
            sub: format!(
                "{:.1}/{:.1} GB ({:.0}%)",
                gb(used),
                gb(s.mem.total),
                used as f64 / s.mem.total.max(1) as f64 * 100.0
            ),
            value: String::new(),
            color: t.chart_mem,
        });
        for d in &s.disks {
            let letters = if d.letters.is_empty() { String::new() } else { format!(" ({})", d.letters) };
            out.push(Entry {
                id: format!("disk:{}", d.index),
                title: (tr().common.disk_n)(d.index, &letters),
                sub: d.info.kind.clone(),
                value: format!("{:.0}%", d.active),
                color: t.chart_disk,
            });
        }
        for n in &s.nets {
            out.push(Entry {
                id: format!("net:{}", n.index),
                title: n.kind.into(),
                sub: n.alias.clone(),
                value: (tr().perf.send_recv)(&short_rate(n.send_bps), &short_rate(n.recv_bps)),
                color: t.chart_net,
            });
        }
        let gpus: Vec<&String> = {
            let mut v: Vec<&String> = self.hist.keys().filter(|k| k.starts_with("gpu:")).collect();
            v.sort();
            v
        };
        for id in gpus.iter() {
            let live = s.gpus.iter().find(|g| format!("gpu:{}", g.luid) == **id);
            out.push(Entry {
                id: (*id).clone(),
                title: format!("GPU {}", crate::system::sysinfo::gpu_number(id.strip_prefix("gpu:").unwrap_or(id))),
                sub: live.map_or_else(String::new, |g| g.name.clone()),
                value: live.map_or_else(|| "—".into(), |g| format!("{:.0}%", g.util)),
                color: t.chart_gpu,
            });
        }
        out
    }

    fn time_label(settings: &Settings) -> String {
        let ms = if settings.interval_ms == 0 { 1000 } else { settings.interval_ms };
        let secs = HISTORY as u32 * ms / 1000;
        if secs >= 60 && secs.is_multiple_of(60) {
            (tr().units.minutes)(secs as u64 / 60)
        } else {
            (tr().units.seconds)(secs as u64)
        }
    }

    fn paint_list(&mut self, c: &mut Canvas, area: Rect, entries: &[Entry]) {
        let t = THEME;
        let list = rect(area.x + c.px(8), area.y + c.px(8), c.px(LIST_W), area.h - c.px(16));
        self.list_rect = list;
        let eh = c.px(ENTRY_H);
        let max_scroll = (eh * entries.len() as i32 - list.h).max(0);
        self.list_scroll = self.list_scroll.clamp(0, max_scroll);
        self.entry_rects.clear();
        c.set_clip(list);
        for (i, e) in entries.iter().enumerate() {
            let r = rect(list.x, list.y + i as i32 * eh - self.list_scroll, list.w, eh - c.px(4));
            self.entry_rects.push(r);
            let selected = e.id == self.selected;
            if selected {
                c.round(r, c.px(6) as f32, t.hover, 255);
                c.round(rect(r.x, r.y + c.px(18), c.px(3), r.h - c.px(36)), 1.5, t.accent, 255);
            } else if self.hover == Hover::Entry(i) {
                c.round(r, c.px(6) as f32, t.hover, 150);
            }
            let thumb = rect(r.x + c.px(12), r.y + c.px(10), c.px(76), r.h - c.px(20));
            if let Some(h) = self.hist.get(&e.id) {
                let (series, max) = self.thumb_series(&e.id, h);
                graph(c, thumb, &series, max, e.color, false);
            } else {
                graph(c, thumb, &[], 100.0, e.color, false);
            }
            let tx = thumb.r() + c.px(12);
            let tw = r.r() - tx - c.px(8);
            c.text(&e.title, Font::Semi, rect(tx, r.y + c.px(8), tw, c.px(22)), t.text, Align::Left);
            c.text(&e.sub, Font::Small, rect(tx, r.y + c.px(30), tw, c.px(18)), t.text_dim, Align::Left);
            if !e.value.is_empty() {
                c.text(&e.value, Font::Small, rect(tx, r.y + c.px(47), tw, c.px(18)), t.text_dim, Align::Left);
            }
        }
        c.reset_clip();
        c.fill(rect(list.r() + c.px(8), area.y + c.px(12), 1, area.h - c.px(24)), t.border);
    }

    fn thumb_series<'a>(&self, id: &str, h: &'a Hist) -> (Vec<ThumbLine<'a>>, f32) {
        let t = THEME;
        if id.starts_with("net:") {
            let max = nice_max(h.a.iter().chain(&h.b).copied().fold(0.0, f32::max));
            (vec![(&h.b[..], t.chart_net, true), (&h.a[..], dim(t.chart_net), false)], max)
        } else {
            let color = match id {
                "cpu" => t.chart_cpu,
                "mem" => t.chart_mem,
                _ if id.starts_with("disk:") => t.chart_disk,
                _ => t.chart_gpu,
            };
            (vec![(&h.a[..], color, true)], 100.0)
        }
    }

    fn paint_detail(&mut self, c: &mut Canvas, area: Rect, settings: &Settings, entries: &[Entry]) {
        let t = THEME;
        let left = self.list_rect.r() + c.px(28);
        let r = rect(left, area.y + c.px(12), area.r() - left - c.px(24), area.h - c.px(24));
        if r.w < c.px(200) {
            return;
        }
        let Some(entry) = entries.iter().find(|e| e.id == self.selected) else { return };
        let s = self.snap.sys.clone();
        let p = &tr().perf;
        let id = self.selected.clone();

        // Heading and model name.
        c.text(&entry.title, Font::Display, rect(r.x, r.y, r.w / 2, c.px(44)), t.text, Align::Left);
        let model = match id.as_str() {
            "cpu" => s.info.cpu_name.clone(),
            "mem" => {
                let total = if s.info.installed_mem > 0 { s.info.installed_mem } else { s.mem.total };
                format!("{:.1} GB {}", total as f64 / 1073741824.0, s.info.mem_type).trim().to_string()
            }
            _ if id.starts_with("disk:") => s.disks.iter().find(|d| format!("disk:{}", d.index) == id).map_or(String::new(), |d| d.info.model.clone()),
            _ if id.starts_with("net:") => s.nets.iter().find(|n| format!("net:{}", n.index) == id).map_or(String::new(), |n| n.description.clone()),
            _ => entry.sub.clone(),
        };
        self.toggle_rects = [Rect::default(); 2];
        if id == "cpu" {
            // Segmented switch between the overall graph and one graph per logical processor.
            let labels = [tr().perf.overall, tr().perf.logical];
            let bh = c.px(30);
            let w1 = c.measure(labels[1], Font::Small) + c.px(24);
            let w0 = c.measure(labels[0], Font::Small) + c.px(24);
            let seg = rect(r.r() - w0 - w1 - c.px(6), r.y + c.px(48), w0 + w1 + c.px(6), bh + c.px(6));
            c.round(seg, c.px(6) as f32, t.window, 255);
            self.toggle_rects = [
                rect(seg.x + c.px(3), seg.y + c.px(3), w0, bh),
                rect(seg.x + c.px(3) + w0, seg.y + c.px(3), w1, bh),
            ];
            for (i, label) in labels.iter().enumerate() {
                let on = (i == 1) == self.cpu_logical;
                let b = self.toggle_rects[i];
                if on {
                    c.round(b, c.px(4) as f32, t.accent, 255);
                } else if self.hover == Hover::Toggle(i) {
                    c.round(b, c.px(4) as f32, t.hover, 255);
                }
                c.text(label, Font::Small, b, if on { t.on_accent } else { t.text }, Align::Center);
            }
        }
        c.text(&model, Font::Value, rect(r.r() - r.w / 2, r.y, r.w / 2, c.px(44)), t.text, Align::Right);

        let stats_h = c.px(if id == "cpu" || id == "mem" { 210 } else { 170 });
        let top = r.y + c.px(88);
        let graph_bottom = (r.b() - stats_h).max(top + c.px(140));
        let gr = rect(r.x, top, r.w, graph_bottom - top);
        let time = Self::time_label(settings);
        let Some(h) = self.hist.get(&id) else { return };

        let stats_y = gr.b() + c.px(12);
        match id.as_str() {
            "cpu" => {
                let gr = labeled(c, gr, tr().perf.utilization_pct, "100%", &time, "0");
                if self.cpu_logical && !h.cores.is_empty() {
                    let n = h.cores.len() as i32;
                    let aspect = gr.w as f32 / gr.h.max(1) as f32;
                    let cols = ((n as f32 * aspect / 1.6).sqrt().ceil() as i32).clamp(1, n);
                    let rows = (n + cols - 1) / cols;
                    let gap = c.px(4);
                    let cw = (gr.w - gap * (cols - 1)) / cols;
                    let ch = (gr.h - gap * (rows - 1)) / rows.max(1);
                    for (i, core) in h.cores.iter().enumerate() {
                        let (cx, cy) = (i as i32 % cols, i as i32 / cols);
                        let cell = rect(gr.x + cx * (cw + gap), gr.y + cy * (ch + gap), cw, ch);
                        graph(c, cell, &[(&core[..], t.chart_cpu, true)], 100.0, t.chart_cpu, false);
                    }
                } else {
                    graph(c, gr, &[(&h.a[..], t.chart_cpu, true), (&h.b[..], dim(t.chart_cpu), false)], 100.0, t.chart_cpu, true);
                }
                let cpu = &s.cpu;
                let speed = if cpu.mhz > 0.0 { format!("{:.2} GHz", cpu.mhz / 1000.0) } else { "—".into() };
                let big = [
                    (p.utilization, format!("{:.0}%", cpu.total)),
                    (p.speed, speed),
                    (tr().common.processes, cpu.processes.to_string()),
                    (tr().common.threads, cpu.threads.to_string()),
                    (tr().common.handles, cpu.handles.to_string()),
                    (p.uptime, uptime(cpu.uptime_ms)),
                ];
                let info = &s.info;
                let base = if info.base_mhz > 0 { format!("{:.2} GHz", info.base_mhz as f32 / 1000.0) } else { "—".into() };
                let kv = [
                    (p.base_speed, base),
                    (p.sockets, info.sockets.to_string()),
                    (p.cores, info.cores.to_string()),
                    (p.logical_count, info.logical.to_string()),
                    (p.l1, cache(info.l1)),
                    (p.l2, cache(info.l2)),
                    (p.l3, cache(info.l3)),
                ];
                stats(c, rect(r.x, stats_y, r.w, stats_h - c.px(12)), &big, 3, &kv);
            }
            "mem" => {
                let m = &s.mem;
                let total_gb = m.total as f64 / 1073741824.0;
                let bar_h = c.px(52);
                let g = rect(gr.x, gr.y, gr.w, gr.h - bar_h - c.px(30));
                let g = labeled(c, g, tr().perf.mem_usage, &format!("{total_gb:.1} GB"), &time, "0");
                graph(c, g, &[(&h.a[..], t.chart_mem, true)], 100.0, t.chart_mem, true);
                let label_y = g.b() + c.px(22);
                c.text(tr().perf.mem_composition, Font::Small, rect(gr.x, label_y, gr.w, c.px(20)), t.text_dim, Align::Left);
                let bar = rect(gr.x, label_y + c.px(22), gr.w, bar_h - c.px(6));
                composition(c, bar, m);
                let used = m.total.saturating_sub(m.available);
                let big = [
                    (p.in_use_compressed, format!("{} ({})", gb1(used), gb1(m.compressed))),
                    (p.available, gb1(m.available)),
                    (p.committed, format!("{:.0}/{:.0} GB", m.commit as f64 / 1073741824.0, m.commit_limit as f64 / 1073741824.0)),
                    (p.cached, gb1(m.cached)),
                    (p.paged_pool, bytes(m.paged_pool)),
                    (p.nonpaged_pool, bytes(m.nonpaged_pool)),
                ];
                let info = &s.info;
                let reserved = if info.installed_mem > m.total { bytes(info.installed_mem - m.total) } else { "—".into() };
                let kv = [
                    (p.speed_colon, if info.mem_speed > 0 { (tr().units.mts)(info.mem_speed) } else { "—".into() }),
                    (p.slots, if info.slots_total > 0 { format!("{}/{}", info.slots_used, info.slots_total) } else { "—".into() }),
                    (p.form_factor, if info.form_factor.is_empty() { "—".into() } else { info.form_factor.clone() }),
                    (p.hw_reserved, reserved),
                ];
                stats(c, rect(r.x, stats_y, r.w, stats_h - c.px(12)), &big, 2, &kv);
            }
            _ if id.starts_with("disk:") => {
                let Some(d) = s.disks.iter().find(|d| format!("disk:{}", d.index) == id) else { return };
                let half = (gr.h - c.px(30)) / 2;
                let g1 = labeled(c, rect(gr.x, gr.y, gr.w, half), tr().perf.active_time, "100%", &time, "0");
                graph(c, g1, &[(&h.a[..], t.chart_disk, true)], 100.0, t.chart_disk, true);
                let max = nice_max(h.b.iter().chain(&h.c).copied().fold(0.0, f32::max));
                let g2 = labeled(c, rect(gr.x, gr.y + half + c.px(30), gr.w, half), tr().perf.transfer_rate, &bytes_per_sec(max as f64), &time, "0");
                graph(c, g2, &[(&h.b[..], t.chart_disk, true), (&h.c[..], dim(t.chart_disk), false)], max, t.chart_disk, true);
                let big = [
                    (p.active_time, format!("{:.0}%", d.active)),
                    (p.avg_response, (tr().units.millis)(d.response_ms)),
                    (p.read_speed, bytes_per_sec(d.read_bps)),
                    (p.write_speed, bytes_per_sec(d.write_bps)),
                ];
                let kv = [
                    (p.capacity, if d.info.capacity > 0 { bytes(d.info.capacity) } else { "—".into() }),
                    (p.type_colon, d.info.kind.clone()),
                    (p.drive, if d.letters.is_empty() { "—".into() } else { d.letters.clone() }),
                ];
                stats(c, rect(r.x, stats_y, r.w, stats_h - c.px(12)), &big, 2, &kv);
                legend(c, rect(g2.x, g2.y - c.px(22), g2.w * 2 / 3, c.px(22)), &[(tr().perf.read, t.chart_disk), (tr().perf.write, dim(t.chart_disk))]);
            }
            _ if id.starts_with("net:") => {
                let Some(n) = s.nets.iter().find(|n| format!("net:{}", n.index) == id) else { return };
                let max = nice_max(h.a.iter().chain(&h.b).copied().fold(0.0, f32::max));
                let g = labeled(c, gr, tr().perf.throughput, &bits_per_sec(max as f64), &time, "0");
                graph(c, g, &[(&h.b[..], t.chart_net, true), (&h.a[..], dim(t.chart_net), false)], max, t.chart_net, true);
                let big = [(tr().perf.send, bits_per_sec(n.send_bps)), (tr().perf.receive, bits_per_sec(n.recv_bps))];
                let dash = |s: &str| if s.is_empty() { "—".to_string() } else { s.to_string() };
                let kv = [
                    (p.adapter, dash(&n.alias)),
                    (p.conn_type, n.kind.to_string()),
                    (p.link_speed, if n.link_bps > 0 { bits_per_sec(n.link_bps as f64 / 8.0) } else { "—".into() }),
                    (p.ipv4, dash(&n.ipv4)),
                    (p.ipv6, dash(&n.ipv6)),
                ];
                stats(c, rect(r.x, stats_y, r.w, stats_h - c.px(12)), &big, 2, &kv);
                legend(c, rect(g.x, g.y - c.px(22), g.w * 2 / 3, c.px(22)), &[(tr().perf.receive, t.chart_net), (tr().perf.send, dim(t.chart_net))]);
            }
            _ => {
                let live = s.gpus.iter().find(|g| format!("gpu:{}", g.luid) == id);
                let mem_h = (gr.h / 4).max(c.px(70));
                let eng_area = rect(gr.x, gr.y, gr.w, gr.h - mem_h - c.px(30));
                let engines: Vec<&(String, Vec<f32>)> = h.engines.iter().take(4).collect();
                let n = engines.len().max(1) as i32;
                let cols = if n > 2 { 2 } else { n };
                let rows = (n + cols - 1) / cols;
                let gap = c.px(16);
                let cw = (eng_area.w - gap * (cols - 1)) / cols;
                let ch = (eng_area.h - c.px(22) * rows - gap * (rows - 1)) / rows;
                for (i, (name, series)) in engines.iter().enumerate() {
                    let (cx, cy) = (i as i32 % cols, i as i32 / cols);
                    let x = eng_area.x + cx * (cw + gap);
                    let y = eng_area.y + cy * (ch + c.px(22) + gap);
                    let v = series.last().copied().unwrap_or(0.0);
                    c.text(name, Font::Small, rect(x, y, cw / 2, c.px(20)), t.text_dim, Align::Left);
                    c.text(&format!("{v:.0}%"), Font::Small, rect(x + cw / 2, y, cw / 2, c.px(20)), t.text_dim, Align::Right);
                    graph(c, rect(x, y + c.px(22), cw, ch), &[(&series[..], t.chart_gpu, true)], 100.0, t.chart_gpu, true);
                }
                if engines.is_empty() {
                    c.text(tr().perf.gpu_loading, Font::Body, eng_area, t.text_dim, Align::Center);
                }
                let mem_y = eng_area.b() + c.px(30);
                let half = (gr.w - gap) / 2;
                let (ded_total, sh_total) = live.map_or((0, 0), |g| (g.dedicated_total, g.shared_total));
                let ded_max = if ded_total > 0 { ded_total as f32 } else { nice_max(h.b.iter().copied().fold(0.0, f32::max)) };
                let sh_max = if sh_total > 0 { sh_total as f32 } else { nice_max(h.c.iter().copied().fold(0.0, f32::max)) };
                let g1 = labeled(c, rect(gr.x, mem_y - c.px(22), half, mem_h + c.px(22)), tr().perf.ded_usage, &bytes(ded_max as u64), "", "");
                graph(c, g1, &[(&h.b[..], t.chart_gpu, true)], ded_max, t.chart_gpu, false);
                let g2 = labeled(c, rect(gr.x + half + gap, mem_y - c.px(22), half, mem_h + c.px(22)), tr().perf.shared_usage, &bytes(sh_max as u64), "", "");
                graph(c, g2, &[(&h.c[..], t.chart_gpu, true)], sh_max, t.chart_gpu, false);
                if let Some(g) = live {
                    let pair = |used: u64, total: u64| {
                        if total > 0 {
                            format!("{:.1}/{:.1} GB", used as f64 / 1073741824.0, total as f64 / 1073741824.0)
                        } else {
                            gb1(used)
                        }
                    };
                    let big = [
                        (p.utilization, format!("{:.0}%", g.util)),
                        (tr().common.dedicated_gpu_mem, pair(g.dedicated, g.dedicated_total)),
                        (tr().common.shared_gpu_mem, pair(g.shared, g.shared_total)),
                        (tr().common.gpu_mem, pair(g.dedicated + g.shared, g.dedicated_total + g.shared_total)),
                    ];
                    stats(c, rect(r.x, stats_y, r.w, stats_h - c.px(12)), &big, 2, &[]);
                } else {
                    c.text(tr().perf.gpu_preparing, Font::Small, rect(r.x, stats_y, r.w, c.px(24)), t.text_dim, Align::Left);
                }
            }
        }
    }

    fn update_hover(&mut self, cx: &mut Cx) {
        let (x, y) = self.mouse;
        let mut h = Hover::None;
        if self.list_rect.contains(x, y) {
            if let Some(i) = self.entry_rects.iter().position(|r| r.contains(x, y)) {
                h = Hover::Entry(i);
            }
        } else if let Some(i) = self.toggle_rects.iter().position(|r| r.contains(x, y)) {
            h = Hover::Toggle(i);
        }
        if h != self.hover {
            self.hover = h;
            *cx.dirty = true;
        }
    }
}

// ---------------------------------------------------------------- drawing helpers

fn dim(color: u32) -> u32 {
    let ch = |s: u32| ((((color >> s) & 255) * 3 / 5) & 255) << s;
    ch(16) | ch(8) | ch(0)
}

fn gb1(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1073741824.0)
}

fn cache(b: u64) -> String {
    if b == 0 {
        "—".into()
    } else {
        bytes(b)
    }
}

fn short_rate(bytes_per_sec: f64) -> String {
    let k = bytes_per_sec * 8.0 / 1000.0;
    if k < 1000.0 {
        format!("{k:.0} Kbps")
    } else {
        format!("{:.1} Mbps", k / 1000.0)
    }
}

/// Rounds up to 1, 2 or 5 times a power of ten.
fn nice_max(v: f32) -> f32 {
    if v <= 0.0 {
        return 1024.0;
    }
    let p = 10f32.powf(v.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if v <= m * p {
            return m * p;
        }
    }
    10.0 * p
}

/// Draws the caption row above a graph and the axis row under it; returns the graph rect.
fn labeled(c: &mut Canvas, r: Rect, title: &str, top_right: &str, bottom_left: &str, bottom_right: &str) -> Rect {
    let t = THEME;
    let lh = c.px(22);
    c.text(title, Font::Small, rect(r.x, r.y, r.w * 2 / 3, lh), t.text_dim, Align::Left);
    c.text(top_right, Font::Small, rect(r.x + r.w / 3, r.y, r.w * 2 / 3, lh), t.text_dim, Align::Right);
    let has_axis = !bottom_left.is_empty() || !bottom_right.is_empty();
    let g = rect(r.x, r.y + lh, r.w, r.h - lh - if has_axis { lh } else { 0 });
    if has_axis {
        c.text(bottom_left, Font::Small, rect(r.x, g.b(), r.w / 2, lh), t.text_dim, Align::Left);
        c.text(bottom_right, Font::Small, rect(r.x + r.w / 2, g.b(), r.w / 2, lh), t.text_dim, Align::Right);
    }
    g
}

/// A line graph with the newest value at the right edge. The first series is filled.
fn graph(c: &mut Canvas, r: Rect, series: &[(&[f32], u32, bool)], max: f32, frame: u32, grid: bool) {
    let t = THEME;
    if r.w <= 2 || r.h <= 2 {
        return;
    }
    c.fill(r, t.window);
    if grid {
        for i in 1..10 {
            c.fill(rect(r.x, r.y + r.h * i / 10, r.w, 1), t.chart_grid);
        }
        for i in 1..20 {
            c.fill(rect(r.x + r.w * i / 20, r.y, 1, r.h), t.chart_grid);
        }
    }
    let inner = r.inset(1, 1);
    let step = inner.w as f32 / (HISTORY - 1) as f32;
    let lw = c.px(1).max(1);
    let max = max.max(f32::EPSILON);
    for (values, color, fill) in series {
        let n = values.len();
        if n < 2 {
            continue;
        }
        let x0 = inner.r() as f32 - (n - 1) as f32 * step;
        let y_at = |v: f32| inner.b() as f32 - (v / max).clamp(0.0, 1.0) * inner.h as f32;
        let mut prev: Option<i32> = None;
        for px in inner.x.max(x0.floor() as i32)..inner.r() {
            let pos = (px as f32 - x0) / step;
            if pos < 0.0 {
                continue;
            }
            let i = (pos as usize).min(n - 1);
            let f = pos - i as f32;
            let v = if i + 1 < n { values[i] * (1.0 - f) + values[i + 1] * f } else { values[i] };
            let y = y_at(v).round() as i32;
            if *fill {
                c.fill_alpha(rect(px, y, 1, inner.b() - y), *color, 48);
            }
            let (a, b) = match prev {
                Some(p) => (p.min(y), p.max(y)),
                None => (y, y),
            };
            c.fill(rect(px, a - lw / 2, 1, (b - a + lw).max(lw)).intersect(&inner), *color);
            prev = Some(y);
        }
    }
    c.stroke(r, 0.0, 1.0, frame, 140);
}

fn composition(c: &mut Canvas, bar: Rect, m: &crate::system::model::MemSample) {
    let t = THEME;
    let total = m.total.max(1) as f64;
    let free = if m.free > 0 { m.free } else { m.available.saturating_sub(m.standby) };
    let in_use = m.total.saturating_sub(m.available).saturating_sub(m.modified);
    let parts = [
        (in_use, 110u8, tr().perf.in_use),
        (m.modified, 60, tr().perf.modified),
        (m.standby, 22, tr().perf.standby),
        (free, 0, tr().perf.free),
    ];
    c.fill(bar, t.window);
    let mut x = bar.x as f64;
    for (v, alpha, _) in parts {
        let w = v as f64 / total * bar.w as f64;
        let seg = rect(x.round() as i32, bar.y, (x + w).round() as i32 - x.round() as i32, bar.h);
        if seg.w > 0 {
            if alpha > 0 {
                c.fill_alpha(seg, t.chart_mem, alpha);
            }
            c.fill(rect(seg.r() - 1, bar.y, 1, bar.h), t.chart_mem);
        }
        x += w;
    }
    c.stroke(bar, 0.0, 1.0, t.chart_mem, 255);
}

fn legend(c: &mut Canvas, r: Rect, items: &[(&str, u32)]) {
    let t = THEME;
    let mut x = r.r();
    for (label, color) in items.iter().rev() {
        let w = c.measure(label, Font::Small);
        x -= w;
        c.text(label, Font::Small, rect(x, r.y, w, r.h), t.text_dim, Align::Left);
        x -= c.px(16);
        c.fill(rect(x, r.y + r.h / 2 - 1, c.px(12), c.px(3)), *color);
        x -= c.px(16);
    }
}

/// Large figures in a grid on the left, small key/value pairs on the right.
fn stats(c: &mut Canvas, r: Rect, big: &[(&str, String)], per_row: usize, kv: &[(&str, String)]) {
    let t = THEME;
    let kv_w = if kv.is_empty() { 0 } else { c.px(330).min(r.w / 2) };
    let big_w = r.w - kv_w;
    let cell_w = (big_w / per_row.max(1) as i32).min(c.px(190));
    let row_h = c.px(60);
    for (i, (label, value)) in big.iter().enumerate() {
        let (col, row) = ((i % per_row) as i32, (i / per_row) as i32);
        let x = r.x + col * cell_w;
        let y = r.y + c.px(6) + row * row_h;
        if y + row_h > r.b() + c.px(10) {
            break;
        }
        c.text(label, Font::Small, rect(x, y, cell_w - c.px(8), c.px(20)), t.text_dim, Align::Left);
        c.text(value, Font::Value, rect(x, y + c.px(20), cell_w - c.px(8), c.px(32)), t.text, Align::Left);
    }
    if kv.is_empty() {
        return;
    }
    let kx = r.r() - kv_w;
    let lw = kv_w * 11 / 20;
    let lh = c.px(24);
    for (i, (k, v)) in kv.iter().enumerate() {
        let y = r.y + c.px(8) + i as i32 * lh;
        if y + lh > r.b() + c.px(10) {
            break;
        }
        c.text(k, Font::Small, rect(kx, y, lw, lh), t.text_dim, Align::Left);
        c.text(v, Font::Small, rect(kx + lw, y, kv_w - lw, lh), t.text, Align::Left);
    }
}

impl Page for PerformancePage {
    fn nav(&self) -> Nav {
        Nav {
            icon: icons::CHART,
            label: tr().perf.title,
            bottom: false,
        }
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        if snap.sys.elapsed > 0.0 {
            self.record(&snap.sys);
        }
        self.snap = snap.clone();
        *cx.dirty = true;
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        self.list_rect.contains(x, y)
    }

    /// The graphs keep history wherever the user is, minimized too, so the
    /// machine-wide values (GPU included) are needed at all times.
    fn demand(&self, _settings: &Settings) -> crate::system::demand::Needs {
        use crate::system::demand::need;
        crate::system::demand::Needs { shown: 0, always: need::SYS | need::SYS_GPU }
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        match *ev {
            Ev::Move { x, y } => {
                self.mouse = (x, y);
                self.update_hover(cx);
            }
            Ev::Leave => {
                self.mouse = (-1, -1);
                self.update_hover(cx);
            }
            Ev::Down { x, y, right: false } => {
                self.mouse = (x, y);
                self.update_hover(cx);
                match self.hover {
                    Hover::Entry(i) => {
                        if let Some(e) = self.entries().get(i) {
                            self.selected = e.id.clone();
                            *cx.dirty = true;
                        }
                    }
                    Hover::Toggle(i) => {
                        self.cpu_logical = i == 1;
                        *cx.dirty = true;
                    }
                    Hover::None => {}
                }
            }
            Ev::Pan { dy, .. } => {
                self.list_scroll -= dy as i32;
                *cx.dirty = true;
            }
            Ev::Wheel { dy, .. } => {
                if self.list_rect.contains(self.mouse.0, self.mouse.1) {
                    let step = self.entry_rects.first().map_or(60, |r| r.h);
                    self.list_scroll -= (dy * step as f32) as i32;
                    *cx.dirty = true;
                }
            }
            Ev::Key { vk, .. } => {
                let entries = self.entries();
                let cur = entries.iter().position(|e| e.id == self.selected).unwrap_or(0);
                let next = match vk {
                    VK_UP => cur.saturating_sub(1),
                    VK_DOWN => (cur + 1).min(entries.len().saturating_sub(1)),
                    VK_F5 => {
                        cx.shared.poke();
                        return;
                    }
                    _ => return,
                };
                if let Some(e) = entries.get(next) {
                    self.selected = e.id.clone();
                    *cx.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, settings: &Settings) {
        let entries = self.entries();
        if !entries.iter().any(|e| e.id == self.selected) {
            self.selected = "cpu".into();
        }
        self.paint_list(c, area, &entries);
        self.paint_detail(c, area, settings, &entries);
    }
}
