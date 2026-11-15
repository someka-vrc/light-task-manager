//! Painting of the toolbar, the graph with its tooltip, and the dialogs.

use super::*;

/// Plots one series over the view, a column of pixels at a time. Where a pixel covers
/// several samples it shows their maximum, so brief spikes stay visible when zoomed out.
#[allow(clippy::too_many_arguments)]
pub(super) fn plot_series(c: &mut Canvas, inner: Rect, data: &[Arc<Sample>], start: i64, end: i64, f: &SeriesFn, color: u32, max_gap: i64) {
    if inner.w <= 1 || data.len() < 2 {
        return;
    }
    let lw = c.px(1).max(1) + 1;
    let span = (end - start).max(1) as f64;
    let per_px = span / inner.w as f64;
    let y_at = |v: f32| inner.b() as f32 - (v / 100.0).clamp(0.0, 1.0) * (inner.h - 1) as f32;
    let mut prev: Option<i32> = None;
    for px in inner.x..inner.r() {
        let t0 = start + ((px - inner.x) as f64 * per_px) as i64;
        let t1 = start + ((px - inner.x + 1) as f64 * per_px) as i64;
        let j0 = data.partition_point(|s| s.time < t0);
        let j1 = data.partition_point(|s| s.time < t1);
        let v = if j1 > j0 {
            data[j0..j1].iter().filter_map(|s| f(s)).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
        } else if j0 > 0 && j0 < data.len() && data[j0].time - data[j0 - 1].time <= max_gap {
            let (a, b) = (&data[j0 - 1], &data[j0]);
            match (f(a), f(b)) {
                (Some(va), Some(vb)) => {
                    let k = (t0 - a.time) as f32 / (b.time - a.time).max(1) as f32;
                    Some(va + (vb - va) * k)
                }
                _ => None,
            }
        } else {
            None
        };
        // A gap in the samples (no recording) breaks the line.
        if j1 > j0 && j0 > 0 && data[j0].time - data[j0 - 1].time > max_gap {
            prev = None;
        }
        let Some(v) = v else {
            prev = None;
            continue;
        };
        let y = y_at(v).round() as i32;
        let (a, b) = match prev {
            Some(p) => (p.min(y), p.max(y)),
            None => (y, y),
        };
        c.fill(rect(px, a - lw / 2, 1, (b - a + lw).max(lw)).intersect(&inner), color);
        prev = Some(y);
    }
}

