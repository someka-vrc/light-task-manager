//! Layout and painting of the toolbar, the header, the rows and the overlays.

use super::*;

/// The view mode icons: ≡ flat, ▦ groups, ⊢ tree.
fn paint_view_icon(c: &mut Canvas, area: Rect, mode: ViewMode, color: u32) {
    let (cx, cy) = (area.x + area.w / 2, area.y + area.h / 2);
    let w = c.px(2).max(1);
    let (half, step) = (c.px(8), c.px(5));
    match mode {
        ViewMode::Flat => {
            for k in -1..=1 {
                c.fill(rect(cx - half, cy + k * step - w / 2, half * 2, w), color);
            }
        }
        ViewMode::Group => {
            let s = c.px(7);
            let gap = c.px(2);
            for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                let r = rect(cx + dx * (s + gap) + if dx == 0 { gap / 2 } else { -gap / 2 }, cy + dy * (s + gap) + if dy == 0 { gap / 2 } else { -gap / 2 }, s, s);
                c.stroke(r, 1.5, 1.2, color, 255);
            }
        }
        ViewMode::Tree => {
            let left = cx - half + c.px(1);
            let top = cy - c.px(8);
            c.fill(rect(left, top, w, c.px(13)), color);
            c.fill(rect(left, top, c.px(9), w), color);
            for k in 0..2 {
                let y = top + c.px(6) + k * c.px(6);
                c.fill(rect(left, y, c.px(6), w), color);
                c.fill(rect(left + c.px(8), y, c.px(8), w), color);
            }
        }
    }
}

/// The expand-all (pointing down) and collapse-all (pointing up) icons.
fn paint_double_chevron(c: &mut Canvas, area: Rect, up: bool, color: u32) {
    let ch = if up { icons::CHEVRON_UP } else { icons::CHEVRON_DOWN };
    let shift = c.px(4);
    for dy in [-shift, shift] {
        c.text(&glyph(ch), Font::IconSmall, rect(area.x, area.y + dy, area.w, area.h), color, Align::Center);
    }
}

/// The aim button's icon: a ring with four ticks.
fn paint_crosshair(c: &mut Canvas, area: Rect, color: u32) {
    let (cx, cy) = (area.x + area.w / 2, area.y + area.h / 2);
    let ring = c.px(12);
    c.stroke(rect(cx - ring / 2, cy - ring / 2, ring, ring), ring as f32 / 2.0, 1.5, color, 255);
    let (reach, tick, w) = (c.px(9), c.px(5), c.px(2).max(1));
    c.fill(rect(cx - w / 2, cy - reach, w, tick), color);
    c.fill(rect(cx - w / 2, cy + reach - tick, w, tick), color);
    c.fill(rect(cx - reach, cy - w / 2, tick, w), color);
    c.fill(rect(cx + reach - tick, cy - w / 2, tick, w), color);
    c.fill(rect(cx - w / 2, cy - w / 2, w, w), color);
}

impl DetailsPage {
    pub(super) fn layout(&mut self, c: &Canvas, area: Rect, settings: &Settings) {
        let toolbar = c.px(60);
        let side = c.px(12);
        let table = rect(area.x + side, area.y + toolbar, area.w - side * 2, area.h - toolbar);
        let (row_h, header_h) = (c.px(28), c.px(32));
        let mut x = 0;
        let mut cols = Vec::with_capacity(settings.columns.len());
        for (col, w) in &settings.columns {
            let w = c.px(*w as i32);
            cols.push(ColBox { col: *col, x, w });
            x += w;
        }
        let right = area.r() - c.px(16);
        let btn_h = c.px(34);
        let btn_y = area.y + (toolbar - btn_h) / 2;
        let end_w = btn_h;
        let aim_btn = rect(right - end_w - c.px(8) - btn_h, btn_y, btn_h, btn_h);
        let col_btn = rect(aim_btn.x - c.px(8) - btn_h, btn_y, btn_h, btn_h);
        // The view button is wider: it carries a small arrow for its menu.
        let view_w = btn_h + c.px(14);
        let view_btn = rect(col_btn.x - c.px(6) - view_w, btn_y, view_w, btn_h);
        let collapse_btn = rect(view_btn.x - c.px(6) - btn_h, btn_y, btn_h, btn_h);
        let expand_btn = rect(collapse_btn.x - c.px(4) - btn_h, btn_y, btn_h, btn_h);
        self.l = Layout {
            area,
            table,
            body: rect(table.x, table.y + header_h, table.w, table.h - header_h),
            row_h,
            header_h,
            pad: c.px(10),
            edge: c.px(4),
            bar: c.px(12),
            content_w: x,
            cols,
            end_btn: rect(right - end_w, btn_y, end_w, btn_h),
            aim_btn,
            col_btn,
            view_btn,
            expand_btn,
            collapse_btn,
            chevron: c.px(18),
            indent: c.px(20),
        };
        if let Some(last) = self.l.cols.last_mut() {
            if x < table.w {
                last.w += table.w - x;
                self.l.content_w = table.w;
            }
        }
        self.clamp_scroll();
    }

