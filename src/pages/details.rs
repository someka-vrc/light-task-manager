//! The details page: a virtualized process table.

use super::{glyph, icons, Cmd, Cursor, Cx, Ev, Nav, Page};
use crate::actions;
use crate::columns::{self, Col, ALL};
use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::model::{Proc, Snapshot};
use crate::settings::Settings;
use crate::theme::THEME;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_DELETE: u16 = 0x2E;
const VK_C: u16 = 0x43;
const VK_F5: u16 = 0x74;

const WHEEL_LINES: f32 = 3.0;
const SCROLL_EASE_SECONDS: f32 = 0.04;
const PICKER_COLUMNS: i32 = 3;

#[derive(Clone, Copy, PartialEq)]
enum Act {
    EndTask,
    EndTree,
    OpenLocation,
    Properties,
    CopyRow,
    PickColumns,
    ResetColumns,
}

struct MenuItem {
    label: &'static str,
    act: Act,
    enabled: bool,
}

enum Overlay {
    None,
    Menu {
        origin: (i32, i32),
        items: Vec<MenuItem>,
        rect: Rect,
        hover: Option<usize>,
    },
    Confirm {
        key: (u32, i64),
        name: String,
        tree: bool,
        hover: Option<usize>,
        buttons: [Rect; 2],
    },
    Columns {
        hover: Option<usize>,
        cells: Vec<Rect>,
        hover_btn: Option<usize>,
        buttons: [Rect; 2],
        panel: Rect,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    Resize { idx: usize, start_x: i32, start_w: u16 },
    Header { idx: usize, start_x: i32, x: i32, moving: bool },
    VThumb { offset: i32 },
    HThumb { offset: i32 },
}

#[derive(Clone, Copy)]
struct ColBox {
    col: Col,
    x: i32,
    w: i32,
}

#[derive(Default)]
struct Layout {
    area: Rect,
    table: Rect,
    body: Rect,
    row_h: i32,
    header_h: i32,
    pad: i32,
    edge: i32,
    bar: i32,
    cols: Vec<ColBox>,
    content_w: i32,
    end_btn: Rect,
    col_btn: Rect,
}

pub struct DetailsPage {
    snap: Arc<Snapshot>,
    sorted: Vec<u32>,
    view: Vec<u32>,
    filter: Vec<String>,
    haystacks: Vec<String>,
    selected: Option<(u32, i64)>,
    scroll_y: f32,
    target_y: f32,
    scroll_x: i32,
    hover_row: Option<usize>,
    hover_header: Option<usize>,
    hover_edge: bool,
    hover_vbar: bool,
    hover_hbar: bool,
    hover_end: bool,
    hover_col: bool,
    mouse: (i32, i32),
    drag: Drag,
    overlay: Overlay,
    l: Layout,
    cell: String,
}

impl DetailsPage {
    pub fn new() -> Self {
        Self {
            snap: Arc::new(Snapshot::default()),
            sorted: Vec::new(),
            view: Vec::new(),
            filter: Vec::new(),
            haystacks: Vec::new(),
            selected: None,
            scroll_y: 0.0,
            target_y: 0.0,
            scroll_x: 0,
            hover_row: None,
            hover_header: None,
            hover_edge: false,
            hover_vbar: false,
            hover_hbar: false,
            hover_end: false,
            hover_col: false,
            mouse: (-1, -1),
            drag: Drag::None,
            overlay: Overlay::None,
            l: Layout::default(),
            cell: String::with_capacity(256),
        }
    }

    fn resort(&mut self, settings: &Settings) {
        let procs = &self.snap.procs;
        let (col, desc) = (settings.sort, settings.descending);
        self.sorted.clear();
        self.sorted.extend(0..procs.len() as u32);
        self.sorted.sort_unstable_by(|a, b| {
            let (pa, pb) = (&procs[*a as usize], &procs[*b as usize]);
            let order = columns::compare(col, pa, pb);
            let order = if desc { order.reverse() } else { order };
            order.then(pa.pid.cmp(&pb.pid))
        });
        self.refilter();
    }