impl TimeMachinePage {
    pub(super) fn paint_toolbar(&mut self, c: &mut Canvas, bar: Rect, s: &Settings) {
        let t = THEME;
        let x = bar.x + c.px(20);
        let title = tr().tm.title;
        let on = s.tm_enabled;
        let paused = self.frozen.is_some();

        // In a narrow window the buttons show only their icons, and then the title goes.
        let spec = btn(icons::SETTINGS, tr().common.settings, true);
        let resume_spec = btn(icons::PLAY, tr().tm.resume, true);
        // Export needs a selected time or range.
        let export_spec = btn(icons::SAVE, tr().common.export, self.sel.is_some());
        let label = if on { tr().common.on } else { tr().common.off };
        let lw = c.measure(label, Font::Body);
        let full_tw = c.measure(title, Font::Title);
        let switch_w = c.px(40) + c.px(8) + lw;
        let width = |tw: i32, compact: bool, c: &mut Canvas| -> i32 {
            let button = |sp: &ButtonSpec, c: &mut Canvas| if compact { c.px(40) } else { c.measure(sp.label, Font::Body) + c.px(48) };
            let resume = if paused { button(&resume_spec, c) + button(&export_spec, c) + c.px(32) } else { c.px(16) };
            c.px(20) + tw + c.px(24) + switch_w + c.px(16) + button(&spec, c) + resume
        };
        let compact = width(full_tw, false, c) > bar.w;
        let show_title = width(full_tw, true, c) <= bar.w;
        let tw = if show_title { full_tw } else { -c.px(24) };
        if show_title {
            c.text(title, Font::Title, rect(x, bar.y, tw + c.px(4), c.px(TOOLBAR_H)), t.text, Align::Left);
        }

        // On/off switch.
        let sw = rect(x + tw + c.px(24), bar.y + (c.px(TOOLBAR_H) - c.px(20)) / 2, c.px(40), c.px(20));
        let track = if on { t.accent } else { t.window };
        c.round(sw, sw.h as f32 / 2.0, track, 255);
        c.stroke(sw, sw.h as f32 / 2.0, 1.0, if on { t.accent } else if self.hover == Hover::Switch { t.text_dim } else { t.text_faint }, 255);
        let knob = c.px(12);
        let kx = if on { sw.r() - c.px(4) - knob } else { sw.x + c.px(4) };
        c.round(rect(kx, sw.y + (sw.h - knob) / 2, knob, knob), knob as f32 / 2.0, if on { t.on_accent } else { t.text_dim }, 255);
        c.text(label, Font::Body, rect(sw.r() + c.px(8), bar.y, lw + c.px(4), c.px(TOOLBAR_H)), t.text, Align::Left);
        self.switch_rect = rect(sw.x - c.px(4), sw.y - c.px(6), sw.w + lw + c.px(16), sw.h + c.px(12));

        // Settings button.
        let bw = if compact { c.px(40) } else { c.measure(spec.label, Font::Body) + c.px(48) };
        let bh = c.px(34);
        let br = rect(self.switch_rect.r() + c.px(12), bar.y + (c.px(TOOLBAR_H) - bh) / 2, bw, bh);
        self.settings_rect = br;
        paint_button(c, br, &spec, self.hover == Hover::Settings, compact);

        // Resume button on the right while paused.
        // Export and resume on the right while paused.
        self.export_rect = Rect::default();
        if paused {
            let width = |sp: &ButtonSpec, c: &mut Canvas| if compact { c.px(40) } else { c.measure(sp.label, Font::Body) + c.px(48) };
            let rw = width(&resume_spec, c);
            let r = rect(bar.r() - c.px(16) - rw, br.y, rw, bh);
            self.resume.rects = vec![r];
            paint_button(c, r, &resume_spec, self.resume.hover == Some(0), compact);
            let ew = width(&export_spec, c);
            let e = rect(r.x - c.px(8) - ew, br.y, ew, bh);
            self.export_rect = e;
            paint_button(c, e, &export_spec, self.hover == Hover::Export, compact);
        } else {
            self.resume.rects.clear();
        }
    }

    /// GPU adapters with values in the shown samples, with their number (the rank of the
    /// LUID, as on the performance page). Adapters that are gone drop out.
    pub(super) fn shown_adapters(&self) -> Vec<(usize, usize)> {
        let data = self.data();
        let mut ids: Vec<&str> = self.adapters.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        let mut out: Vec<(usize, usize)> = (0..self.adapters.len())
            .filter(|i| data.iter().any(|s| s.gpus.get(*i).is_some_and(|v| !v.is_nan())))
            .map(|i| (i, ids.iter().position(|id| *id == self.adapters[i].id).unwrap_or(i)))
            .collect();
        out.sort_by_key(|(_, rank)| *rank);
        out
    }

    /// The lines of the graph: label, color, value.
    pub(super) fn series(&self, ts: &[Target]) -> Vec<(String, u32, Box<SeriesFn>)> {
        let t = THEME;
        let adapters = self.shown_adapters();
        let mut out: Vec<(String, u32, Box<SeriesFn>)> = Vec::new();
        for tg in ts {
            match tg {
                Target::Cpu => out.push(("CPU".into(), t.chart_cpu, Box::new(|x: &Sample| Some(x.cpu)))),
                Target::CpuDetail => out.push((tr().tm.cpu_kernel.into(), dim(t.chart_cpu), Box::new(|x: &Sample| Some(x.kernel)))),
                Target::Mem => out.push((tr().common.memory.into(), t.chart_mem, Box::new(|x: &Sample| Some(x.mem_pct)))),
                Target::Commit => out.push((
                    tr().common.commit_mem.into(),
                    commit_color(),
                    Box::new(|x: &Sample| (x.commit_limit > 0).then(|| x.commit as f32 / x.commit_limit as f32 * 100.0)),
                )),
                Target::Disk => out.push((
                    tr().tm.disk_busiest.into(),
                    t.chart_disk,
                    Box::new(|x: &Sample| x.disks.iter().copied().filter(|v| !v.is_nan()).reduce(f32::max)),
                )),
                Target::Gpu => {
                    for &(i, rank) in &adapters {
                        out.push((format!("GPU {rank}"), gpu_color(rank), Box::new(move |x: &Sample| x.gpus.get(i).copied().filter(|v| !v.is_nan()))));
                    }
                }
                Target::GpuMem => {
                    for &(i, rank) in &adapters {
                        let total = self.adapters[i].total;
                        if total > 0 {
                            out.push((
                                (tr().tm.gpu_n_mem)(rank),
                                dim(gpu_color(rank)),
                                Box::new(move |x: &Sample| {
                                    let present = x.gpus.get(i).is_some_and(|v| !v.is_nan());
                                    present.then(|| x.gpu_mem.get(i).copied().unwrap_or(0) as f32 / total as f32 * 100.0)
                                }),
                            ));
                        }
                    }
                }
                Target::Handles | Target::Faults | Target::Hung => {}
            }
        }
        out
    }