    pub(super) fn paint_toolbar(&mut self, c: &mut Canvas, settings: &Settings) {
        let t = THEME;
        let l = &self.l;
        let area = l.area;
        let title = tr().details.title;
        let title_w = c.measure(title, Font::Title);
        let toolbar = rect(area.x + c.px(20), area.y, title_w + c.px(4), c.px(60));
        // In a narrow window the buttons take the room and the title goes.
        let leftmost = if self.mode == ViewMode::Flat { l.view_btn.x } else { l.expand_btn.x };
        if toolbar.r() + c.px(12) <= leftmost {
            c.text(title, Font::Title, toolbar, t.text, Align::Left);
        }

        let r = c.px(6) as f32;
        let view_btn = l.view_btn;
        let menu_open = matches!(&self.overlay, Overlay::Menu { items, .. } if items.iter().any(|i| matches!(i.act, Act::View(_))));
        c.round(view_btn, r, if menu_open { t.pressed } else if self.hover_view { t.hover } else { t.content }, 255);
        let icon = rect(view_btn.x + c.px(2), view_btn.y, view_btn.h, view_btn.h);
        paint_view_icon(c, icon, settings.view, t.text);
        let arrow = rect(icon.r() - c.px(6), view_btn.y, view_btn.r() - icon.r() + c.px(4), view_btn.h);
        c.text(&glyph(icons::CHEVRON_DOWN), Font::IconSmall, arrow, t.text_dim, Align::Center);
        c.tip(view_btn, (tr().details.switch_view)(settings.view.label()));

        if self.mode != ViewMode::Flat {
            let tiers = [(l.expand_btn, self.hover_expand, false), (l.collapse_btn, self.hover_collapse, true)];
            for (b, hover, up) in tiers {
                c.round(b, r, if hover { t.hover } else { t.content }, 255);
                paint_double_chevron(c, b, up, t.text);
                c.tip(b, if up { tr().details.collapse_all } else { tr().details.expand_all });
            }
        }

        let col_btn = l.col_btn;
        c.round(col_btn, r, if self.hover_col { t.hover } else { t.content }, 255);
        c.text(&glyph(icons::COLUMNS), Font::Icon, col_btn, t.text, Align::Center);
        c.tip(col_btn, tr().details.pick_columns);

        let aim_btn = l.aim_btn;
        let bg = if self.aiming { t.selection } else if self.hover_aim { t.hover } else { t.content };
        c.round(aim_btn, r, bg, 255);
        paint_crosshair(c, aim_btn, if self.aiming { t.accent } else { t.text });
        c.tip(aim_btn, tr().details.find_by_window);

        let can_end = self.selected_proc().is_some();
        let end_btn = l.end_btn;
        let fg = if can_end { t.text } else { t.text_faint };
        if can_end && self.hover_end {
            c.round(end_btn, r, t.hover, 255);
        }
        c.stroke(end_btn, r, 1.0, t.border, 255);
        c.text(&glyph(icons::BLOCK), Font::IconSmall, end_btn, fg, Align::Center);
        c.tip(end_btn, tr().details.end_task_tip);
    }

