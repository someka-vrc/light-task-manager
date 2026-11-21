//! Mouse, touch and keyboard input, menus, and the actions they run.

use super::*;

impl DetailsPage {
    pub(super) fn update_hover(&mut self, cx: &mut Cx) {
        let (x, y) = self.mouse;
        let l = &self.l;
        let vbar = self.max_scroll_y() > 0.0
            && x >= l.table.r() - l.bar
            && l.body.contains(x, y);
        let hbar = self.hbar_h() > 0 && y >= l.body.b() - l.bar && l.body.contains(x, y);
        let row = if l.body.contains(x, y) && !vbar && !hbar && l.row_h > 0 {
            let index = ((y - l.body.y) as f32 + self.scroll_y) as i32 / l.row_h;
            ((index as usize) < self.view.len()).then_some(index as usize)
        } else {
            None
        };
        let chevron = row.is_some_and(|i| {
            let content_x = x - l.table.x + self.scroll_x;
            let m = self.meta[i];
            let toggles = match self.view[i] {
                Row::Group(_) => true,
                Row::Node(_) => m.branch,
                _ => false,
            };
            toggles
                && l.cols.iter().find(|c| c.col.is_name()).is_some_and(|c| {
                    let start = c.x + m.depth as i32 * l.indent;
                    let w = if cx.touch { (c.x + c.w - start).min(l.pad + l.chevron * 3) } else { l.pad + l.chevron };
                    content_x >= start && content_x < start + w
                })
        });
        self.hover_chevron = chevron;
        let edge_reach = if cx.touch { l.edge * TOUCH_EDGE_FACTOR } else { l.edge };
        let (mut header, mut edge) = (None, false);
        let in_header = y >= l.table.y && y < l.body.y && x >= l.table.x && x < l.table.r();
        if in_header {
            let cx_x = x - l.table.x + self.scroll_x;
            for (i, c) in l.cols.iter().enumerate() {
                if (cx_x - (c.x + c.w)).abs() <= edge_reach {
                    header = Some(i);
                    edge = true;
                    break;
                }
                if cx_x >= c.x && cx_x < c.x + c.w {
                    header = Some(i);
                }
            }
        }
        let end = l.end_btn.contains(x, y);
        let col = l.col_btn.contains(x, y);
        let aim = l.aim_btn.contains(x, y);
        let group = l.view_btn.contains(x, y);
        let tiered = self.mode != ViewMode::Flat;
        let expand = tiered && l.expand_btn.contains(x, y);
        let collapse = tiered && l.collapse_btn.contains(x, y);
        let graph = match (l.graph_arrow.contains(x, y), l.graph_btn.contains(x, y)) {
            (true, _) => 2,
            (false, true) => 1,
            _ => 0,
        };
        let split = l.split.contains(x, y);
        if graph != self.hover_graph || split != self.hover_split {
            self.hover_graph = graph;
            self.hover_split = split;
            *cx.dirty = true;
        }
        if row != self.hover_row
            || header != self.hover_header
            || edge != self.hover_edge
            || vbar != self.hover_vbar
            || hbar != self.hover_hbar
            || end != self.hover_end
            || col != self.hover_col
            || aim != self.hover_aim
            || group != self.hover_view
            || expand != self.hover_expand
            || collapse != self.hover_collapse
        {
            self.hover_row = row;
            self.hover_header = header;
            self.hover_edge = edge;
            self.hover_vbar = vbar;
            self.hover_hbar = hbar;
            self.hover_end = end;
            self.hover_col = col;
            self.hover_aim = aim;
            self.hover_view = group;
            self.hover_expand = expand;
            self.hover_collapse = collapse;
            *cx.dirty = true;
        }
    }

    pub(super) fn apply_columns_changed(&mut self, cx: &mut Cx) {
        // The sampler's needs follow the columns through `Page::demand`.
        if self.range.is_some() {
            // The range table sums up only the items it shows.
            self.show_selection(cx.settings);
        } else {
            self.regroup(cx.settings);
        }
        *cx.dirty = true;
    }