    fn refilter(&mut self) {
        self.view.clear();
        if self.filter.is_empty() {
            self.view.extend_from_slice(&self.sorted);
        } else {
            if self.haystacks.len() != self.snap.procs.len() {
                self.haystacks = self.snap.procs.iter().map(haystack).collect();
            }
            let (hay, terms) = (&self.haystacks, &self.filter);
            self.view.extend(
                self.sorted
                    .iter()
                    .filter(|i| terms.iter().all(|t| hay[**i as usize].contains(t.as_str()))),
            );
        }
        self.clamp_scroll();
    }

    fn max_scroll_y(&self) -> f32 {
        (self.view.len() as i32 * self.l.row_h - self.l.body.h + self.hbar_h()).max(0) as f32
    }

    fn hbar_h(&self) -> i32 {
        if self.l.content_w > self.l.table.w {
            self.l.bar
        } else {
            0
        }
    }

    fn max_scroll_x(&self) -> i32 {
        (self.l.content_w - self.l.table.w).max(0)
    }

    fn clamp_scroll(&mut self) {
        let max = self.max_scroll_y();
        self.target_y = self.target_y.clamp(0.0, max);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
        self.scroll_x = self.scroll_x.clamp(0, self.max_scroll_x());
    }

    fn selected_index(&self) -> Option<usize> {
        let key = self.selected?;
        self.view
            .iter()
            .position(|i| self.snap.procs[*i as usize].key() == key)
    }

    fn selected_proc(&self) -> Option<&Proc> {
        let key = self.selected?;
        self.snap.procs.iter().find(|p| p.key() == key)
    }

    fn select_index(&mut self, index: usize) {
        if let Some(i) = self.view.get(index) {
            self.selected = Some(self.snap.procs[*i as usize].key());
            let (top, rh) = (index as i32 * self.l.row_h, self.l.row_h);
            let visible = self.l.body.h - self.hbar_h();
            let mut y = self.target_y;
            if (top as f32) < y {
                y = top as f32;
            } else if ((top + rh) as f32) > y + visible as f32 {
                y = (top + rh - visible) as f32;
            }
            self.target_y = y;
            self.scroll_y = y;
        }
    }

