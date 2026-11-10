//! Building blocks shared by the list pages: a sortable virtual table, a
//! context menu, a confirmation dialog and toolbar buttons.

use super::{glyph, icons, Ev};
use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::model::ImageInfo;
use crate::theme::THEME;
use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Instant;

pub const VK_RETURN: u16 = 0x0D;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_PRIOR: u16 = 0x21;
pub const VK_NEXT: u16 = 0x22;
pub const VK_END: u16 = 0x23;
pub const VK_HOME: u16 = 0x24;
pub const VK_UP: u16 = 0x26;
pub const VK_DOWN: u16 = 0x28;
pub const VK_DELETE: u16 = 0x2E;
pub const VK_C: u16 = 0x43;
pub const VK_F5: u16 = 0x74;

const WHEEL_LINES: f32 = 3.0;
const SCROLL_EASE_SECONDS: f32 = 0.04;
const DOUBLE_CLICK_SECONDS: f32 = 0.45;
const FLING_DECAY_SECONDS: f32 = 0.5;
const FLING_STOP_SPEED: f32 = 40.0;

// ---------------------------------------------------------------- formatting

pub fn bytes(v: u64) -> String {
    let v = v as f64;
    const K: f64 = 1024.0;
    if v < K {
        format!("{v:.0} B")
    } else if v < K * K {
        format!("{:.1} KB", v / K)
    } else if v < K * K * K {
        format!("{:.1} MB", v / K / K)
    } else if v < K * K * K * K {
        format!("{:.1} GB", v / K / K / K)
    } else {
        format!("{:.1} TB", v / K / K / K / K)
    }
}

/// Bytes per second as bits per second, the way network speeds are quoted.
pub fn bits_per_sec(bytes_per_sec: f64) -> String {
    let b = bytes_per_sec * 8.0;
    if b < 1000.0 {
        format!("{b:.0} bps")
    } else if b < 1e6 {
        format!("{:.1} Kbps", b / 1e3)
    } else if b < 1e9 {
        format!("{:.1} Mbps", b / 1e6)
    } else {
        format!("{:.1} Gbps", b / 1e9)
    }
}

pub fn bytes_per_sec(v: f64) -> String {
    const K: f64 = 1024.0;
    if v < K {
        format!("{v:.0} B/秒")
    } else if v < K * K {
        format!("{:.0} KB/秒", v / K)
    } else if v < K * K * K {
        format!("{:.1} MB/秒", v / K / K)
    } else {
        format!("{:.1} GB/秒", v / K / K / K)
    }
}

/// 100ns units to h:mm:ss.
pub fn duration_100ns(v: u64) -> String {
    let s = v / 10_000_000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn uptime(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}:{:02}", s / 86400, s / 3600 % 24, s / 60 % 60, s % 60)
}

// ---------------------------------------------------------------- chrome

pub fn panel(c: &mut Canvas, r: Rect) {
    let t = THEME;
    let radius = c.px(8) as f32;
    for i in (1..=4).rev() {
        let grow = c.px(i * 3);
        c.round(rect(r.x - grow, r.y - grow + c.px(2), r.w + grow * 2, r.h + grow * 2), radius + grow as f32, 0, 22);
    }
    c.round(r, radius, t.popup, 255);
    c.stroke(r, radius, 1.0, t.border, 255);
}

pub const TOOLBAR_H: i32 = 60;

/// Page title and a dim info line, as on the details page.
pub fn paint_title(c: &mut Canvas, area: Rect, title: &str, info: &str, info_right: i32) {
    let t = THEME;
    let x = area.x + c.px(20);
    let w = c.measure(title, Font::Title);
    c.text(title, Font::Title, rect(x, area.y, w + c.px(4), c.px(TOOLBAR_H)), t.text, Align::Left);
    let ix = x + w + c.px(16);
    c.text(info, Font::Small, rect(ix, area.y + c.px(3), (info_right - ix - c.px(12)).max(0), c.px(TOOLBAR_H)), t.text_dim, Align::Left);
}

#[derive(Clone, Copy)]
pub struct ButtonSpec {
    pub icon: char,
    pub label: &'static str,
    pub enabled: bool,
}