    /// The width of a shown column, in unscaled pixels.
    fn col_width(&self, s: &Settings, i: usize) -> u16 {
        match self.l.cols.get(i).map(|b| b.col) {
            Some(AnyCol::P(_)) => s.columns.get(i).map_or(100, |c| c.1),
            Some(AnyCol::R(rc)) => s.range_items.iter().find(|(it, _)| *it == rc.item).map_or(60, |c| c.1),
            None => 100,
        }
    }

    /// Sets the width of a shown column: in the range table, of every column of its item.
    fn set_col_width(&self, s: &mut Settings, i: usize, w: u16) {
        match self.l.cols.get(i).map(|b| b.col) {
            Some(AnyCol::P(_)) => {
                if let Some(c) = s.columns.get_mut(i) {
                    c.1 = w;
                }
            }
            Some(AnyCol::R(rc)) => {
                if let Some(c) = s.range_items.iter_mut().find(|(it, _)| *it == rc.item) {
                    c.1 = w;
                }
            }
            None => {}
        }
    }

    /// Moves a shown column before another: in the range table, its item with all its columns.
    fn move_col(&self, s: &mut Settings, from: usize, to: usize) {
        match self.l.cols.get(from).map(|b| b.col) {
            Some(AnyCol::P(_)) => {
                let list = &mut s.columns;
                if from < list.len() {
                    let item = list.remove(from);
                    let at = if to > from { to - 1 } else { to };
                    list.insert(at.min(list.len()), item);
                }
            }
            Some(AnyCol::R(rc)) => {
                let target = match self.l.cols.get(to).map(|b| b.col) {
                    Some(AnyCol::R(t)) => Some(t.item),
                    _ => None,
                };
                let list = &mut s.range_items;
                let Some(at) = list.iter().position(|(it, _)| *it == rc.item) else { return };
                let item = list.remove(at);
                let dest = match target {
                    Some(t) if t != rc.item => list.iter().position(|(it, _)| *it == t).unwrap_or(list.len()),
                    Some(_) => at,
                    None => list.len(),
                };
                list.insert(dest.min(list.len()), item);
            }
            None => {}
        }
    }

    /// Clipboard text of a row: the shown columns, or every property when the copy scope is
    /// "all". Properties that need a column to be shown are "-" while hidden.
    pub(super) fn copy_text(&self, row: Row, s: &Settings) -> String {
        let members = match row {
            Row::Group(g) => Some(self.groups[g as usize].members.len()),
            _ => None,
        };
        let p = self.row_proc(row);
        if self.range.is_some() {
            let fields: Vec<(String, String, String)> = self
                .shown_columns(s)
                .into_iter()
                .map(|(c, _)| {
                    let mut buf = String::new();
                    self.cell_text(c, row, members, &mut buf);
                    let AnyCol::R(rc) = c else { return (c.label(), String::new(), buf) };
                    let heading = if rc.op == ROp::None { rc.label() } else { format!("{} {}", rc.item.label(), rc.label()) };
                    (heading, rc.prop(), buf)
                })
                .collect();
            let refs = fields.iter().map(|(h, prop, v)| crate::ui::copyfmt::Field { heading: h, property: prop, value: v.as_str() });
            return crate::ui::copyfmt::format_row(s.copy_format, refs);
        }
        let mut fields: Vec<(&'static str, &'static str, String)> = Vec::new();
        let mut push = |id: Col, value: String| fields.push((columns::def(id).label(), columns::prop(id), value));
        match s.copy_scope {
            crate::ui::copyfmt::Scope::Visible => {
                for (id, _) in &s.columns {
                    let mut buf = String::new();
                    fill_cell(*id, p, members, &self.scale, &mut buf);
                    push(*id, buf);
                }
            }
            crate::ui::copyfmt::Scope::All => {
                for d in columns::display_order().map(columns::def) {
                    let shown = s.columns.iter().any(|(c, _)| *c == d.id);
                    let needs_view = columns::needs_gui_counts(d.id)
                        || columns::needs_proc_gpu(d.id)
                        || columns::needs_proc_net(d.id)
                        || columns::needs_eco_qos(d.id);
                    let mut buf = String::new();
                    if shown || !needs_view {
                        fill_cell(d.id, p, members, &self.scale, &mut buf);
                    } else {
                        buf.push('-');
                    }
                    push(d.id, buf);
                }
            }
        }
        let refs = fields.iter().map(|(h, prop, v)| crate::ui::copyfmt::Field { heading: h, property: prop, value: v.as_str() });
        crate::ui::copyfmt::format_row(s.copy_format, refs)
    }