    fn update_hover(&mut self, cx: &mut Cx) {
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
        let (mut header, mut edge) = (None, false);
        let in_header = y >= l.table.y && y < l.body.y && x >= l.table.x && x < l.table.r();
        if in_header {
            let cx_x = x - l.table.x + self.scroll_x;
            for (i, c) in l.cols.iter().enumerate() {
                if (cx_x - (c.x + c.w)).abs() <= l.edge {
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
        if row != self.hover_row
            || header != self.hover_header
            || edge != self.hover_edge
            || vbar != self.hover_vbar
            || hbar != self.hover_hbar
            || end != self.hover_end
            || col != self.hover_col
        {
            self.hover_row = row;
            self.hover_header = header;
            self.hover_edge = edge;
            self.hover_vbar = vbar;
            self.hover_hbar = hbar;
            self.hover_end = end;
            self.hover_col = col;
            *cx.dirty = true;
        }
    }

    fn vthumb(&self) -> Option<Rect> {
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

    fn hthumb(&self) -> Option<Rect> {
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

    fn apply_columns_changed(&mut self, cx: &mut Cx) {
        let want = cx.settings.columns.iter().any(|(c, _)| columns::needs_gui_counts(*c));
        cx.shared.want_gui_counts.store(want, Ordering::Relaxed);
        if want {
            cx.shared.poke();
        }
        *cx.dirty = true;
    }

    fn open_row_menu(&mut self, x: i32, y: i32) {
        let has_path = self.selected_proc().is_some_and(|p| !p.stat.path().is_empty());
        let item = |label, act, enabled| MenuItem { label, act, enabled };
        self.overlay = Overlay::Menu {
            origin: (x, y),
            items: vec![
                item("タスクの終了", Act::EndTask, true),
                item("プロセス ツリーの終了", Act::EndTree, true),
                item("ファイルの場所を開く", Act::OpenLocation, has_path),
                item("プロパティ", Act::Properties, has_path),
                item("行をコピー", Act::CopyRow, true),
            ],
            rect: Rect::default(),
            hover: None,
        };
    }

    fn open_header_menu(&mut self, x: i32, y: i32) {
        self.overlay = Overlay::Menu {
            origin: (x, y),
            items: vec![
                MenuItem { label: "列の選択...", act: Act::PickColumns, enabled: true },
                MenuItem { label: "既定の列に戻す", act: Act::ResetColumns, enabled: true },
            ],
            rect: Rect::default(),
            hover: None,
        };
    }

    fn open_columns(&mut self) {
        self.overlay = Overlay::Columns {
            hover: None,
            cells: Vec::new(),
            hover_btn: None,
            buttons: [Rect::default(); 2],
            panel: Rect::default(),
        };
    }

    fn confirm_end(&mut self, tree: bool) {
        if let Some(p) = self.selected_proc() {
            self.overlay = Overlay::Confirm {
                key: p.key(),
                name: p.stat.name.clone(),
                tree,
                hover: None,
                buttons: [Rect::default(); 2],
            };
        }
    }

    fn run(&mut self, act: Act, cx: &mut Cx) {
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
                if let Some(p) = self.selected_proc() {
                    let mut line = String::new();
                    for (i, (col, _)) in cx.settings.columns.iter().enumerate() {
                        if i > 0 {
                            line.push('\t');
                        }
                        columns::cell(*col, p, &mut line);
                    }
                    actions::set_clipboard(&line);
                }
            }
            Act::PickColumns => self.open_columns(),
            Act::ResetColumns => {
                cx.settings.columns = columns::defaults();
                self.apply_columns_changed(cx);
            }
        }
        *cx.dirty = true;
    }

    fn end_processes(&mut self, key: (u32, i64), tree: bool, cx: &mut Cx) {
        let mut targets = vec![key];
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
            let hint = if denied { "アクセスが拒否されました。設定から管理者として再起動できます。" } else { "プロセスは既に終了している可能性があります。" };
            cx.cmds.push(Cmd::Toast(format!("{failed} 個のプロセスを終了できませんでした。{hint}")));
        }
        cx.shared.poke();
    }

    fn overlay_event(&mut self, ev: &Ev, cx: &mut Cx) -> bool {
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
            Overlay::Confirm { key, tree, hover, buttons, .. } => match *ev {
                Ev::Move { x, y } => {
                    let new = buttons.iter().position(|b| b.contains(x, y));
                    if new != *hover {
                        *hover = new;
                        *cx.dirty = true;
                    }
                }
                Ev::Down { x, y, right: false } => match buttons.iter().position(|b| b.contains(x, y)) {
                    Some(0) => {
                        confirm = Some((*key, *tree));
                        close = true;
                    }
                    Some(_) => close = true,
                    None => {}
                },
                Ev::Key { vk: VK_RETURN, .. } => {
                    confirm = Some((*key, *tree));
                    close = true;
                }
                _ => {}
            },
            Overlay::Columns { hover, cells, hover_btn, buttons, panel } => match *ev {
                Ev::Move { x, y } => {
                    let new = cells.iter().position(|b| b.contains(x, y));
                    let new_btn = buttons.iter().position(|b| b.contains(x, y));
                    if new != *hover || new_btn != *hover_btn {
                        *hover = new;
                        *hover_btn = new_btn;
                        *cx.dirty = true;
                    }
                }
                Ev::Down { x, y, right: false } => {
                    if let Some(i) = cells.iter().position(|b| b.contains(x, y)) {
                        toggle = Some(i);
                    } else {
                        match buttons.iter().position(|b| b.contains(x, y)) {
                            Some(0) => reset = true,
                            Some(_) => close = true,
                            None => close = !panel.contains(x, y),
                        }
                    }
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
        if let Some((key, tree)) = confirm {
            self.end_processes(key, tree, cx);
        }
        if let Some(i) = toggle {
            let col = ALL[i].id;
            let list = &mut cx.settings.columns;
            match list.iter().position(|(c, _)| *c == col) {
                Some(at) if list.len() > 1 => {
                    list.remove(at);
                }
                Some(_) => {}
                None => list.push((col, ALL[i].width)),
            }
            self.apply_columns_changed(cx);
        }
        if reset {
            cx.settings.columns = columns::defaults();
            self.apply_columns_changed(cx);
        }
        true
    }

    fn on_mouse_move(&mut self, x: i32, y: i32, cx: &mut Cx) {
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

    fn on_mouse_down(&mut self, x: i32, y: i32, right: bool, cx: &mut Cx) {
        self.mouse = (x, y);
        self.update_hover(cx);
        if !right {
            if self.l.end_btn.contains(x, y) && self.selected_proc().is_some() {
                self.confirm_end(false);
                *cx.dirty = true;
                return;
            }
            if self.l.col_btn.contains(x, y) {
                self.open_columns();
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
                self.open_header_menu(x, y);
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
            let key = self.snap.procs[self.view[i] as usize].key();
            if self.selected != Some(key) {
                self.selected = Some(key);
                *cx.dirty = true;
            }
            if right {
                self.open_row_menu(x, y);
                *cx.dirty = true;
            }
        } else if self.l.body.contains(x, y) && !right && self.selected.take().is_some() {
            *cx.dirty = true;
        }
    }

    fn on_mouse_up(&mut self, x: i32, cx: &mut Cx) {
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

    fn insertion_index(&self, x: i32) -> usize {
        let content_x = x - self.l.table.x + self.scroll_x;
        self.l
            .cols
            .iter()
            .position(|c| content_x < c.x + c.w / 2)
            .unwrap_or(self.l.cols.len())
    }

    fn on_key(&mut self, vk: u16, ctrl: bool, cx: &mut Cx) {
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
            VK_C if ctrl => self.run(Act::CopyRow, cx),
            VK_F5 => cx.shared.poke(),
            _ => {}
        }
    }

    fn layout(&mut self, c: &Canvas, area: Rect, settings: &Settings) {
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
        let end_w = c.px(158);
        let right = area.r() - c.px(16);
        let btn_h = c.px(34);
        let btn_y = area.y + (toolbar - btn_h) / 2;
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
            col_btn: rect(right - end_w - c.px(8) - btn_h, btn_y, btn_h, btn_h),
        };
        if let Some(last) = self.l.cols.last_mut() {
            if x < table.w {
                last.w += table.w - x;
                self.l.content_w = table.w;
            }
        }
        self.clamp_scroll();
    }

    fn paint_toolbar(&mut self, c: &mut Canvas) {
        let t = THEME;
        let l = &self.l;
        let area = l.area;
        let toolbar = rect(area.x + c.px(20), area.y, c.px(80), c.px(60));
        c.text("詳細", Font::Title, toolbar, t.text, Align::Left);

        let shown = self.view.len();
        let total = self.snap.procs.len();
        let gib = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
        let info = if shown == total {
            format!(
                "{total} プロセス   CPU {:.0}%   メモリ {:.1} / {:.1} GB",
                self.snap.cpu_total,
                gib(self.snap.mem_used),
                gib(self.snap.mem_total)
            )
        } else {
            format!("{shown} / {total} プロセス")
        };
        let info_x = area.x + c.px(84);
        let info_w = l.col_btn.x - info_x - c.px(12);
        c.text(&info, Font::Small, rect(info_x, area.y + c.px(3), info_w, c.px(60)), t.text_dim, Align::Left);

        let r = c.px(6) as f32;
        let col_btn = l.col_btn;
        c.round(col_btn, r, if self.hover_col { t.hover } else { t.content }, 255);
        c.text(&glyph(icons::COLUMNS), Font::Icon, col_btn, t.text, Align::Center);

        let can_end = self.selected.is_some();
        let end_btn = l.end_btn;
        let fg = if can_end { t.text } else { t.text_faint };
        if can_end && self.hover_end {
            c.round(end_btn, r, t.hover, 255);
        }
        c.stroke(end_btn, r, 1.0, t.border, 255);
        let icon_x = end_btn.x + c.px(12);
        c.text(&glyph(icons::BLOCK), Font::IconSmall, rect(icon_x, end_btn.y, c.px(16), end_btn.h), fg, Align::Left);
        c.text("タスクを終了する", Font::Body, rect(icon_x + c.px(20), end_btn.y, end_btn.w - c.px(40), end_btn.h), fg, Align::Left);
    }

    fn paint_header(&mut self, c: &mut Canvas, settings: &Settings) {
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
            c.text(d.label, Font::Small, text_area, color, align);
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

    fn paint_rows(&mut self, c: &mut Canvas) {
        let t = THEME;
        let (body, rh, pad, table) = (self.l.body, self.l.row_h, self.l.pad, self.l.table);
        c.set_clip(body);
        let scroll = self.scroll_y.round() as i32;
        let first = (scroll / rh.max(1)) as usize;
        let last = (((scroll + body.h) / rh.max(1)) as usize + 1).min(self.view.len());
        let icon_size = c.px(16);
        let radius = c.px(5) as f32;
        let selected = self.selected;
        let mut buf = std::mem::take(&mut self.cell);
        for i in first..last {
            let p = &self.snap.procs[self.view[i] as usize];
            let y = body.y + i as i32 * rh - scroll;
            let row = rect(table.x, y, table.w, rh);
            let is_selected = selected == Some(p.key());
            if is_selected {
                c.round(row.inset(c.px(2), c.px(1)), radius, t.selection, 255);
                c.round(rect(row.x + c.px(2), y + c.px(7), c.px(3), rh - c.px(14)), 1.5, t.accent, 255);
            } else if self.hover_row == Some(i) {
                c.round(row.inset(c.px(2), c.px(1)), radius, t.hover, 255);
            }
            for b in &self.l.cols {
                let x = table.x + b.x - self.scroll_x;
                if x + b.w < table.x || x > table.r() {
                    continue;
                }
                let mut cell = rect(x + pad, y, b.w - pad * 2, rh);
                let d = columns::def(b.col);
                let mut color = t.text;
                if b.col == Col::Name {
                    let top = y + (rh - icon_size) / 2;
                    match p.stat.image.as_ref().and_then(|m| m.icon()) {
                        Some(icon) => c.icon(icon, cell.x, top, icon_size),
                        None => c.round(rect(cell.x + 1, top + 1, icon_size - 2, icon_size - 2), 3.0, t.border, 255),
                    }
                    let shift = icon_size + c.px(8);
                    cell.x += shift;
                    cell.w -= shift;
                } else if !is_selected {
                    color = t.text_dim;
                    if b.col == Col::Cpu && p.cpu.round() >= 1.0 && p.pid != 0 {
                        color = t.text;
                    } else if b.col == Col::Status && p.status == crate::model::Status::NotResponding {
                        color = t.warn;
                    }
                }
                buf.clear();
                columns::cell(b.col, p, &mut buf);
                let align = if d.right { Align::Right } else { Align::Left };
                c.text(&buf, Font::Body, cell, color, align);
            }
        }
        self.cell = buf;

        if self.view.is_empty() && !self.snap.procs.is_empty() {
            c.text("一致するプロセスはありません", Font::Body, body, t.text_dim, Align::Center);
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
                c.text(columns::def(b.col).label, Font::Small, ghost.inset(pad, 0), t.text, Align::Left);
            }
            c.reset_clip();
        }
    }

    fn panel(c: &mut Canvas, r: Rect) {
        let t = THEME;
        let radius = c.px(8) as f32;
        for i in (1..=4).rev() {
            let grow = c.px(i * 3);
            c.round(rect(r.x - grow, r.y - grow + c.px(2), r.w + grow * 2, r.h + grow * 2), radius + grow as f32, 0, 22);
        }
        c.round(r, radius, t.popup, 255);
        c.stroke(r, radius, 1.0, t.border, 255);
    }

    fn paint_overlay(&mut self, c: &mut Canvas, settings: &Settings) {
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
                    c.text(item.label, Font::Body, row.inset(c.px(14), 0), color, Align::Left);
                }
            }
            Overlay::Confirm { name, tree, hover, buttons, .. } => {
                c.fill_alpha(area, 0, 110);
                let (w, h) = (c.px(440), c.px(192));
                let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
                Self::panel(c, r);
                let inner = r.inset(c.px(22), 0);
                let title = if *tree { "プロセス ツリーを終了しますか?" } else { "タスクを終了しますか?" };
                c.text(title, Font::Semi, rect(inner.x, r.y + c.px(18), inner.w, c.px(26)), t.text, Align::Left);
                c.text(name, Font::Body, rect(inner.x, r.y + c.px(52), inner.w, c.px(24)), t.accent, Align::Left);
                let note = if *tree {
                    "このプロセスと子プロセスをすべて終了します。"
                } else {
                    "このプロセスを終了します。"
                };
                c.text(note, Font::Small, rect(inner.x, r.y + c.px(80), inner.w, c.px(22)), t.text_dim, Align::Left);
                c.text("保存されていないデータは失われます。", Font::Small, rect(inner.x, r.y + c.px(100), inner.w, c.px(22)), t.text_dim, Align::Left);
                let (bw, bh) = (c.px(120), c.px(36));
                let by = r.b() - bh - c.px(16);
                buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
                buttons[0] = rect(buttons[1].x - bw - c.px(8), by, bw, bh);
                let radius = c.px(6) as f32;
                c.round(buttons[0], radius, t.danger, if *hover == Some(0) { 255 } else { 225 });
                c.text("終了する", Font::Body, buttons[0], 0xFFFFFF, Align::Center);
                c.round(buttons[1], radius, if *hover == Some(1) { t.pressed } else { t.hover }, 255);
                c.text("キャンセル", Font::Body, buttons[1], t.text, Align::Center);
            }
            Overlay::Columns { hover, cells, hover_btn, buttons, panel } => {
                c.fill_alpha(area, 0, 110);
                let rows = (ALL.len() as i32 + PICKER_COLUMNS - 1) / PICKER_COLUMNS;
                let cell_h = c.px(30);
                let w = c.px(760).min(area.w - c.px(32));
                let h = c.px(60) + rows * cell_h + c.px(64);
                let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
                *panel = r;
                Self::panel(c, r);
                c.text("列の選択", Font::Semi, rect(r.x + c.px(22), r.y + c.px(16), r.w, c.px(28)), t.text, Align::Left);
                let cell_w = (w - c.px(32)) / PICKER_COLUMNS;
                cells.clear();
                for (i, d) in ALL.iter().enumerate() {
                    let (cx, cy) = (i as i32 % PICKER_COLUMNS, i as i32 / PICKER_COLUMNS);
                    let cell = rect(r.x + c.px(16) + cx * cell_w, r.y + c.px(56) + cy * cell_h, cell_w, cell_h);
                    cells.push(cell);
                    if *hover == Some(i) {
                        c.round(cell.inset(c.px(2), c.px(1)), c.px(4) as f32, t.pressed, 255);
                    }
                    let box_size = c.px(16);
                    let bx = rect(cell.x + c.px(8), cell.y + (cell_h - box_size) / 2, box_size, box_size);
                    let on = settings.columns.iter().any(|(col, _)| *col == d.id);
                    if on {
                        c.round(bx, c.px(3) as f32, t.accent, 255);
                        c.text(&glyph(icons::CHECK), Font::IconSmall, bx, t.on_accent, Align::Center);
                    } else {
                        c.stroke(bx, c.px(3) as f32, 1.0, t.text_faint, 255);
                    }
                    let label = rect(bx.r() + c.px(8), cell.y, cell.r() - bx.r() - c.px(12), cell_h);
                    c.text(d.full, Font::Small, label, t.text, Align::Left);
                }
                let (bw, bh) = (c.px(124), c.px(36));
                let by = r.b() - bh - c.px(14);
                buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
                buttons[0] = rect(buttons[1].x - bw - c.px(8), by, bw, bh);
                let radius = c.px(6) as f32;
                c.round(buttons[0], radius, if *hover_btn == Some(0) { t.pressed } else { t.hover }, 255);
                c.text("既定に戻す", Font::Body, buttons[0], t.text, Align::Center);
                c.round(buttons[1], radius, t.accent, if *hover_btn == Some(1) { 255 } else { 230 });
                c.text("閉じる", Font::Body, buttons[1], t.on_accent, Align::Center);
            }
        }
    }

    /// Benchmark and screenshot support.
    pub fn scroll_to(&mut self, y: f32) {
        self.scroll_y = y.clamp(0.0, self.max_scroll_y());
        self.target_y = self.scroll_y;
    }

    pub fn max_scroll(&self) -> f32 {
        self.max_scroll_y()
    }
}

fn haystack(p: &Proc) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        p.stat.name,
        p.pid,
        p.stat.user,
        p.stat.description()
    )
    .to_lowercase()
}

impl Page for DetailsPage {
    fn nav(&self) -> Nav {
        Nav {
            icon: icons::LIST,
            label: "詳細",
            bottom: false,
        }
    }