    /// Lays the legend out in rows; returns each item's position and the rows used.
    pub(super) fn legend_layout(c: &mut Canvas, items: &[(String, u32, Box<SeriesFn>)], width: i32) -> (Vec<(i32, i32)>, i32) {
        let (mut x, mut row) = (0, 0);
        let mut out = Vec::with_capacity(items.len());
        for (label, _, _) in items {
            let w = c.px(18) + c.measure(label, Font::Small) + c.px(16);
            if x > 0 && x + w > width {
                x = 0;
                row += 1;
            }
            out.push((x, row));
            x += w;
        }
        (out, row + 1)
    }

    pub(super) fn paint_graph(&mut self, c: &mut Canvas, block: Rect, s: &Settings) {
        let t = THEME;
        let lh = c.px(22);
        let ts = targets(s);
        let series = self.series(&ts);
        // The legend wraps, and the graph gives up the height it takes.
        let hundred = c.measure("100%", Font::Small) + c.px(12);
        let (places, rows) = Self::legend_layout(c, &series, block.w - hundred);
        let head = rect(block.x, block.y, block.w, lh * rows);
        let plot = rect(block.x, head.b(), block.w, block.h - head.h - lh);
        let axis = rect(block.x, plot.b(), block.w, lh);
        self.plot = plot;
        let data = self.data();

        for ((label, color, _), (x, row)) in series.iter().zip(&places) {
            let (x, y) = (head.x + x, head.y + row * lh);
            c.fill(rect(x, y + lh / 2 - 1, c.px(12), c.px(3)), *color);
            let w = c.measure(label, Font::Small);
            c.text(label, Font::Small, rect(x + c.px(18), y, w + c.px(2), lh), t.text_dim, Align::Left);
        }
        c.text("100%", Font::Small, rect(head.x, head.y, head.w, lh), t.text_dim, Align::Right);

        // Frame and grid.
        c.fill(plot, t.window);
        for i in 1..10 {
            c.fill(rect(plot.x, plot.y + plot.h * i / 10, plot.w, 1), t.chart_grid);
        }
        for i in 1..20 {
            c.fill(rect(plot.x + plot.w * i / 20, plot.y, 1, plot.h), t.chart_grid);
        }
        let (start, end) = self.view(s);
        let inner = plot.inset(1, 1);
        let max_gap = (s.interval_ms.max(500) as i64 * 3 / 1000).max(3) * SEC;

        // Series.
        for (_, color, f) in &series {
            plot_series(c, inner, data, start, end, f.as_ref(), *color, max_gap);
        }

        // Selection.
        if let (Some((a, b)), Some(fd)) = (self.sel, &self.frozen) {
            let xa = self.x_at(fd[a].time, s);
            let xb = self.x_at(fd[b].time, s);
            // Cut to the plot by hand: resetting the clip would also drop the page's own.
            let cut = |r: Rect| r.intersect(&inner);
            if a == b {
                c.fill(cut(rect(xa - c.px(1), inner.y, c.px(2).max(2), inner.h)), t.accent);
            } else {
                c.fill_alpha(cut(rect(xa, inner.y, (xb - xa).max(1), inner.h)), t.accent, 46);
                c.fill(cut(rect(xa, inner.y, 1, inner.h)), t.accent);
                c.fill(cut(rect(xb, inner.y, 1, inner.h)), t.accent);
            }
        }
        c.stroke(plot, 0.0, 1.0, t.border, 255);

        // Axis: the view's edges, and the selection or the pause in the middle.
        c.text(&self.clock(start), Font::Small, rect(axis.x, axis.y, axis.w / 3, axis.h), t.text_dim, Align::Left);
        c.text(&self.clock(end), Font::Small, rect(axis.r() - axis.w / 3, axis.y, axis.w / 3, axis.h), t.text_dim, Align::Right);
        let middle = match (self.sel, &self.frozen) {
            (Some((a, b)), Some(fd)) if a == b => (tr().tm.selected_at)(&self.clock(fd[a].time)),
            (Some((a, b)), Some(fd)) => (tr().tm.selected_range)(
                &self.clock(fd[a].time),
                &self.clock(fd[b].time),
                &duration_text((fd[b].time - fd[a].time) / SEC),
            ),
            _ if self.frozen.is_some() => (tr().tm.view_range)(&duration_text((end - start) / SEC)),
            _ => String::new(),
        };
        c.text(&middle, Font::Small, rect(axis.x + axis.w / 3, axis.y, axis.w / 3, axis.h), t.text, Align::Center);

        if data.len() < 2 {
            let msg = if s.interval_ms == 0 { tr().tm.not_recording } else { tr().tm.started };
            c.text(msg, Font::Body, plot, t.text_dim, Align::Center);
        }
    }