    pub(super) fn open_row_menu(&mut self, x: i32, y: i32) {
        let has_path = self.selected_proc().is_some_and(|p| !p.stat.path().is_empty());
        // In the past, only what still runs can be ended.
        let can_end = !self.end_targets().is_empty();
        let item = |label, act, enabled| MenuItem { label, act, enabled, checked: None };
        self.overlay = Overlay::Menu {
            origin: (x, y),
            items: vec![
                item(tr().common.end_task, Act::EndTask, can_end),
                item(tr().details.end_tree, Act::EndTree, can_end),
                item(tr().common.open_location, Act::OpenLocation, has_path),
                item(tr().common.properties, Act::Properties, has_path),
                item(tr().common.copy_row, Act::CopyRow, true),
            ],
            rect: Rect::default(),
            hover: None,
        };
    }

    pub(super) fn open_header_menu(&mut self, x: i32, y: i32, col: AnyCol, s: &Settings) {
        let item = |label, act, enabled| MenuItem { label, act, enabled, checked: None };
        let mut items = match col {
            AnyCol::P(c) => vec![item(tr().details.hide_column, Act::HideColumn(c), c != Col::Name && s.columns.len() > 1)],
            // The range table hides an item, or an operation of every item.
            AnyCol::R(rc) => {
                let mut v = vec![item(tr().tm.hide_item, Act::HideItem(rc.item), rc.item != RItem::Name)];
                if rc.item.kind() == range::Kind::Gauge {
                    v.push(item(tr().tm.hide_op, Act::HideOp(rc.op), s.range_ops.count_ones() > 1));
                }
                v
            }
        };
        items.push(item(tr().details.pick_columns_menu, Act::PickColumns, true));
        items.push(item(tr().details.reset_columns, Act::ResetColumns, true));
        self.overlay = Overlay::Menu { origin: (x, y), items, rect: Rect::default(), hover: None };
    }

    /// The view mode menu, below its button.
    pub(super) fn open_view_menu(&mut self, current: ViewMode) {
        let b = self.l.view_btn;
        let items = ViewMode::ALL
            .iter()
            .map(|m| MenuItem { label: m.label(), act: Act::View(*m), enabled: true, checked: Some(*m == current) })
            .collect();
        self.overlay = Overlay::Menu { origin: (b.x, b.b() + 4), items, rect: Rect::default(), hover: None };
    }

    pub(super) fn open_columns(&mut self) {
        self.overlay = Overlay::Columns {
            hover: None,
            cells: Vec::new(),
            hover_btn: None,
            buttons: [Rect::default(); 2],
            panel: Rect::default(),
            scroll: 0,
            max_scroll: 0,
            list: Rect::default(),
            line: 30,
        };
    }

    /// The processes "end task" would end: the selected process or group's members, those
    /// still running, oldest first (so that ending in reverse reaches children before parents).
    pub(super) fn end_targets(&self) -> Vec<(u32, i64)> {
        let mut keys: Vec<(u32, i64)> = match self.selected_group() {
            Some(g) => g.members.iter().map(|m| self.snap.procs[*m as usize].key()).collect(),
            None => self.selected_proc().map(|p| vec![p.key()]).unwrap_or_default(),
        };
        keys.retain(|k| self.alive(*k));
        keys.sort_by_key(|k| k.1);
        keys
    }

    pub(super) fn confirm_end(&mut self, tree: bool) {
        let targets = self.end_targets();
        let chosen = match (self.selected_group(), targets.is_empty()) {
            (_, true) => None,
            (Some(g), _) => Some(((tr().details.n_processes)(&g.summary.stat.name, targets.len()), targets)),
            (None, _) => self.selected_proc().map(|p| (p.stat.name.clone(), targets)),
        };
        if let Some((name, targets)) = chosen {
            self.overlay = Overlay::Confirm {
                targets,
                name,
                tree,
                hover: None,
                buttons: [Rect::default(); 2],
            };
        }
    }