    fn wants_filter(&self) -> bool {
        true
    }

    fn set_filter(&mut self, text: &str, cx: &mut Cx) {
        self.filter = text.to_lowercase().split_whitespace().map(String::from).collect();
        self.refilter();
        *cx.dirty = true;
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        self.snap = snap.clone();
        self.haystacks.clear();
        self.resort(cx.settings);
        if self.selected.is_some() && self.selected_proc().is_none() {
            self.selected = None;
        }
        self.hover_row = None;
        self.update_hover(cx);
        *cx.dirty = true;
    }

    fn on_enriched(&mut self, cx: &mut Cx) {
        self.haystacks.clear();
        if !self.filter.is_empty() {
            self.refilter();
        }
        *cx.dirty = true;
    }

    fn animating(&self) -> bool {
        self.scroll_y != self.target_y
    }

    fn cursor(&self) -> Cursor {
        if self.hover_edge || matches!(self.drag, Drag::Resize { .. }) {
            Cursor::SizeWE
        } else {
            Cursor::Arrow
        }
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Ev::Move { x, y } = *ev {
            self.mouse = (x, y);
        }
        if !matches!(ev, Ev::Tick { .. }) && self.overlay_event(ev, cx) {
            return;
        }
        match *ev {
            Ev::Move { x, y } => self.on_mouse_move(x, y, cx),
            Ev::Leave => {
                self.mouse = (-1, -1);
                self.update_hover(cx);
            }
            Ev::Down { x, y, right } => self.on_mouse_down(x, y, right, cx),
            Ev::Up { x } => self.on_mouse_up(x, cx),
            Ev::Wheel { dx, dy } => {
                if dx != 0.0 {
                    self.scroll_x -= (dx * WHEEL_LINES * self.l.row_h as f32) as i32;
                    self.clamp_scroll();
                }
                if dy != 0.0 {
                    let step = dy * WHEEL_LINES * self.l.row_h as f32;
                    self.target_y = (self.target_y - step).clamp(0.0, self.max_scroll_y());
                    self.event(&Ev::Tick { dt: 0.016 }, cx);
                }
                *cx.dirty = true;
            }
            Ev::Key { vk, ctrl } => self.on_key(vk, ctrl, cx),
            Ev::Tick { dt } => {
                if self.animating() {
                    self.scroll_y += (self.target_y - self.scroll_y)
                        * (1.0 - (-dt / SCROLL_EASE_SECONDS).exp());
                    if (self.target_y - self.scroll_y).abs() < 0.5 {
                        self.scroll_y = self.target_y;
                    }
                    self.update_hover(cx);
                    *cx.dirty = true;
                }
            }
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, settings: &Settings) {
        self.layout(c, area, settings);
        self.paint_toolbar(c);
        self.paint_header(c, settings);
        self.paint_rows(c);
        self.paint_overlay(c, settings);
    }
}

impl DetailsPage {
    /// Benchmark support: replaces the data without a full context.
    pub fn load_for_test(&mut self, snap: Arc<Snapshot>, settings: &Settings) {
        self.snap = snap;
        self.haystacks.clear();
        self.resort(settings);
    }

    pub fn filter_for_test(&mut self, text: &str) {
        self.filter = text.to_lowercase().split_whitespace().map(String::from).collect();
        self.refilter();
    }

    pub fn resort_for_test(&mut self, settings: &Settings) {
        self.resort(settings);
    }
}