pub fn btn(icon: char, label: &'static str, enabled: bool) -> ButtonSpec {
    ButtonSpec { icon, label, enabled }
}

/// Lays out toolbar buttons from the right edge, in the given order (left to right).
pub fn toolbar_rects(c: &mut Canvas, area: Rect, specs: &[ButtonSpec]) -> Vec<Rect> {
    let h = c.px(34);
    let y = area.y + (c.px(TOOLBAR_H) - h) / 2;
    let mut x = area.r() - c.px(16);
    let mut out = vec![Rect::default(); specs.len()];
    for (i, s) in specs.iter().enumerate().rev() {
        let w = c.measure(s.label, Font::Body) + c.px(48);
        x -= w;
        out[i] = rect(x, y, w, h);
        x -= c.px(8);
    }
    out
}

pub fn paint_button(c: &mut Canvas, r: Rect, s: &ButtonSpec, hover: bool) {
    let t = THEME;
    let radius = c.px(6) as f32;
    let fg = if s.enabled { t.text } else { t.text_faint };
    if s.enabled && hover {
        c.round(r, radius, t.hover, 255);
    }
    c.stroke(r, radius, 1.0, t.border, 255);
    let ix = r.x + c.px(12);
    c.text(&glyph(s.icon), Font::IconSmall, rect(ix, r.y, c.px(16), r.h), fg, Align::Left);
    c.text(s.label, Font::Body, rect(ix + c.px(22), r.y, r.w - c.px(36), r.h), fg, Align::Left);
}

/// A toolbar: rects are recomputed on paint, hover is tracked on move.
#[derive(Default)]
pub struct Toolbar {
    pub rects: Vec<Rect>,
    pub hover: Option<usize>,
}

impl Toolbar {
    pub fn paint(&mut self, c: &mut Canvas, area: Rect, specs: &[ButtonSpec]) -> i32 {
        self.rects = toolbar_rects(c, area, specs);
        for (i, s) in specs.iter().enumerate() {
            paint_button(c, self.rects[i], s, self.hover == Some(i));
        }
        self.rects.first().map_or(area.r(), |r| r.x)
    }

    /// Returns true when the hover state changed.
    pub fn hover_at(&mut self, x: i32, y: i32) -> bool {
        let h = self.rects.iter().position(|r| r.contains(x, y));
        std::mem::replace(&mut self.hover, h) != h
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.rects.iter().position(|r| r.contains(x, y))
    }
}

// ---------------------------------------------------------------- menu

pub struct MenuItem {
    pub label: String,
    pub id: u32,
    pub enabled: bool,
}

pub fn item(label: impl Into<String>, id: u32, enabled: bool) -> MenuItem {
    MenuItem { label: label.into(), id, enabled }
}

pub struct Menu {
    origin: (i32, i32),
    items: Vec<MenuItem>,
    rect: Rect,
    hover: Option<usize>,
}

pub enum MenuOut {
    Stay,
    Close,
    Pick(u32),
}

impl Menu {
    pub fn new(x: i32, y: i32, items: Vec<MenuItem>) -> Self {
        Self { origin: (x, y), items, rect: Rect::default(), hover: None }
    }

    pub fn paint(&mut self, c: &mut Canvas, area: Rect) {
        let t = THEME;
        let item_h = c.px(34);
        let mut w = c.px(200);
        for i in &self.items {
            w = w.max(c.measure(&i.label, Font::Body) + c.px(40));
        }
        let h = item_h * self.items.len() as i32;
        let x = self.origin.0.min(area.r() - w - c.px(6)).max(area.x);
        let y = self.origin.1.min(area.b() - h - c.px(6)).max(area.y);
        self.rect = rect(x, y, w, h);
        panel(c, self.rect);
        for (i, it) in self.items.iter().enumerate() {
            let row = rect(x, y + i as i32 * item_h, w, item_h);
            if self.hover == Some(i) && it.enabled {
                c.round(row.inset(c.px(4), c.px(2)), c.px(4) as f32, t.pressed, 255);
            }
            let color = if it.enabled { t.text } else { t.text_faint };
            c.text(&it.label, Font::Body, row.inset(c.px(14), 0), color, Align::Left);
        }
    }