    pub(super) fn paint_hover(&mut self, c: &mut Canvas, area: Rect, s: &Settings, ts: &[Target]) {
        let t = THEME;
        let (mx, my) = self.mouse;
        let inner = self.plot.inset(1, 1);
        if !inner.contains(mx, my) || self.dialog.is_some() || self.export.is_some() || self.menu.is_some() {
            return;
        }
        c.fill(rect(mx, inner.y, 1, inner.h), t.text_dim);
        let time = self.time_at(mx, s);
        let data = self.data();
        let Some(i) = Self::nearest(data, time) else { return };
        let sample = &data[i];
        // Each line: text, color, and the value the graph plots for it (if any).
        let mut lines: Vec<(String, u32, Option<f32>)> = vec![(self.clock(sample.time), t.text, None)];
        let adapters = self.shown_adapters();
        let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
        for tg in ts {
            match tg {
                Target::Cpu => lines.push((format!("CPU  {:.1}%", sample.cpu), t.chart_cpu, Some(sample.cpu))),
                Target::CpuDetail => {
                    let speed = if sample.mhz > 0.0 { format!("  {}  {:.2} GHz", tr().perf.speed, sample.mhz / 1000.0) } else { String::new() };
                    lines.push((format!("{}  {:.1}%{speed}", tr().tm.kernel, sample.kernel), dim(t.chart_cpu), Some(sample.kernel)));
                }
                Target::Mem => lines.push((
                    format!("{}  {} MB ({:.1}%)", tr().common.memory, group_thousands(sample.mem_used / 1024 / 1024), sample.mem_pct),
                    t.chart_mem,
                    Some(sample.mem_pct),
                )),
                Target::Commit => lines.push((
                    format!(
                        "{}  {} / {} MB ({:.1}%)",
                    tr().tm.commit,
                        group_thousands(sample.commit / 1024 / 1024),
                        group_thousands(sample.commit_limit / 1024 / 1024),
                        sample.commit as f64 / sample.commit_limit.max(1) as f64 * 100.0
                    ),
                    commit_color(),
                    (sample.commit_limit > 0).then(|| sample.commit as f32 / sample.commit_limit as f32 * 100.0),
                )),
                Target::Disk => {
                    // The graph plots the busiest disk.
                    let mut busiest = sample.disks.iter().copied().filter(|v| !v.is_nan()).reduce(f32::max);
                    for (k, v) in sample.disks.iter().enumerate() {
                        if !v.is_nan() {
                            let name = self.disks.get(k).map_or(tr().common.disk, |d| d.name.as_str());
                            let plotted = busiest.filter(|b| b == v);
                            if plotted.is_some() {
                                busiest = None;
                            }
                            lines.push((format!("{name}  {v:.0}%"), t.chart_disk, plotted));
                        }
                    }
                }
                Target::Gpu => {
                    for &(i, rank) in &adapters {
                        if let Some(v) = sample.gpus.get(i).filter(|v| !v.is_nan()) {
                            let name = &self.adapters[i].name;
                            let name = if name.is_empty() { String::new() } else { format!(" ({name})") };
                            lines.push((format!("GPU {rank}{name}  {v:.0}%"), gpu_color(rank), Some(*v)));
                        }
                    }
                }
                Target::GpuMem => {
                    for &(i, rank) in &adapters {
                        if sample.gpus.get(i).is_some_and(|v| !v.is_nan()) {
                            let used = sample.gpu_mem.get(i).copied().unwrap_or(0);
                            let total = self.adapters[i].total;
                            let text = if total > 0 {
                                format!("{}  {:.1} / {:.1} GB", (tr().tm.gpu_n_mem)(rank), gb(used), gb(total))
                            } else {
                                format!("{}  {:.1} GB", (tr().tm.gpu_n_mem)(rank), gb(used))
                            };
                            let plotted = (total > 0).then(|| used as f32 / total as f32 * 100.0);
                            lines.push((text, dim(gpu_color(rank)), plotted));
                        }
                    }
                }
                Target::Handles => lines.push((format!("{}  {}", tr().common.handles, group_thousands(sample.handles as u64)), t.text_dim, None)),
                Target::Faults => lines.push((format!("{}  {} {}", tr().common.page_faults, group_thousands(sample.faults as u64), tr().units.per_sec), t.text_dim, None)),
                Target::Hung => {
                    let n = sample.procs.iter().filter(|p| p.hung).count();
                    lines.push((format!("{}  {}", tr().common.not_responding, (tr().tm.n_processes)(n)), if n > 0 { DIED_COLOR } else { t.text_dim }, None));
                }
            }
        }
        // The line nearest the mouse at this sample, judged by height alone: across, the
        // next sample may well be nearer than this one.
        let y_at = |v: f32| (inner.b() as f32 - (v / 100.0).clamp(0.0, 1.0) * (inner.h - 1) as f32).round() as i32;
        let near = lines
            .iter()
            .enumerate()
            .filter_map(|(k, (_, _, v))| v.map(|v| (k, y_at(v))))
            .min_by_key(|(_, y)| (y - my).abs());
        if let Some((k, y)) = near {
            let x = self.x_at(sample.time, s);
            let d = c.px(9).max(5);
            let dot = rect(x - d / 2, y - d / 2, d, d);
            c.round(dot, d as f32 / 2.0, lines[k].1, 255);
            c.stroke(dot.inset(-1, -1), (d + 2) as f32 / 2.0, 1.0, t.window, 255);
        }
        let bold = near.map(|(k, _)| k);

        let lh = c.px(20);
        // Each line but the time leads with its color, and the text stays readable.
        let sw = c.px(16);
        let font = |k: usize| if Some(k) == bold { Font::SmallSemi } else { Font::Small };
        let w = lines.iter().enumerate().map(|(k, (l, _, _))| c.measure(l, font(k))).max().unwrap_or(0) + c.px(20) + sw;
        let h = lh * lines.len() as i32 + c.px(12);
        // Below the pointer, so the lines beside it stay in sight: to the lower left,
        // or to the lower right when there is no room on the left.
        let mut x = mx - c.px(4) - w;
        if x < inner.x + c.px(4) {
            x = mx + c.px(4);
        }
        x = x.max(inner.x + c.px(4)).min((inner.r() - w - c.px(4)).max(inner.x));
        // It may run over the table below the graph; above the pointer when the page ends first.
        let mut y = my + c.px(20);
        if y + h > area.b() - c.px(4) {
            y = my - c.px(8) - h;
        }
        let y = y.max(area.y + c.px(4));
        let r = rect(x, y, w, h);
        c.round(r, c.px(5) as f32, t.popup, 255);
        c.stroke(r, c.px(5) as f32, 1.0, t.border, 255);
        for (k, (line, color, _)) in lines.iter().enumerate() {
            let y = r.y + c.px(6) + lh * k as i32;
            if k > 0 && *color != t.text_dim {
                c.fill(rect(r.x + c.px(10), y + lh / 2 - 1, c.px(10), c.px(3)), *color);
            }
            let fg = if k == 0 || *color != t.text_dim { t.text } else { t.text_dim };
            c.text(line, font(k), rect(r.x + c.px(10) + sw, y, w - c.px(20) - sw, lh), fg, Align::Left);
            if Some(k) == bold {
                // Underlined too: bold alone is easy to miss.
                let uw = c.measure(line, font(k));
                c.fill(rect(r.x + c.px(10) + sw, y + lh / 2 + c.px(6), uw, c.px(1).max(1)), fg);
            }
        }
    }