    pub(super) fn paint_header(&mut self, c: &mut Canvas, settings: &Settings) {
        let t = THEME;
        let (table, pad, header_h) = (self.l.table, self.l.pad, self.l.header_h);
        let top = rect(table.x, table.y, table.w, header_h);
        c.set_clip(top);
        for (i, b) in self.l.cols.iter().enumerate() {
            let x = table.x + b.x - self.scroll_x;
            if x + b.w < table.x || x > table.r() {
                continue;
            }
            let cell = rect(x, table.y, b.w, header_h);
            if self.hover_header == Some(i) && !self.hover_edge {
                c.round(cell.inset(c.px(2), c.px(3)), c.px(4) as f32, t.hover, 255);
            }
            let d = columns::def(b.col);
            let sorted = settings.sort == b.col;
            let arrow = if sorted { c.px(14) } else { 0 };
            let text_area = rect(x + pad, table.y, b.w - pad * 2 - arrow, header_h);
            let align = if d.right { Align::Right } else { Align::Left };
            let color = if sorted { t.text } else { t.text_dim };
            c.text(d.label(), Font::Small, text_area, color, align);
            if sorted {
                let ch = if settings.descending { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP };
                let a = rect(x + b.w - pad - arrow, table.y, arrow, header_h);
                c.text(&glyph(ch), Font::IconSmall, a, t.accent, Align::Right);
            }
            if self.hover_header == Some(i) && self.hover_edge {
                c.fill(rect(x + b.w - 1, table.y + c.px(7), c.px(2), header_h - c.px(14)), t.accent);
            }
        }
        c.reset_clip();
        c.fill(rect(table.x, table.y + header_h - 1, table.w, 1), t.border);
    }

    pub(super) fn paint_rows(&mut self, c: &mut Canvas) {
        let t = THEME;
        let (body, rh, pad, table) = (self.l.body, self.l.row_h, self.l.pad, self.l.table);
        c.set_clip(body);
        let scroll = self.scroll_y.round() as i32;
        let first = (scroll / rh.max(1)) as usize;
        let last = (((scroll + body.h) / rh.max(1)) as usize + 1).min(self.view.len());
        let icon_size = c.px(16);
        let radius = c.px(5) as f32;
        let (chevron, indent) = (self.l.chevron, self.l.indent);
        let mut buf = std::mem::take(&mut self.cell);
        for i in first..last {
            let kind = self.view[i];
            let p = self.row_proc(kind);
            let members = match kind {
                Row::Group(g) => Some(self.groups[g as usize].members.len()),
                _ => None,
            };
            let y = body.y + i as i32 * rh - scroll;
            let row = rect(table.x, y, table.w, rh);
            let is_selected = self.row_selected(kind);
            let m = self.meta[i];
            let ended = matches!(kind, Row::Ghost(_));
            // A process that just started or ended is tinted for a while.
            let life = if ended {
                Some((t.died, t.died_bg))
            } else if !matches!(kind, Row::Group(_)) && self.born.contains_key(&p.key()) {
                Some((t.born, t.born_bg))
            } else {
                None
            };
            let inner = row.inset(c.px(2), c.px(1));
            let marker = rect(row.x + c.px(2), y + c.px(7), c.px(3), rh - c.px(14));
            if is_selected {
                c.round(inner, radius, t.selection, 255);
            } else if let Some((_, bg)) = life {
                c.round(inner, radius, bg, 255);
                if self.hover_row == Some(i) {
                    c.round(inner, radius, t.text, 14);
                }
            } else if self.hover_row == Some(i) {
                c.round(inner, radius, t.hover, 255);
            }
            if let Some((fg, _)) = life {
                c.round(marker, 1.5, fg, 255);
            } else if is_selected {
                c.round(marker, 1.5, t.accent, 255);
            }
            for b in &self.l.cols {
                let x = table.x + b.x - self.scroll_x;
                if x + b.w < table.x || x > table.r() {
                    continue;
                }
                let mut cell = rect(x + pad, y, b.w - pad * 2, rh);
                let d = columns::def(b.col);
                let mut color = if ended { t.died } else if m.faded { t.text_faint } else { t.text };
                if b.col == Col::Name {
                    if self.mode != ViewMode::Flat {
                        let at = m.depth as i32 * indent;
                        if matches!(kind, Row::Group(_)) || m.branch {
                            let ch = if m.open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                            c.text(&glyph(ch), Font::IconSmall, rect(cell.x + at, y, chevron, rh), t.text_dim, Align::Left);
                        }
                        let shift = (at + chevron).min(cell.w);
                        cell.x += shift;
                        cell.w -= shift;
                    }
                    let top = y + (rh - icon_size) / 2;
                    match p.stat.image.as_ref().and_then(|m| m.icon()) {
                        Some(icon) => c.icon(icon, cell.x, top, icon_size),
                        None => c.round(rect(cell.x + 1, top + 1, icon_size - 2, icon_size - 2), 3.0, t.border, 255),
                    }
                    let shift = icon_size + c.px(8);
                    cell.x += shift;
                    cell.w -= shift;
                } else if ended || m.faded {
                    color = t.text_faint;
                } else if !is_selected {
                    color = t.text_dim;
                    if b.col == Col::Cpu && p.cpu.round() >= 1.0 && p.pid != 0 {
                        color = t.text;
                    } else if b.col == Col::Status && p.status == crate::system::model::Status::NotResponding {
                        color = t.warn;
                    }
                }
                buf.clear();
                fill_cell(b.col, p, members, &self.scale, &mut buf);
                let align = if d.right { Align::Right } else { Align::Left };
                c.text(&buf, Font::Body, cell, color, align);
            }
        }
        self.cell = buf;

        if self.view.is_empty() && !self.snap.procs.is_empty() {
            c.text(tr().details.no_match, Font::Body, body, t.text_dim, Align::Center);
        }

        let vactive = self.hover_vbar || matches!(self.drag, Drag::VThumb { .. });
        if let Some(thumb) = self.vthumb() {
            let w = if vactive { c.px(8) } else { c.px(4) };
            let r = rect(thumb.r() - w - c.px(2), thumb.y + c.px(2), w, thumb.h - c.px(4));
            c.round(r, w as f32 / 2.0, t.text_faint, if vactive { 255 } else { 170 });
        }
        let hactive = self.hover_hbar || matches!(self.drag, Drag::HThumb { .. });
        if let Some(thumb) = self.hthumb() {
            let h = if hactive { c.px(8) } else { c.px(4) };
            let r = rect(thumb.x + c.px(2), thumb.b() - h - c.px(2), thumb.w - c.px(4), h);
            c.round(r, h as f32 / 2.0, t.text_faint, if hactive { 255 } else { 170 });
        }
        c.reset_clip();

        if let Drag::Header { idx, x, moving: true, .. } = self.drag {
            let target = self.insertion_index(x);
            let bx = match self.l.cols.get(target) {
                Some(b) => b.x,
                None => self.l.content_w,
            };
            let lx = table.x + bx - self.scroll_x;
            c.set_clip(table);
            c.fill(rect(lx - 1, table.y, c.px(2), table.h), t.accent);
            if let Some(b) = self.l.cols.get(idx) {
                let ghost = rect(x - b.w / 2, table.y + c.px(2), b.w, self.l.header_h - c.px(4));
                c.round(ghost, c.px(4) as f32, t.pressed, 230);
                c.text(columns::def(b.col).label(), Font::Small, ghost.inset(pad, 0), t.text, Align::Left);
            }
            c.reset_clip();
        }
    }