    fn index_at(&self, x: i32, y: i32) -> Option<usize> {
        let r = self.rect;
        r.contains(x, y).then(|| ((y - r.y) * self.items.len() as i32 / r.h.max(1)) as usize)
    }

    pub fn event(&mut self, ev: &Ev, dirty: &mut bool) -> MenuOut {
        match *ev {
            Ev::Move { x, y } => {
                let h = self.index_at(x, y);
                if h != self.hover {
                    self.hover = h;
                    *dirty = true;
                }
                MenuOut::Stay
            }
            Ev::Down { x, y, .. } => match self.index_at(x, y) {
                Some(i) => match self.items.get(i) {
                    Some(it) if it.enabled => MenuOut::Pick(it.id),
                    _ => MenuOut::Stay,
                },
                None => MenuOut::Close,
            },
            Ev::Key { vk: VK_ESCAPE, .. } => MenuOut::Close,
            _ => MenuOut::Stay,
        }
    }
}

// ---------------------------------------------------------------- confirm

pub struct Confirm {
    pub title: String,
    pub subject: String,
    pub lines: Vec<String>,
    pub ok: &'static str,
    pub danger: bool,
    /// Identifies what to do on OK; meaning is up to the page.
    pub tag: u32,
    hover: Option<usize>,
    buttons: [Rect; 2],
}

pub enum ConfirmOut {
    Stay,
    Cancel,
    Ok,
}

impl Confirm {
    pub fn new(title: impl Into<String>, subject: impl Into<String>, lines: &[&str], ok: &'static str, danger: bool, tag: u32) -> Self {
        Self {
            title: title.into(),
            subject: subject.into(),
            lines: lines.iter().map(|s| s.to_string()).collect(),
            ok,
            danger,
            tag,
            hover: None,
            buttons: [Rect::default(); 2],
        }
    }

    pub fn paint(&mut self, c: &mut Canvas, area: Rect) {
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(460);
        let h = c.px(132) + c.px(20) * self.lines.len() as i32;
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
        panel(c, r);
        let inner = r.inset(c.px(22), 0);
        c.text(&self.title, Font::Semi, rect(inner.x, r.y + c.px(18), inner.w, c.px(26)), t.text, Align::Left);
        c.text(&self.subject, Font::Body, rect(inner.x, r.y + c.px(50), inner.w, c.px(24)), t.accent, Align::Left);
        for (i, line) in self.lines.iter().enumerate() {
            let y = r.y + c.px(78) + c.px(20) * i as i32;
            c.text(line, Font::Small, rect(inner.x, y, inner.w, c.px(20)), t.text_dim, Align::Left);
        }
        let (bw, bh) = (c.px(120), c.px(36));
        let by = r.b() - bh - c.px(16);
        self.buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
        self.buttons[0] = rect(self.buttons[1].x - bw - c.px(8), by, bw, bh);
        let radius = c.px(6) as f32;
        let alpha = if self.hover == Some(0) { 255 } else { 225 };
        if self.danger {
            c.round(self.buttons[0], radius, t.danger, alpha);
            c.text(self.ok, Font::Body, self.buttons[0], 0xFFFFFF, Align::Center);
        } else {
            c.round(self.buttons[0], radius, t.accent, alpha);
            c.text(self.ok, Font::Body, self.buttons[0], t.on_accent, Align::Center);
        }
        c.round(self.buttons[1], radius, if self.hover == Some(1) { t.pressed } else { t.hover }, 255);
        c.text("キャンセル", Font::Body, self.buttons[1], t.text, Align::Center);
    }