    pub(super) fn paint_time_tooltip(&mut self, c: &mut Canvas, area: Rect) {
        if self.mode() != Mode::Range || self.table.header_hover() != Some(3) {
            return;
        }
        let Some(h) = self.table.header_rect(3) else { return };
        let t = THEME;
        let lines = [
            tr().tm.legend_born,
            tr().tm.legend_died,
            tr().tm.legend_note,
        ];
        let lh = c.px(20);
        let w = lines.iter().map(|l| c.measure(l, Font::Small)).max().unwrap_or(0) + c.px(20);
        let hgt = lh * lines.len() as i32 + c.px(12);
        let x = h.x.min(area.r() - w - c.px(8)).max(area.x);
        let r = rect(x, h.b() + c.px(4), w, hgt);
        c.round(r, c.px(5) as f32, t.popup, 250);
        c.stroke(r, c.px(5) as f32, 1.0, t.border, 255);
        for (k, line) in lines.iter().enumerate() {
            c.text(line, Font::Small, rect(r.x + c.px(10), r.y + c.px(6) + lh * k as i32, w - c.px(20), lh), t.text, Align::Left);
        }
    }

    pub(super) fn paint_dialog(&mut self, c: &mut Canvas, area: Rect, s: &Settings) {
        let typed = self.dialog.as_ref().and_then(|d| d.minutes()).unwrap_or(s.tm_minutes);
        let estimate = self.estimate(s, typed);
        let procs = self.last_procs;
        let Some(d) = &mut self.dialog else { return };
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(480).min(area.w - c.px(32));
        let h = c.px(364).min(area.h - c.px(16));
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
        d.panel = r;
        panel(c, r);
        let x = r.x + c.px(22);
        let iw = r.w - c.px(44);
        c.text(tr().tm.settings_title, Font::Semi, rect(x, r.y + c.px(18), iw, c.px(26)), t.text, Align::Left);

        c.text(tr().tm.targets, Font::Body, rect(x, r.y + c.px(52), iw, c.px(24)), t.text_dim, Align::Left);
        // Two columns of five.
        let col_w = iw / 2;
        for (i, tg) in DIALOG_ORDER.iter().enumerate() {
            let on = enabled(s, *tg);
            let row = rect(x + col_w * (i / 5) as i32, r.y + c.px(80) + c.px(30) * (i % 5) as i32, col_w - c.px(4), c.px(28));
            d.checks[i] = row;
            if d.hover == Some(10 + i as u8) {
                c.round(row, c.px(4) as f32, t.hover, 255);
            }
            let bx = rect(row.x + c.px(6), row.y + (row.h - c.px(18)) / 2, c.px(18), c.px(18));
            if on {
                c.round(bx, c.px(4) as f32, t.accent, 255);
                c.text(&glyph(icons::CHECK), Font::IconSmall, bx, t.on_accent, Align::Center);
            } else {
                c.stroke(bx, c.px(4) as f32, 1.0, t.text_dim, 255);
            }
            c.text(tg.label(), Font::Body, rect(bx.r() + c.px(10), row.y, row.r() - bx.r() - c.px(12), row.h), t.text, Align::Left);
        }

        let py = r.y + c.px(240);
        c.text(tr().tm.retention, Font::Body, rect(x, py, iw, c.px(24)), t.text_dim, Align::Left);

        // A number box with a spinner on its right edge.
        let field = rect(x, py + c.px(28), c.px(110), c.px(32));
        let spin_w = c.px(24);
        let spin = rect(field.r() - spin_w, field.y + 1, spin_w - 1, field.h - 2);
        let up = rect(spin.x, spin.y, spin.w, spin.h / 2);
        let down = rect(spin.x, up.b(), spin.w, spin.h - up.h);
        d.field = field;
        d.up = up;
        d.down = down;
        c.round(field, c.px(6) as f32, t.field, 255);
        let border = if d.focused { t.accent } else if d.hover == Some(6) { t.text_faint } else { t.border };
        c.stroke(field, c.px(6) as f32, 1.0, border, 255);
        let text_area = rect(field.x + c.px(10), field.y, spin.x - field.x - c.px(14), field.h);
        let tw = c.measure(&d.text, Font::Body);
        if d.focused && d.fresh && !d.text.is_empty() {
            c.fill(rect(text_area.x - c.px(2), field.y + c.px(7), tw + c.px(4), field.h - c.px(14)), t.selection);
        }
        c.text(&d.text, Font::Body, text_area, t.text, Align::Left);
        if d.focused && !d.fresh {
            c.fill(rect(text_area.x + tw + 1, field.y + c.px(8), 1, field.h - c.px(16)), t.text);
        }
        let cur = d.minutes().unwrap_or(s.tm_minutes);
        c.fill(rect(spin.x - 1, spin.y + c.px(4), 1, spin.h - c.px(8)), t.border);
        for (k, (b, glyph_ch, enabled)) in [(up, icons::CHEVRON_UP, cur < MAX_MINUTES), (down, icons::CHEVRON_DOWN, cur > 1)].iter().enumerate() {
            if *enabled && d.hover == Some(3 + k as u8) {
                c.fill(b.inset(c.px(2), 1), t.hover);
            }
            c.text(&glyph(*glyph_ch), Font::IconSmall, *b, if *enabled { t.text } else { t.text_faint }, Align::Center);
            c.tip(*b, if k == 0 { tr().tm.longer } else { tr().tm.shorter });
        }
        c.text(tr().tm.min_unit, Font::Body, rect(field.r() + c.px(8), field.y, c.px(24), field.h), t.text, Align::Left);
        let hint = (tr().tm.minutes_range)(MAX_MINUTES);
        c.text(&hint, Font::Small, rect(field.r() + c.px(36), field.y, iw - field.w - c.px(36), field.h), t.text_dim, Align::Left);

        let mb = estimate / 1024.0 / 1024.0;
        let est = (tr().tm.mem_estimate)(mb);
        c.text(&est, Font::Body, rect(x, py + c.px(70), iw, c.px(24)), t.text, Align::Left);
        let interval = if s.interval_ms == 0 { 1000 } else { s.interval_ms };
        let basis = (tr().tm.estimate_basis)(procs, interval as f32 / 1000.0);
        c.text(&basis, Font::Small, rect(x, py + c.px(94), iw, c.px(20)), t.text_dim, Align::Left);

        let (bw, bh) = (c.px(120), c.px(36));
        let close = rect(r.r() - c.px(22) - bw, r.b() - bh - c.px(16), bw, bh);
        d.close = close;
        c.round(close, c.px(6) as f32, t.accent, if d.hover == Some(5) { 255 } else { 225 });
        c.text(tr().common.close, Font::Body, close, t.on_accent, Align::Center);
    }