    pub(super) fn run(&mut self, act: Act, cx: &mut Cx) {
        match act {
            Act::EndTask => self.confirm_end(false),
            Act::EndTree => self.confirm_end(true),
            Act::OpenLocation => {
                if let Some(p) = self.selected_proc() {
                    actions::open_location(p.stat.path());
                }
            }
            Act::Properties => {
                if let Some(p) = self.selected_proc() {
                    actions::properties(p.stat.path());
                }
            }
            Act::CopyRow => {
                if let Some(i) = self.selected_index() {
                    let text = self.copy_text(self.view[i], cx.settings);
                    actions::set_clipboard(&text);
                }
            }
            Act::PickColumns => self.open_columns(),
            Act::HideColumn(col) => {
                if col != Col::Name && cx.settings.columns.len() > 1 {
                    cx.settings.columns.retain(|(c, _)| *c != col);
                    self.apply_columns_changed(cx);
                }
            }
            Act::HideItem(item) => {
                if item != RItem::Name {
                    cx.settings.range_items.retain(|(i, _)| *i != item);
                    self.apply_columns_changed(cx);
                }
            }
            Act::HideOp(op) => {
                // At least one operation stays, or the values would have no column.
                if cx.settings.range_ops & !op.bit() != 0 {
                    cx.settings.range_ops &= !op.bit();
                    self.apply_columns_changed(cx);
                }
            }
            Act::ResetColumns => {
                if self.range.is_some() {
                    cx.settings.range_items = range::default_items();
                    cx.settings.range_ops = range::DEFAULT_OPS;
                } else {
                    cx.settings.columns = columns::defaults();
                }
                self.apply_columns_changed(cx);
            }
            Act::View(mode) => {
                cx.settings.view = mode;
                self.regroup(cx.settings);
                if let Some(i) = self.selected_index() {
                    self.select_index(i);
                }
            }
        }
        *cx.dirty = true;
    }

    pub(super) fn end_processes(&mut self, mut targets: Vec<(u32, i64)>, tree: bool, cx: &mut Cx) {
        if tree {
            let mut i = 0;
            while i < targets.len() {
                let (parent, created) = targets[i];
                for p in &self.live.procs {
                    if p.ppid == parent && p.create_time >= created && !targets.contains(&p.key()) {
                        targets.push(p.key());
                    }
                }
                i += 1;
            }
        }
        let mut failed = 0;
        let mut denied = false;
        // Children first, so a parent cannot respawn them.
        for (pid, created) in targets.iter().rev() {
            if let Err(code) = actions::terminate(*pid, *created) {
                failed += 1;
                denied |= code == 5;
            }
        }
        if failed > 0 {
            let hint = if denied { tr().common.access_denied } else { tr().details.maybe_exited };
            cx.cmds.push(Cmd::Toast((tr().details.end_failed)(failed, hint)));
        }
        cx.shared.poke();
    }