    pub fn event(&mut self, ev: &Ev, dirty: &mut bool) -> ConfirmOut {
        match *ev {
            Ev::Move { x, y } => {
                let h = self.buttons.iter().position(|b| b.contains(x, y));
                if h != self.hover {
                    self.hover = h;
                    *dirty = true;
                }
                ConfirmOut::Stay
            }
            Ev::Down { x, y, right: false } => match self.buttons.iter().position(|b| b.contains(x, y)) {
                Some(0) => ConfirmOut::Ok,
                Some(_) => ConfirmOut::Cancel,
                None => ConfirmOut::Stay,
            },
            Ev::Key { vk: VK_RETURN, .. } => ConfirmOut::Ok,
            Ev::Key { vk: VK_ESCAPE, .. } => ConfirmOut::Cancel,
            _ => ConfirmOut::Stay,
        }
    }
}

// ---------------------------------------------------------------- table

pub enum Key {
    Num(f64),
    /// Lowercase.
    Text(String),
}

pub fn num(v: impl Into<f64>) -> Key {
    Key::Num(v.into())
}

pub fn text(s: &str) -> Key {
    Key::Text(s.to_lowercase())
}

pub struct Row {
    pub id: u64,
    pub cells: Vec<String>,
    pub keys: Vec<Key>,
    pub icon: Option<Arc<ImageInfo>>,
    /// Drawn in the dim color, e.g. a stopped service.
    pub dim: bool,
    /// Lowercase text the filter matches against.
    pub hay: String,
}

impl Row {
    pub fn new(id: u64, cells: Vec<String>, keys: Vec<Key>) -> Self {
        let hay = cells.join("\u{1}").to_lowercase();
        Self { id, cells, keys, icon: None, dim: false, hay }
    }
}

pub struct Column {
    pub label: &'static str,
    /// Device-independent pixels. Column 0 also takes any spare width.
    pub width: i32,
    pub right: bool,
    pub descending_first: bool,
}

pub const fn col(label: &'static str, width: i32, right: bool) -> Column {
    Column { label, width, right, descending_first: right }
}

pub enum TableOut {
    None,
    /// Selection changed.
    Select,
    Context { x: i32, y: i32 },
    /// Double click or Enter on a row.
    Activate,
}

pub struct Table {
    cols: &'static [Column],
    rows: Vec<Row>,
    view: Vec<usize>,
    filter: Vec<String>,
    pub sort: usize,
    pub desc: bool,
    pub selected: Option<u64>,
    scroll_y: f32,
    target_y: f32,
    /// Vertical fling speed in pixels per second.
    fling: f32,
    hover_row: Option<usize>,
    hover_header: Option<usize>,
    hover_bar: bool,
    drag_thumb: Option<i32>,
    pressed_header: Option<usize>,
    mouse: (i32, i32),
    last_click: Option<(Instant, u64)>,
    pub empty_text: &'static str,
    // Layout, in pixels.
    table: Rect,
    body: Rect,
    row_h: i32,
    header_h: i32,
    pad: i32,
    bar: i32,
    xs: Vec<(i32, i32)>,
}

impl Table {
    pub fn new(cols: &'static [Column], sort: usize, desc: bool) -> Self {
        Self {
            cols,
            rows: Vec::new(),
            view: Vec::new(),
            filter: Vec::new(),
            sort,
            desc,
            selected: None,
            scroll_y: 0.0,
            target_y: 0.0,
            fling: 0.0,
            hover_row: None,
            hover_header: None,
            hover_bar: false,
            drag_thumb: None,
            pressed_header: None,
            mouse: (-1, -1),
            last_click: None,
            empty_text: "項目はありません",
            table: Rect::default(),
            body: Rect::default(),
            row_h: 28,
            header_h: 32,
            pad: 10,
            bar: 12,
            xs: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn shown(&self) -> usize {
        self.view.len()
    }

    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        if let Some(id) = self.selected {
            if !self.rows.iter().any(|r| r.id == id) {
                self.selected = None;
            }
        }
        self.resort();
    }

    pub fn set_filter(&mut self, text: &str) {
        self.filter = text.to_lowercase().split_whitespace().map(String::from).collect();
        self.refilter();
    }

    pub fn selected_row(&self) -> Option<&Row> {
        let id = self.selected?;
        self.rows.iter().find(|r| r.id == id)
    }

    /// Tab-separated cells of the selected row.
    pub fn selected_text(&self) -> Option<String> {
        self.selected_row().map(|r| r.cells.join("\t"))
    }

    fn resort(&mut self) {
        let (col, desc) = (self.sort, self.desc);
        let rows = &self.rows;
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|a, b| {
            let o = match (rows[*a].keys.get(col), rows[*b].keys.get(col)) {
                (Some(Key::Num(x)), Some(Key::Num(y))) => x.total_cmp(y),
                (Some(Key::Text(x)), Some(Key::Text(y))) => x.cmp(y),
                _ => Ordering::Equal,
            };
            let o = if desc { o.reverse() } else { o };
            o.then(rows[*a].id.cmp(&rows[*b].id))
        });
        // Reorder in place so `view` can hold plain indices.
        let mut taken: Vec<Option<Row>> = std::mem::take(&mut self.rows).into_iter().map(Some).collect();
        self.rows = order.into_iter().map(|i| taken[i].take().unwrap()).collect();
        self.refilter();
    }