    pub(super) fn paint_export(&mut self, c: &mut Canvas, area: Rect) {
        let filtering = self.table.filtering();
        let single = self.sel.is_some_and(|(a, b)| a == b);
        let Some(d) = &mut self.export else { return };
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(500).min(area.w - c.px(32));
        let h = c.px(372).min(area.h - c.px(16));
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
        d.panel = r;
        panel(c, r);
        let x = r.x + c.px(22);
        let iw = r.w - c.px(44);
        c.text(tr().common.export, Font::Semi, rect(x, r.y + c.px(18), iw, c.px(26)), t.text, Align::Left);

        // A row of radio buttons; returns their rectangles.
        let radio_row = |c: &mut Canvas, y: i32, labels: &[&str], chosen: usize, hover_base: u8, hover: Option<u8>| -> Vec<Rect> {
            let mut rx = x;
            let mut out = Vec::new();
            for (i, label) in labels.iter().enumerate() {
                let lw = c.measure(label, Font::Body);
                let row = rect(rx, y, lw + c.px(40), c.px(28));
                out.push(row);
                if hover == Some(hover_base + i as u8) {
                    c.round(row, c.px(4) as f32, t.hover, 255);
                }
                let dot = rect(row.x + c.px(6), row.y + (row.h - c.px(16)) / 2, c.px(16), c.px(16));
                let on = i == chosen;
                c.round(dot, dot.h as f32 / 2.0, if on { t.accent } else { t.window }, 255);
                c.stroke(dot, dot.h as f32 / 2.0, 1.0, if on { t.accent } else { t.text_dim }, 255);
                if on {
                    c.round(dot.inset(c.px(5), c.px(5)), c.px(3) as f32, t.on_accent, 255);
                }
                c.text(label, Font::Body, rect(dot.r() + c.px(8), row.y, lw + c.px(4), row.h), t.text, Align::Left);
                rx = row.r() + c.px(12);
            }
            out
        };

        // Which samples: the selection (a time or a range), or everything kept.
        c.text(tr().tm.export_range, Font::Body, rect(x, r.y + c.px(52), iw, c.px(24)), t.text_dim, Align::Left);
        let first = if single { tr().tm.selected_point } else { tr().tm.selected_period };
        let rects = radio_row(c, r.y + c.px(78), &[first, tr().tm.whole], (d.scope == Scope::All) as usize, 7, d.hover);
        d.scopes = [rects[0], rects[1]];

        // What to write, and what the chosen one writes.
        c.text(tr().tm.export_content, Font::Body, rect(x, r.y + c.px(112), iw, c.px(24)), t.text_dim, Align::Left);
        let labels: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
        let chosen = Kind::ALL.iter().position(|k| *k == d.kind).unwrap_or(0);
        let rects = radio_row(c, r.y + c.px(138), &labels, chosen, 0, d.hover);
        d.radios = [rects[0], rects[1], rects[2]];
        // The whole record's table is not the one on screen.
        let description = if d.scope == Scope::All && d.kind == Kind::Table {
            tr().tm.whole_table_desc
        } else {
            d.kind.description()
        };
        c.text(description, Font::Small, rect(x, r.y + c.px(170), iw, c.px(20)), t.text_dim, Align::Left);
        if filtering && d.kind.filtered() {
            c.text(tr().tm.filter_applies, Font::Small, rect(x, r.y + c.px(190), iw, c.px(20)), t.warn, Align::Left);
        }

        // Format and encoding, side by side.
        let half = (iw - c.px(16)) / 2;
        let ly = r.y + c.px(218);
        c.text(tr().tm.format, Font::Body, rect(x, ly, half, c.px(24)), t.text_dim, Align::Left);
        c.text(tr().tm.encoding, Font::Body, rect(x + half + c.px(16), ly, half, c.px(24)), t.text_dim, Align::Left);
        d.format_box = rect(x, ly + c.px(26), half, c.px(32));
        d.encoding_box = rect(x + half + c.px(16), ly + c.px(26), half, c.px(32));
        for (b, label, id) in [(d.format_box, d.format.label(), 3u8), (d.encoding_box, d.encoding.label(), 4u8)] {
            c.round(b, c.px(6) as f32, if d.hover == Some(id) { t.hover } else { t.field }, 255);
            c.stroke(b, c.px(6) as f32, 1.0, t.border, 255);
            c.text(label, Font::Body, rect(b.x + c.px(10), b.y, b.w - c.px(36), b.h), t.text, Align::Left);
            c.text(&glyph(icons::CHEVRON_DOWN), Font::IconSmall, rect(b.r() - c.px(28), b.y, c.px(20), b.h), t.text_dim, Align::Center);
        }
        if d.encoding == Encoding::ShiftJis {
            let b = d.encoding_box;
            c.text(tr().tm.lossy, Font::Small, rect(b.x, b.b() + c.px(4), b.w, c.px(20)), t.warn, Align::Left);
        }

        let (bw, bh) = (c.px(110), c.px(36));
        d.ok = rect(r.r() - c.px(22) - bw, r.b() - bh - c.px(16), bw, bh);
        d.cancel = rect(d.ok.x - c.px(8) - bw, d.ok.y, bw, bh);
        // The size on the buttons' line, at the left.
        let size = d.estimate.map_or_else(|| "—".to_string(), |(_, b)| size_text(b));
        c.text(&(tr().tm.est_size)(&size), Font::Body, rect(x, d.ok.y, (d.cancel.x - x - c.px(8)).max(0), bh), t.text, Align::Left);
        c.round(d.cancel, c.px(6) as f32, if d.hover == Some(5) { t.pressed } else { t.hover }, 255);
        c.text(tr().common.cancel, Font::Body, d.cancel, t.text, Align::Center);
        c.round(d.ok, c.px(6) as f32, t.accent, if d.hover == Some(6) { 255 } else { 225 });
        c.text(tr().tm.write, Font::Body, d.ok, t.on_accent, Align::Center);

        if let Some((_, m)) = &mut d.list {
            m.paint(c, area);
        }
    }
}
