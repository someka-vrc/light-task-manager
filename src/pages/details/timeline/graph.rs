//! Painting the graph, its tooltip and the dialogs.

use super::target::Target;
use super::*;

/// GPU adapters get their own colors up to the third, kept in the cool hues next to each
/// other so they read as one kind; the rest share one.
fn gpu_color(rank: usize) -> u32 {
    match rank {
        0 => THEME.chart_gpu,
        1 => 0xA07CFF,
        2 => 0xF072C8,
        _ => 0xB4B4BE,
    }
}

fn dim(color: u32) -> u32 {
    let ch = |sh: u32| ((((color >> sh) & 255) * 3 / 5) & 255) << sh;
    ch(16) | ch(8) | ch(0)
}

/// Committed memory is a kind of memory, so it takes the memory color dimmed.
fn commit_color() -> u32 {
    dim(THEME.chart_mem)
}

/// Plots one series over the view, a column of pixels at a time. Where a pixel covers
/// several samples it shows their maximum, so brief spikes stay visible when zoomed out.
#[allow(clippy::too_many_arguments)]
fn plot_series(c: &mut Canvas, inner: Rect, data: &[Arc<Sample>], start: i64, end: i64, f: &SeriesFn, color: u32, max_gap: i64) {
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

impl Timeline {
    /// GPU adapters with values in the shown samples, with their number (the rank of the
    /// LUID, the same everywhere in the app). Adapters that are gone drop out.
    pub fn shown_adapters(&self) -> Vec<(usize, usize)> {
        let data = self.data();
        let mut out: Vec<(usize, usize)> = (0..self.adapters.len())
            .filter(|i| data.iter().any(|s| s.gpus.get(*i).is_some_and(|v| !v.is_nan())))
            .map(|i| (i, crate::system::sysinfo::gpu_number(&self.adapters[i].id)))
            .collect();
        out.sort_by_key(|(_, rank)| *rank);
        out
    }

    /// The lines of the graph: label, color, value.
    fn series(&self, ts: &[Target]) -> Vec<(String, u32, Box<SeriesFn>)> {
        let t = THEME;
        let adapters = self.shown_adapters();
        let mut out: Vec<(String, u32, Box<SeriesFn>)> = Vec::new();
        for tg in ts {
            match tg {
                Target::Cpu => out.push(("CPU".into(), t.chart_cpu, Box::new(|x: &Sample| Some(x.cpu)))),
                Target::CpuDetail => out.push((tr().tm.cpu_kernel.into(), dim(t.chart_cpu), Box::new(|x: &Sample| Some(x.kernel)))),
                Target::Mem => out.push((tr().common.memory.into(), t.chart_mem, Box::new(|x: &Sample| Some(x.mem_pct())))),
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
                // Each adapter's use and memory sit together, both where GPU comes.
                Target::GpuMem if ts.contains(&Target::Gpu) => {}
                Target::Gpu | Target::GpuMem => {
                    for &(i, rank) in &adapters {
                        if ts.contains(&Target::Gpu) {
                            out.push((format!("GPU {rank}"), gpu_color(rank), Box::new(move |x: &Sample| x.gpus.get(i).copied().filter(|v| !v.is_nan()))));
                        }
                        let total = self.adapters[i].total;
                        if ts.contains(&Target::GpuMem) && total > 0 {
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
            }
        }
        out
    }

    /// Lays the legend out in rows; returns each item's position and the rows used.
    fn legend_layout(c: &mut Canvas, items: &[(String, u32, Box<SeriesFn>)], width: i32) -> (Vec<(i32, i32)>, i32) {
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

    pub fn paint_graph(&mut self, c: &mut Canvas, block: Rect, s: &Settings) {
        let t = THEME;
        let lh = c.px(22);
        let ts = targets(s);
        let series = self.series(&ts);
        // The legend wraps, and the graph gives up the height it takes.
        let hundred = c.measure("100%", Font::Small) + c.px(12);
        let (places, rows) = Self::legend_layout(c, &series, block.w - hundred);
        let head = rect(block.x, block.y, block.w, lh * rows);
        let plot = rect(block.x, head.b(), block.w, (block.h - head.h - lh).max(0));
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

    /// The tooltip over the graph, with every value at the sample under the pointer.
    pub fn paint_hover(&mut self, c: &mut Canvas, area: Rect, s: &Settings) {
        let t = THEME;
        let (mx, my) = self.mouse;
        let inner = self.plot.inset(1, 1);
        if !s.tm_enabled || !inner.contains(mx, my) || self.overlay_open() || self.drag.is_some() {
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
        let ts = targets(s);
        for &tg in &ts {
            match tg {
                Target::Cpu => lines.push((format!("CPU  {:.1}%", sample.cpu), t.chart_cpu, Some(sample.cpu))),
                Target::CpuDetail => {
                    let speed = if sample.mhz > 0.0 { format!("  {}  {:.2} GHz", tr().perf.speed, sample.mhz / 1000.0) } else { String::new() };
                    lines.push((format!("{}  {:.1}%{speed}", tr().tm.cpu_kernel, sample.kernel), dim(t.chart_cpu), Some(sample.kernel)));
                }
                Target::Mem => lines.push((
                    format!("{}  {} MB ({:.1}%)", tr().common.memory, group_thousands(sample.mem_used / 1024 / 1024), sample.mem_pct()),
                    t.chart_mem,
                    Some(sample.mem_pct()),
                )),
                Target::Commit => lines.push((
                    format!(
                        "{}  {} / {} MB ({:.1}%)",
                        tr().common.commit_mem,
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
                // As in the legend, each adapter's use and memory sit together.
                Target::GpuMem if ts.contains(&Target::Gpu) => {}
                Target::Gpu | Target::GpuMem => {
                    for &(i, rank) in &adapters {
                        let Some(v) = sample.gpus.get(i).filter(|v| !v.is_nan()) else { continue };
                        if ts.contains(&Target::Gpu) {
                            let name = &self.adapters[i].name;
                            let name = if name.is_empty() { String::new() } else { format!(" ({name})") };
                            lines.push((format!("GPU {rank}{name}  {v:.0}%"), gpu_color(rank), Some(*v)));
                        }
                        if ts.contains(&Target::GpuMem) {
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
        // The last line tells where the settings are.
        let hint = tr().tm.right_click_settings;

        let lh = c.px(20);
        // Each line but the time leads with its color, and the text stays readable.
        let sw = c.px(16);
        let font = |k: usize| if Some(k) == bold { Font::SmallSemi } else { Font::Small };
        let hint_w = c.measure(hint, Font::Small) - sw;
        let w = lines.iter().enumerate().map(|(k, (l, _, _))| c.measure(l, font(k))).max().unwrap_or(0).max(hint_w) + c.px(20) + sw;
        let h = lh * (lines.len() as i32 + 1) + c.px(18);
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
        let hy = r.y + c.px(12) + lh * lines.len() as i32;
        c.fill(rect(r.x + c.px(8), hy - c.px(4), r.w - c.px(16), 1), t.border);
        c.text(hint, Font::Small, rect(r.x + c.px(10), hy, w - c.px(20), lh), t.text_dim, Align::Left);
    }

    /// The menus and dialogs, over everything.
    pub fn paint_overlays(&mut self, c: &mut Canvas, area: Rect, s: &Settings) {
        if let Some(m) = &mut self.menu {
            m.paint(c, area);
        }
        self.paint_dialog(c, area, s);
        self.paint_confirm(c, area);
        self.paint_export(c, area);
    }

    fn paint_export(&mut self, c: &mut Canvas, area: Rect) {
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
        if d.kind.filtered() {
            c.text(tr().tm.filter_applies, Font::Small, rect(x, r.y + c.px(190), iw, c.px(20)), t.text_dim, Align::Left);
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

pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1024.0 * 1024.0 {
        (tr().units.approx)(&format!("{:.0} KB", (b / 1024.0).max(1.0)))
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        (tr().units.approx)(&format!("{:.1} MB", b / 1024.0 / 1024.0))
    } else {
        (tr().units.approx)(&format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0))
    }
}
