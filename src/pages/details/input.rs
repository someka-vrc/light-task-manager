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
                && l.cols.iter().find(|c| c.col == Col::Name).is_some_and(|c| {
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
        self.regroup(cx.settings);
        *cx.dirty = true;
    }

    /// Clipboard text of a process row: the shown columns, or every property when the
    /// copy scope is "all". Properties that need a column to be shown are "-" while hidden.
    pub(super) fn copy_text(&self, p: &Proc, members: Option<usize>, s: &Settings) -> String {
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
        let item = |label, act, enabled| MenuItem { label, act, enabled, checked: None };
        self.overlay = Overlay::Menu {
            origin: (x, y),
            items: vec![
                item(tr().common.end_task, Act::EndTask, true),
                item(tr().details.end_tree, Act::EndTree, true),
                item(tr().common.open_location, Act::OpenLocation, has_path),
                item(tr().common.properties, Act::Properties, has_path),
                item(tr().common.copy_row, Act::CopyRow, true),
            ],
            rect: Rect::default(),
            hover: None,
        };
    }

    pub(super) fn open_header_menu(&mut self, x: i32, y: i32, col: Col, can_hide: bool) {
        self.overlay = Overlay::Menu {
            origin: (x, y),
            items: vec![
                MenuItem { label: tr().details.hide_column, act: Act::HideColumn(col), enabled: can_hide, checked: None },
                MenuItem { label: tr().details.pick_columns_menu, act: Act::PickColumns, enabled: true, checked: None },
                MenuItem { label: tr().details.reset_columns, act: Act::ResetColumns, enabled: true, checked: None },
            ],
            rect: Rect::default(),
            hover: None,
        };
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

    pub(super) fn confirm_end(&mut self, tree: bool) {
        let chosen = if let Some(g) = self.selected_group() {
            let procs = &self.snap.procs;
            let mut keys: Vec<(u32, i64)> = g.members.iter().map(|m| procs[*m as usize].key()).collect();
            // Oldest first, so that ending in reverse reaches children before parents.
            keys.sort_by_key(|k| k.1);
            Some(((tr().details.n_processes)(&g.summary.stat.name, keys.len()), keys))
        } else {
            self.selected_proc().map(|p| (p.stat.name.clone(), vec![p.key()]))
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
                let members = self.selected_group().map(|g| g.members.len());
                if let Some(p) = self.selected_proc() {
                    let text = self.copy_text(p, members, cx.settings);
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
            Act::ResetColumns => {
                cx.settings.columns = columns::defaults();
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
                for p in &self.snap.procs {
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
        if let Some(col) = toggle {
            let list = &mut cx.settings.columns;
            match list.iter().position(|(c, _)| *c == col) {
                // The name column always stays: grouping and the row layout depend on it.
                Some(at) if col != Col::Name && list.len() > 1 => {
                    list.remove(at);
                }
                Some(_) => {}
                None => list.push((col, columns::def(col).width)),
            }
            self.apply_columns_changed(cx);
        }
        if reset {
            cx.settings.columns = columns::defaults();
            self.apply_columns_changed(cx);
        }
        true
    }

    pub(super) fn on_mouse_move(&mut self, x: i32, y: i32, cx: &mut Cx) {
        match self.drag {
            Drag::None => self.update_hover(cx),
            Drag::Resize { idx, start_x, start_w } => {
                let scale = self.l.row_h as f32 / 28.0;
                let w = (start_w as f32 + (x - start_x) as f32 / scale).max(40.0) as u16;
                if let Some(c) = cx.settings.columns.get_mut(idx) {
                    c.1 = w;
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
            if self.l.end_btn.contains(x, y) && self.selected_proc().is_some() {
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
                let col = cx.settings.columns[i].0;
                let can_hide = col != Col::Name && cx.settings.columns.len() > 1;
                self.open_header_menu(x, y, col, can_hide);
                *cx.dirty = true;
            } else if self.hover_edge {
                let w = cx.settings.columns.get(i).map_or(100, |c| c.1);
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
                if let Some((col, _)) = cx.settings.columns.get(idx).copied() {
                    if cx.settings.sort == col {
                        cx.settings.descending = !cx.settings.descending;
                    } else {
                        cx.settings.sort = col;
                        cx.settings.descending = columns::default_descending(col);
                    }
                    self.resort(cx.settings);
                    *cx.dirty = true;
                }
            }
            Drag::Header { idx, moving: true, .. } => {
                let target = self.insertion_index(x);
                let list = &mut cx.settings.columns;
                if idx < list.len() {
                    let item = list.remove(idx);
                    let at = if target > idx { target - 1 } else { target };
                    list.insert(at.min(list.len()), item);
                }
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