    pub(super) fn overlay_event(&mut self, ev: &Ev, cx: &mut Cx) -> bool {
        if matches!(self.overlay, Overlay::None) {
            return false;
        }
        if let Ev::Key { vk: VK_ESCAPE, .. } = ev {
            self.overlay = Overlay::None;
            *cx.dirty = true;
            return true;
        }
        let mut close = false;
        let mut action = None;
        let mut confirm = None;
        let mut toggle = None;
        let mut reset = false;
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Menu { items, rect: r, hover, .. } => match *ev {
                Ev::Move { x, y } => {
                    let new = r.contains(x, y).then(|| ((y - r.y) * items.len() as i32 / r.h.max(1)) as usize);
                    if new != *hover {
                        *hover = new;
                        *cx.dirty = true;
                    }
                }
                Ev::Down { x, y, .. } => {
                    if r.contains(x, y) {
                        let i = ((y - r.y) * items.len() as i32 / r.h.max(1)) as usize;
                        if let Some(item) = items.get(i).filter(|m| m.enabled) {
                            action = Some(item.act);
                            close = true;
                        }
                    } else {
                        close = true;
                    }
                }
                _ => {}
            },
            Overlay::Confirm { targets, tree, hover, buttons, .. } => match *ev {
                Ev::Move { x, y } => {
                    let new = buttons.iter().position(|b| b.contains(x, y));
                    if new != *hover {
                        *hover = new;
                        *cx.dirty = true;
                    }
                }
                Ev::Down { x, y, right: false } => match buttons.iter().position(|b| b.contains(x, y)) {
                    Some(0) => {
                        confirm = Some((std::mem::take(targets), *tree));
                        close = true;
                    }
                    Some(_) => close = true,
                    None => {}
                },
                Ev::Key { vk: VK_RETURN, .. } => {
                    confirm = Some((std::mem::take(targets), *tree));
                    close = true;
                }
                _ => {}
            },
            Overlay::Columns { hover, cells, hover_btn, buttons, panel, scroll, max_scroll, list, line } => match *ev {
                Ev::Move { x, y } => {
                    let new = cells.iter().position(|(b, _)| list.contains(x, y) && b.contains(x, y));
                    let new_btn = buttons.iter().position(|b| b.contains(x, y));
                    if new != *hover || new_btn != *hover_btn {
                        *hover = new;
                        *hover_btn = new_btn;
                        *cx.dirty = true;
                    }
                }
                Ev::Down { x, y, right: false } => {
                    if let Some(i) = cells.iter().position(|(b, _)| list.contains(x, y) && b.contains(x, y)) {
                        toggle = Some(cells[i].1);
                    } else {
                        match buttons.iter().position(|b| b.contains(x, y)) {
                            Some(0) => reset = true,
                            Some(_) => close = true,
                            None => close = !panel.contains(x, y),
                        }
                    }
                }
                Ev::Wheel { dy, .. } if dy != 0.0 => {
                    let step = (dy * WHEEL_LINES * *line as f32) as i32;
                    *scroll = (*scroll - step).clamp(0, *max_scroll);
                    *cx.dirty = true;
                }
                _ => {}
            },
        }
        if close {
            self.overlay = Overlay::None;
            *cx.dirty = true;
        }
        if let Some(act) = action {
            self.run(act, cx);
        }
        if let Some((targets, tree)) = confirm {
            self.end_processes(targets, tree, cx);
        }
        if let Some(pick) = toggle {
            let s = &mut *cx.settings;
            match pick {
                Pick::Col(col) => {
                    let list = &mut s.columns;
                    match list.iter().position(|(c, _)| *c == col) {
                        // The name column always stays: grouping and the row layout depend on it.
                        Some(at) if col != Col::Name && list.len() > 1 => {
                            list.remove(at);
                        }
                        Some(_) => {}
                        None => list.push((col, columns::def(col).width)),
                    }
                }
                Pick::Item(item) => {
                    let list = &mut s.range_items;
                    match list.iter().position(|(i, _)| *i == item) {
                        Some(at) if item != RItem::Name => {
                            list.remove(at);
                        }
                        Some(_) => {}
                        None => list.push((item, item.default_width())),
                    }
                }
                Pick::Op(op) => {
                    let ops = s.range_ops ^ op.bit();
                    // At least one operation stays.
                    if ops != 0 {
                        s.range_ops = ops;
                    }
                }
            }
            self.apply_columns_changed(cx);
        }
        if reset {
            self.run(Act::ResetColumns, cx);
        }
        true
    }

    pub(super) fn on_mouse_move(&mut self, x: i32, y: i32, cx: &mut Cx) {
        match self.drag {
            Drag::None => self.update_hover(cx),
            Drag::Resize { idx, start_x, start_w } => {
                let scale = self.l.row_h as f32 / 28.0;
                let w = (start_w as f32 + (x - start_x) as f32 / scale).max(40.0) as u16;
                self.set_col_width(cx.settings, idx, w);
                *cx.dirty = true;
            }
            Drag::Split { start_y, start_h } => {
                let scale = self.l.row_h as f32 / 28.0;
                let h = (start_h as f32 + (y - start_y) as f32 / scale).clamp(120.0, 2000.0) as u16;
                if h != cx.settings.tm_graph_h {
                    cx.settings.tm_graph_h = h;
                    *cx.dirty = true;
                }
            }
            Drag::Header { idx, start_x, moving, .. } => {
                let moving = moving || (x - start_x).abs() > self.l.edge * 2;
                self.drag = Drag::Header { idx, start_x, x, moving };
                *cx.dirty = true;
            }
            Drag::VThumb { offset } => {
                if let Some(t) = self.vthumb() {
                    let track = self.l.body.h - self.hbar_h() - t.h;
                    let ratio = (y - offset - self.l.body.y) as f32 / track.max(1) as f32;
                    self.scroll_y = (ratio * self.max_scroll_y()).clamp(0.0, self.max_scroll_y());
                    self.target_y = self.scroll_y;
                    *cx.dirty = true;
                }
            }
            Drag::HThumb { offset } => {
                if let Some(t) = self.hthumb() {
                    let track = self.l.table.w - self.l.bar - t.w;
                    let ratio = (x - offset - self.l.table.x) as f32 / track.max(1) as f32;
                    self.scroll_x = (ratio * self.max_scroll_x() as f32) as i32;
                    self.clamp_scroll();
                    *cx.dirty = true;
                }
            }
        }
    }

    pub(super) fn on_mouse_down(&mut self, x: i32, y: i32, right: bool, cx: &mut Cx) {
        self.fling = (0.0, 0.0);
        self.mouse = (x, y);
        self.update_hover(cx);
        if !right {
            if self.l.graph_arrow.contains(x, y) {
                let b = self.l.graph_btn;
                self.tl.open_menu(b.x, b.b() + 4, cx.settings);
                *cx.dirty = true;
                return;
            }
            if self.l.graph_btn.contains(x, y) {
                let out = self.tl.toggle(cx);
                return self.after_timeline(out, cx);
            }
            if self.hover_split && cx.settings.tm_enabled {
                self.drag = Drag::Split { start_y: y, start_h: cx.settings.tm_graph_h };
                return;
            }
            if self.l.end_btn.contains(x, y) && !self.end_targets().is_empty() {
                self.confirm_end(false);
                *cx.dirty = true;
                return;
            }
            if self.l.aim_btn.contains(x, y) {
                if !self.aiming {
                    self.aiming = true;
                    cx.cmds.push(Cmd::Aim);
                }
                *cx.dirty = true;
                return;
            }
            if self.l.col_btn.contains(x, y) {
                self.open_columns();
                *cx.dirty = true;
                return;
            }
            if self.l.view_btn.contains(x, y) {
                self.open_view_menu(cx.settings.view);
                *cx.dirty = true;
                return;
            }
            if self.mode != ViewMode::Flat && self.l.expand_btn.contains(x, y) {
                self.expand_all();
                *cx.dirty = true;
                return;
            }
            if self.mode != ViewMode::Flat && self.l.collapse_btn.contains(x, y) {
                self.collapse_all();
                *cx.dirty = true;
                return;
            }
            if self.hover_vbar {
                let Some(t) = self.vthumb() else { return };
                if !t.contains(x, y) {
                    let page = (self.l.body.h - self.hbar_h()) as f32;
                    let dir = if y < t.y { -1.0 } else { 1.0 };
                    self.target_y = (self.target_y + dir * page).clamp(0.0, self.max_scroll_y());
                    *cx.dirty = true;
                    return;
                }
                self.drag = Drag::VThumb { offset: y - t.y };
                return;
            }
            if self.hover_hbar {
                let Some(t) = self.hthumb() else { return };
                if t.contains(x, y) {
                    self.drag = Drag::HThumb { offset: x - t.x };
                }
                return;
            }
        }
        if let Some(i) = self.hover_header {
            if right {
                if let Some(b) = self.l.cols.get(i) {
                    self.open_header_menu(x, y, b.col, cx.settings);
                    *cx.dirty = true;
                }
            } else if self.hover_edge {
                let w = self.col_width(cx.settings, i);
                self.drag = Drag::Resize { idx: i, start_x: x, start_w: w };
            } else {
                self.drag = Drag::Header { idx: i, start_x: x, x, moving: false };
            }
            return;
        }
        if let Some(i) = self.hover_row {
            let row = self.view[i];
            let sel = self.row_sel(row);
            if self.selected.as_ref() != Some(&sel) {
                self.selected = Some(sel);
                *cx.dirty = true;
            }
            // An ended process can no longer be acted on.
            if right && !matches!(row, Row::Ghost(_)) {
                self.open_row_menu(x, y);
                *cx.dirty = true;
            } else if !right && self.hover_chevron {
                match row {
                    Row::Group(g) => self.toggle_group(g),
                    Row::Node(n) => self.toggle_node(n),
                    _ => {}
                }
                *cx.dirty = true;
            }
        } else if self.l.body.contains(x, y) && !right && self.selected.take().is_some() {
            *cx.dirty = true;
        }
    }

    pub(super) fn on_mouse_up(&mut self, x: i32, cx: &mut Cx) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Header { idx, moving: false, .. } => {
                let s = &mut *cx.settings;
                match self.l.cols.get(idx).map(|b| b.col) {
                    Some(AnyCol::P(col)) if s.sort == col => s.descending = !s.descending,
                    Some(AnyCol::P(col)) => {
                        s.sort = col;
                        s.descending = columns::default_descending(col);
                    }
                    Some(AnyCol::R(col)) if s.range_sort == col => s.range_desc = !s.range_desc,
                    Some(AnyCol::R(col)) => {
                        s.range_sort = col;
                        s.range_desc = col.default_descending();
                    }
                    None => return,
                }
                self.resort(cx.settings);
                *cx.dirty = true;
            }
            Drag::Header { idx, moving: true, .. } => {
                let target = self.insertion_index(x);
                self.move_col(cx.settings, idx, target);
                *cx.dirty = true;
            }
            Drag::None => {}
            _ => *cx.dirty = true,
        }
    }

    pub(super) fn insertion_index(&self, x: i32) -> usize {
        let content_x = x - self.l.table.x + self.scroll_x;
        self.l
            .cols
            .iter()
            .position(|c| content_x < c.x + c.w / 2)
            .unwrap_or(self.l.cols.len())
    }

    pub(super) fn on_key(&mut self, vk: u16, ctrl: bool, cx: &mut Cx) {
        let page = (self.l.body.h / self.l.row_h.max(1)).max(1) as isize;
        let last = self.view.len() as isize - 1;
        let current = self.selected_index().map(|i| i as isize);
        let next = match vk {
            VK_UP => Some(current.map_or(0, |i| i - 1)),
            VK_DOWN => Some(current.map_or(0, |i| i + 1)),
            VK_PRIOR => Some(current.map_or(0, |i| i - page)),
            VK_NEXT => Some(current.map_or(0, |i| i + page)),
            VK_HOME => Some(0),
            VK_END => Some(last),
            _ => None,
        };
        if let Some(n) = next {
            if last >= 0 {
                self.select_index(n.clamp(0, last) as usize);
                *cx.dirty = true;
            }
            return;
        }
        match vk {
            VK_DELETE => {
                self.confirm_end(false);
                *cx.dirty = true;
            }
            VK_LEFT | VK_RIGHT => {
                let Some(i) = self.selected_index() else { return };
                match self.view[i] {
                    Row::Group(g) => {
                        let open = self.expanded.contains(&self.groups[g as usize].id);
                        if open == (vk == VK_LEFT) {
                            self.toggle_group(g);
                            *cx.dirty = true;
                        }
                    }
                    Row::Child(_) if vk == VK_LEFT => {
                        if let Some(parent) = self.view[..i].iter().rposition(|r| matches!(r, Row::Group(_))) {
                            self.select_index(parent);
                            *cx.dirty = true;
                        }
                    }
                    Row::Node(n) => {
                        let m = self.meta[i];
                        if m.branch && m.open != (vk == VK_RIGHT) {
                            self.toggle_node(n);
                            *cx.dirty = true;
                        } else if vk == VK_LEFT && m.depth > 0 {
                            if let Some(parent) = self.meta[..i].iter().rposition(|p| p.depth < m.depth) {
                                self.select_index(parent);
                                *cx.dirty = true;
                            }
                        } else if vk == VK_RIGHT && m.open && i + 1 < self.view.len() {
                            self.select_index(i + 1);
                            *cx.dirty = true;
                        }
                    }
                    _ => {}
                }
            }
            VK_C if ctrl => self.run(Act::CopyRow, cx),
            VK_F5 => cx.shared.poke(),
            _ => {}
        }
    }
}