    fn refilter(&mut self) {
        self.view.clear();
        let terms = &self.filter;
        self.view.extend(
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| terms.iter().all(|t| r.hay.contains(t.as_str())))
                .map(|(i, _)| i),
        );
        self.clamp();
    }

    fn max_scroll(&self) -> f32 {
        (self.view.len() as i32 * self.row_h - self.body.h).max(0) as f32
    }

    fn clamp(&mut self) {
        let max = self.max_scroll();
        self.target_y = self.target_y.clamp(0.0, max);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.view.iter().position(|i| self.rows[*i].id == id)
    }

    fn select_index(&mut self, index: usize) {
        let Some(&i) = self.view.get(index) else { return };
        self.selected = Some(self.rows[i].id);
        let (top, rh) = (index as i32 * self.row_h, self.row_h);
        let mut y = self.target_y;
        if (top as f32) < y {
            y = top as f32;
        } else if ((top + rh) as f32) > y + self.body.h as f32 {
            y = (top + rh - self.body.h) as f32;
        }
        self.target_y = y;
        self.scroll_y = y;
    }

    fn thumb(&self) -> Option<Rect> {
        let max = self.max_scroll();
        if max <= 0.0 {
            return None;
        }
        let track = self.body.h;
        let content = self.view.len() as i32 * self.row_h;
        let h = (track * track / content.max(1)).max(self.bar * 3).min(track);
        let y = self.body.y + ((track - h) as f32 * (self.scroll_y / max)) as i32;
        Some(rect(self.table.r() - self.bar, y, self.bar, h))
    }

    fn update_hover(&mut self) -> bool {
        let (x, y) = self.mouse;
        let bar = self.max_scroll() > 0.0 && x >= self.table.r() - self.bar && self.body.contains(x, y);
        let row = if self.body.contains(x, y) && !bar && self.row_h > 0 {
            let i = ((y - self.body.y) as f32 + self.scroll_y) as i32 / self.row_h;
            ((i as usize) < self.view.len()).then_some(i as usize)
        } else {
            None
        };
        let header = if y >= self.table.y && y < self.body.y && x >= self.table.x && x < self.table.r() {
            self.xs.iter().position(|(cx, w)| x >= *cx && x < cx + w)
        } else {
            None
        };
        let changed = row != self.hover_row || header != self.hover_header || bar != self.hover_bar;
        self.hover_row = row;
        self.hover_header = header;
        self.hover_bar = bar;
        changed
    }

    pub fn animating(&self) -> bool {
        self.scroll_y != self.target_y || self.fling != 0.0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.table.contains(x, y)
    }

    /// Whether a finger or pen drag at (x, y) should scroll the rows.
    pub fn pannable(&self, x: i32, y: i32) -> bool {
        self.body.contains(x, y) && !self.thumb().is_some_and(|t| t.contains(x, y))
    }

    /// Scrolls by a drag distance; returns false when it hit an end.
    fn pan_y(&mut self, dy: f32) -> bool {
        let wanted = self.scroll_y - dy;
        self.scroll_y = wanted.clamp(0.0, self.max_scroll());
        self.target_y = self.scroll_y;
        self.scroll_y == wanted
    }

    pub fn event(&mut self, ev: &Ev, dirty: &mut bool) -> TableOut {
        match *ev {
            Ev::Move { x, y } => {
                self.mouse = (x, y);
                if let Some(offset) = self.drag_thumb {
                    if let Some(t) = self.thumb() {
                        let track = self.body.h - t.h;
                        let ratio = (y - offset - self.body.y) as f32 / track.max(1) as f32;
                        self.scroll_y = (ratio * self.max_scroll()).clamp(0.0, self.max_scroll());
                        self.target_y = self.scroll_y;
                        *dirty = true;
                    }
                } else if self.update_hover() {
                    *dirty = true;
                }
            }
            Ev::Leave => {
                self.mouse = (-1, -1);
                if self.update_hover() {
                    *dirty = true;
                }
            }
            Ev::Down { x, y, right } => {
                self.mouse = (x, y);
                self.update_hover();
                if !right && self.hover_bar {
                    if let Some(t) = self.thumb() {
                        if t.contains(x, y) {
                            self.drag_thumb = Some(y - t.y);
                        } else {
                            let dir = if y < t.y { -1.0 } else { 1.0 };
                            self.target_y = (self.target_y + dir * self.body.h as f32).clamp(0.0, self.max_scroll());
                            *dirty = true;
                        }
                    }
                    return TableOut::None;
                }
                if let Some(h) = self.hover_header {
                    if !right {
                        self.pressed_header = Some(h);
                    }
                    return TableOut::None;
                }
                if let Some(i) = self.hover_row {
                    let id = self.rows[self.view[i]].id;
                    let changed = self.selected != Some(id);
                    self.selected = Some(id);
                    *dirty = true;
                    if right {
                        return TableOut::Context { x, y };
                    }
                    let now = Instant::now();
                    let double = matches!(self.last_click, Some((t, last)) if last == id && (now - t).as_secs_f32() < DOUBLE_CLICK_SECONDS);
                    self.last_click = if double { None } else { Some((now, id)) };
                    if double {
                        return TableOut::Activate;
                    }
                    if changed {
                        return TableOut::Select;
                    }
                } else if self.body.contains(x, y) && !right && self.selected.take().is_some() {
                    *dirty = true;
                    return TableOut::Select;
                }
            }
            Ev::Up { .. } => {
                if self.drag_thumb.take().is_some() {
                    *dirty = true;
                }
                if let Some(h) = self.pressed_header.take() {
                    if self.hover_header == Some(h) {
                        if self.sort == h {
                            self.desc = !self.desc;
                        } else {
                            self.sort = h;
                            self.desc = self.cols[h].descending_first;
                        }
                        self.resort();
                        *dirty = true;
                    }
                }
            }
            Ev::Pan { dy, .. } => {
                self.fling = 0.0;
                self.pan_y(dy);
                self.update_hover();
                *dirty = true;
            }
            Ev::Fling { vy, .. } => {
                self.fling = vy;
                *dirty = true;
            }
            Ev::Wheel { dy, .. } => {
                self.fling = 0.0;
                if dy != 0.0 {
                    let step = dy * WHEEL_LINES * self.row_h as f32;
                    self.target_y = (self.target_y - step).clamp(0.0, self.max_scroll());
                    self.tick(0.016);
                    *dirty = true;
                }
            }
            Ev::Key { vk, .. } => {
                let page = (self.body.h / self.row_h.max(1)).max(1) as isize;
                let last = self.view.len() as isize - 1;
                let cur = self.selected_index().map(|i| i as isize);
                let next = match vk {
                    VK_UP => Some(cur.map_or(0, |i| i - 1)),
                    VK_DOWN => Some(cur.map_or(0, |i| i + 1)),
                    VK_PRIOR => Some(cur.map_or(0, |i| i - page)),
                    VK_NEXT => Some(cur.map_or(0, |i| i + page)),
                    VK_HOME => Some(0),
                    VK_END => Some(last),
                    VK_RETURN if cur.is_some() => return TableOut::Activate,
                    _ => None,
                };
                if let Some(n) = next {
                    if last >= 0 {
                        self.select_index(n.clamp(0, last) as usize);
                        *dirty = true;
                        return TableOut::Select;
                    }
                }
            }
            Ev::Tick { dt } => {
                if self.fling != 0.0 {
                    let v = self.fling;
                    let decay = (-dt / FLING_DECAY_SECONDS).exp();
                    let v = if self.pan_y(v * dt) { v * decay } else { 0.0 };
                    self.fling = if v.abs() < FLING_STOP_SPEED { 0.0 } else { v };
                    self.update_hover();
                    *dirty = true;
                }
                if self.scroll_y != self.target_y {
                    self.tick(dt);
                    self.update_hover();
                    *dirty = true;
                }
            }
        }
        TableOut::None
    }

    fn tick(&mut self, dt: f32) {
        self.scroll_y += (self.target_y - self.scroll_y) * (1.0 - (-dt / SCROLL_EASE_SECONDS).exp());
        if (self.target_y - self.scroll_y).abs() < 0.5 {
            self.scroll_y = self.target_y;
        }
    }

    pub fn paint(&mut self, c: &mut Canvas, area: Rect) {
        let t = THEME;
        self.table = area;
        self.row_h = c.px(28);
        self.header_h = c.px(32);
        self.pad = c.px(10);
        self.bar = c.px(12);
        self.body = rect(area.x, area.y + self.header_h, area.w, area.h - self.header_h);
        self.clamp();

        // Column positions: column 0 absorbs spare width.
        let widths: Vec<i32> = self.cols.iter().map(|k| c.px(k.width)).collect();
        let total: i32 = widths.iter().sum();
        let spare = (area.w - self.bar - total).max(0);
        self.xs.clear();
        let mut x = area.x;
        for (i, w) in widths.iter().enumerate() {
            let w = if i == 0 { w + spare } else { *w };
            self.xs.push((x, w));
            x += w;
        }

        // Header.
        let (pad, header_h) = (self.pad, self.header_h);
        c.set_clip(rect(area.x, area.y, area.w, header_h));
        for (i, k) in self.cols.iter().enumerate() {
            let (x, w) = self.xs[i];
            let cell = rect(x, area.y, w, header_h);
            if self.hover_header == Some(i) {
                c.round(cell.inset(c.px(2), c.px(3)), c.px(4) as f32, t.hover, 255);
            }
            let sorted = self.sort == i;
            let arrow = if sorted { c.px(14) } else { 0 };
            let ta = rect(x + pad, area.y, w - pad * 2 - arrow, header_h);
            let align = if k.right { Align::Right } else { Align::Left };
            c.text(k.label, Font::Small, ta, if sorted { t.text } else { t.text_dim }, align);
            if sorted {
                let ch = if self.desc { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP };
                c.text(&glyph(ch), Font::IconSmall, rect(x + w - pad - arrow, area.y, arrow, header_h), t.accent, Align::Right);
            }
        }
        c.reset_clip();
        c.fill(rect(area.x, area.y + header_h - 1, area.w, 1), t.border);

        // Rows.
        let (body, rh) = (self.body, self.row_h);
        c.set_clip(body);
        let scroll = self.scroll_y.round() as i32;
        let first = (scroll / rh.max(1)) as usize;
        let last = (((scroll + body.h) / rh.max(1)) as usize + 1).min(self.view.len());
        let icon_size = c.px(16);
        let radius = c.px(5) as f32;
        for vi in first..last {
            let r = &self.rows[self.view[vi]];
            let y = body.y + vi as i32 * rh - scroll;
            let row = rect(area.x, y, area.w, rh);
            let is_sel = self.selected == Some(r.id);
            if is_sel {
                c.round(row.inset(c.px(2), c.px(1)), radius, t.selection, 255);
                c.round(rect(row.x + c.px(2), y + c.px(7), c.px(3), rh - c.px(14)), 1.5, t.accent, 255);
            } else if self.hover_row == Some(vi) {
                c.round(row.inset(c.px(2), c.px(1)), radius, t.hover, 255);
            }
            for (i, k) in self.cols.iter().enumerate() {
                let (x, w) = self.xs[i];
                let mut cell = rect(x + pad, y, w - pad * 2, rh);
                if i == 0 {
                    if let Some(img) = &r.icon {
                        let top = y + (rh - icon_size) / 2;
                        match img.icon() {
                            Some(icon) => c.icon(icon, cell.x, top, icon_size),
                            None => c.round(rect(cell.x + 1, top + 1, icon_size - 2, icon_size - 2), 3.0, t.border, 255),
                        }
                        let shift = icon_size + c.px(8);
                        cell.x += shift;
                        cell.w -= shift;
                    }
                }
                let color = if r.dim && !is_sel {
                    t.text_faint
                } else if i == 0 || is_sel {
                    t.text
                } else {
                    t.text_dim
                };
                let align = if k.right { Align::Right } else { Align::Left };
                if let Some(s) = r.cells.get(i) {
                    c.text(s, Font::Body, cell, color, align);
                }
            }
        }
        if self.view.is_empty() {
            let msg = if self.rows.is_empty() { self.empty_text } else { "一致する項目はありません" };
            c.text(msg, Font::Body, body, t.text_dim, Align::Center);
        }
        let active = self.hover_bar || self.drag_thumb.is_some();
        if let Some(th) = self.thumb() {
            let w = if active { c.px(8) } else { c.px(4) };
            let r = rect(th.r() - w - c.px(2), th.y + c.px(2), w, th.h - c.px(4));
            c.round(r, w as f32 / 2.0, t.text_faint, if active { 255 } else { 170 });
        }
        c.reset_clip();
    }
}