    pub(super) fn panel(c: &mut Canvas, r: Rect) {
        // The panel covers whatever it lies over, tooltips included.
        c.clear_tips();
        let t = THEME;
        let radius = c.px(8) as f32;
        for i in (1..=4).rev() {
            let grow = c.px(i * 3);
            c.round(rect(r.x - grow, r.y - grow + c.px(2), r.w + grow * 2, r.h + grow * 2), radius + grow as f32, 0, 22);
        }
        c.round(r, radius, t.popup, 255);
        c.stroke(r, radius, 1.0, t.border, 255);
    }

    pub(super) fn paint_overlay(&mut self, c: &mut Canvas, settings: &Settings) {
        let t = THEME;
        let area = self.l.area;
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Menu { origin, items, rect: out, hover } => {
                let item_h = c.px(34);
                let (w, h) = (c.px(230), item_h * items.len() as i32);
                let x = origin.0.min(area.r() - w - c.px(6)).max(area.x);
                let y = origin.1.min(area.b() - h - c.px(6)).max(area.y);
                *out = rect(x, y, w, h);
                Self::panel(c, *out);
                for (i, item) in items.iter().enumerate() {
                    let row = rect(x, y + i as i32 * item_h, w, item_h);
                    if *hover == Some(i) && item.enabled {
                        c.round(row.inset(c.px(4), c.px(2)), c.px(4) as f32, t.pressed, 255);
                    }
                    let color = if item.enabled { t.text } else { t.text_faint };
                    let mut label = row.inset(c.px(14), 0);
                    if let Some(on) = item.checked {
                        if on {
                            c.text(&glyph(icons::CHECK), Font::IconSmall, rect(label.x, row.y, c.px(16), item_h), t.accent, Align::Left);
                        }
                        label.x += c.px(26);
                        label.w -= c.px(26);
                    }
                    c.text(item.label, Font::Body, label, color, Align::Left);
                }
            }
            Overlay::Confirm { targets, name, tree, hover, buttons } => {
                c.fill_alpha(area, 0, 110);
                let (w, h) = (c.px(440), c.px(192));
                let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
                Self::panel(c, r);
                let inner = r.inset(c.px(22), 0);
                let group = targets.len() > 1;
                let (s, d) = (&tr().common, &tr().details);
                let title = if *tree { d.end_tree_q } else { s.end_task_q };
                c.text(title, Font::Semi, rect(inner.x, r.y + c.px(18), inner.w, c.px(26)), t.text, Align::Left);
                c.text(name, Font::Body, rect(inner.x, r.y + c.px(52), inner.w, c.px(24)), t.accent, Align::Left);
                let note = match (group, *tree) {
                    (true, true) => d.end_group_tree,
                    (true, false) => d.end_group,
                    (false, true) => d.end_proc_tree,
                    (false, false) => s.end_this_process,
                };
                c.text(note, Font::Small, rect(inner.x, r.y + c.px(80), inner.w, c.px(22)), t.text_dim, Align::Left);
                c.text(s.unsaved_lost, Font::Small, rect(inner.x, r.y + c.px(100), inner.w, c.px(22)), t.text_dim, Align::Left);
                let (bw, bh) = (c.px(120), c.px(36));
                let by = r.b() - bh - c.px(16);
                buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
                buttons[0] = rect(buttons[1].x - bw - c.px(8), by, bw, bh);
                let radius = c.px(6) as f32;
                c.round(buttons[0], radius, t.danger, if *hover == Some(0) { 255 } else { 225 });
                c.text(s.end_ok, Font::Body, buttons[0], 0xFFFFFF, Align::Center);
                c.round(buttons[1], radius, if *hover == Some(1) { t.pressed } else { t.hover }, 255);
                c.text(s.cancel, Font::Body, buttons[1], t.text, Align::Center);
            }
            Overlay::Columns { hover, cells, hover_btn, buttons, panel, scroll, max_scroll, list, line } => {
                c.fill_alpha(area, 0, 110);
                let cell_h = c.px(30);
                let gap = c.px(24);
                let margin = c.px(32);
                // Use the most panels that still fit the window; with one panel, labels may be cut.
                let mut chosen: Option<(PickerLayout, Vec<i32>)> = None;
                for layout in PICKER_LAYOUTS.iter() {
                    let widths = picker_panel_widths(c, layout);
                    let total = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1) + c.px(32);
                    let fits = total <= area.w - margin;
                    chosen = Some((*layout, widths));
                    if fits {
                        break;
                    }
                }
                let (layout, panel_w) = chosen.unwrap_or((PICKER_LAYOUTS[PICKER_LAYOUTS.len() - 1], Vec::new()));
                let mut panel_x = Vec::with_capacity(panel_w.len());
                let mut x_acc = c.px(16);
                for w in &panel_w {
                    panel_x.push(x_acc);
                    x_acc += w + gap;
                }
                let w = (x_acc - gap + c.px(16)).min(area.w - margin);
                // Heights in half rows: a heading and each column take 2, the gap between groups 1.
                let half_rows = |groups: &[usize]| -> i32 {
                    let items: i32 = groups.iter().map(|g| 2 + 2 * columns::GROUPS[*g].1.len() as i32).sum();
                    items + groups.len().saturating_sub(1) as i32
                };
                let rows_half = layout.iter().map(|g| half_rows(g)).max().unwrap_or(0);
                let head = c.px(56);
                let foot = c.px(60);
                let content_h = rows_half * cell_h / 2;
                // The panel is as tall as its content, but never taller than the window allows.
                let h = (head + content_h + foot).min(area.h - c.px(32)).max(head + foot);
                let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
                *panel = r;
                Self::panel(c, r);
                c.text(tr().details.pick_columns, Font::Semi, rect(r.x + c.px(22), r.y + c.px(16), r.w, c.px(28)), t.text, Align::Left);
                let viewport = rect(r.x, r.y + head, r.w, h - head - foot);
                *list = viewport;
                *line = cell_h;
                *max_scroll = (content_h - viewport.h).max(0);
                *scroll = (*scroll).clamp(0, *max_scroll);
                cells.clear();
                c.set_clip(viewport);
                for (p, groups) in layout.iter().enumerate() {
                    let px = r.x + panel_x[p];
                    let cell_w = panel_w[p];
                    let mut y = viewport.y - *scroll;
                    for (gi, g) in groups.iter().enumerate() {
                        if gi > 0 {
                            y += cell_h / 2;
                        }
                        let (name, members) = columns::GROUPS[*g];
                        c.text(name(tr()), Font::Semi, rect(px + c.px(8), y, cell_w - c.px(8), cell_h), t.text_dim, Align::Left);
                        y += cell_h;
                        for col in members.iter() {
                            let cell = rect(px, y, cell_w, cell_h);
                            let i = cells.len();
                            cells.push((cell, *col));
                            if *hover == Some(i) {
                                c.round(cell.inset(c.px(2), c.px(1)), c.px(4) as f32, t.pressed, 255);
                            }
                            let box_size = c.px(16);
                            let bx = rect(cell.x + c.px(8), cell.y + (cell_h - box_size) / 2, box_size, box_size);
                            let on = settings.columns.iter().any(|(s, _)| s == col);
                            if on {
                                c.round(bx, c.px(3) as f32, t.accent, 255);
                                c.text(&glyph(icons::CHECK), Font::IconSmall, bx, t.on_accent, Align::Center);
                            } else {
                                c.stroke(bx, c.px(3) as f32, 1.0, t.text_faint, 255);
                            }
                            let label = rect(bx.r() + c.px(8), cell.y, cell.r() - bx.r() - c.px(12), cell_h);
                            c.text(columns::def(*col).full(), Font::Small, label, t.text, Align::Left);
                            y += cell_h;
                        }
                    }
                }
                c.reset_clip();
                if *max_scroll > 0 {
                    // A thin scroll bar at the right edge of the list.
                    let thumb_h = (viewport.h * viewport.h / content_h.max(1)).max(c.px(24));
                    let travel = (viewport.h - thumb_h).max(0) as f32;
                    let thumb_y = viewport.y + (travel * *scroll as f32 / *max_scroll as f32) as i32;
                    c.round(rect(r.r() - c.px(10), thumb_y, c.px(4), thumb_h), c.px(2) as f32, t.text_faint, 200);
                }
                let (bw, bh) = (c.px(124), c.px(36));
                let by = r.b() - bh - c.px(14);
                buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
                buttons[0] = rect(buttons[1].x - bw - c.px(8), by, bw, bh);
                let radius = c.px(6) as f32;
                c.round(buttons[0], radius, if *hover_btn == Some(0) { t.pressed } else { t.hover }, 255);
                c.text(tr().details.reset_default, Font::Body, buttons[0], t.text, Align::Center);
                c.round(buttons[1], radius, t.accent, if *hover_btn == Some(1) { 255 } else { 230 });
                c.text(tr().common.close, Font::Body, buttons[1], t.on_accent, Align::Center);
            }
        }
    }

    pub(super) fn vthumb(&self) -> Option<Rect> {
        let l = &self.l;
        let max = self.max_scroll_y();
        if max <= 0.0 {
            return None;
        }
        let track = l.body.h - self.hbar_h();
        let content = self.view.len() as i32 * l.row_h;
        let h = (track * track / content.max(1)).max(l.bar * 3).min(track);
        let y = l.body.y + ((track - h) as f32 * (self.scroll_y / max)) as i32;
        Some(rect(l.table.r() - l.bar, y, l.bar, h))
    }

    pub(super) fn hthumb(&self) -> Option<Rect> {
        let l = &self.l;
        let max = self.max_scroll_x();
        if max <= 0 {
            return None;
        }
        let track = l.table.w - l.bar;
        let w = (track * track / l.content_w.max(1)).max(l.bar * 3).min(track);
        let x = l.table.x + ((track - w) as f32 * (self.scroll_x as f32 / max as f32)) as i32;
        Some(rect(x, l.body.b() - l.bar, w, l.bar))
    }
}