/// Splits `area` into the toolbar strip and the table area under it.
pub fn split(c: &Canvas, area: Rect) -> (Rect, Rect) {
    let tb = c.px(TOOLBAR_H);
    let side = c.px(12);
    (
        rect(area.x, area.y, area.w, tb),
        rect(area.x + side, area.y + tb, area.w - side * 2, area.h - tb),
    )
}

// ---------------------------------------------------------------- background jobs

/// Runs slow system calls (starting a service, signing out a user) off the UI
/// thread. Results arrive through `Page::on_enriched` of the active page.
#[derive(Clone, Default)]
pub struct Jobs {
    messages: Arc<std::sync::Mutex<Vec<String>>>,
    finished: Arc<std::sync::atomic::AtomicUsize>,
    seen: usize,
}

impl Jobs {
    pub fn spawn(&self, shared: &crate::sampler::Shared, f: impl FnOnce() -> Option<String> + Send + 'static) {
        use std::sync::atomic::Ordering;
        let (messages, finished) = (self.messages.clone(), self.finished.clone());
        let hwnd = shared.hwnd.load(Ordering::Relaxed);
        std::thread::spawn(move || {
            if let Some(m) = f() {
                messages.lock().unwrap().push(m);
            }
            finished.fetch_add(1, Ordering::AcqRel);
            if hwnd != 0 {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                        hwnd as _,
                        crate::sampler::WM_APP_ENRICHED,
                        0,
                        0,
                    )
                };
            }
        });
    }

    /// Turns finished jobs' messages into toasts. Returns true if any job finished.
    pub fn drain(&mut self, cx: &mut super::Cx) -> bool {
        use std::sync::atomic::Ordering;
        for m in self.messages.lock().unwrap().drain(..) {
            cx.cmds.push(super::Cmd::Toast(m));
        }
        let done = self.finished.load(Ordering::Acquire);
        std::mem::replace(&mut self.seen, done) != done
    }
}

/// A Win32 error code as a short sentence.
pub fn error_text(code: u32) -> String {
    match code {
        5 => "アクセスが拒否されました。設定から管理者として再起動できます。".into(),
        2 => "見つかりません。".into(),
        1056 => "サービスは既に実行されています。".into(),
        1058 => "サービスが無効になっています。".into(),
        1062 => "サービスは開始されていません。".into(),
        1051 => "依存している実行中のサービスがあるため停止できません。".into(),
        1053 => "サービスが時間内に応答しませんでした。".into(),
        _ => format!("エラー {code} が発生しました。"),
    }
}
