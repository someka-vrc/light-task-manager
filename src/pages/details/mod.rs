//! The details page: a virtualized process table.

pub mod columns;
pub mod grouping;
mod input;
mod paint;
mod rows;

use super::{Cmd, Cursor, Cx, Ev, Nav, Page};
use crate::i18n::tr;
use crate::settings::{Settings, ViewMode};
use crate::system::actions;
use crate::system::model::{Proc, Snapshot};
use crate::system::win::{filetime_now, SEC};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons::{self, glyph};
use crate::ui::keys::*;
use crate::ui::theme::THEME;
use crate::ui::widgets::{FLING_DECAY_SECONDS, FLING_STOP_SPEED, SCROLL_EASE_SECONDS, WHEEL_LINES};
use columns::Col;
use grouping::{Group, GroupId};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A process that just started or ended stays tinted at least this long, and through at
/// least `LIFETIME_UPDATES` updates.
const LIFETIME_HOLD: Duration = Duration::from_secs(5);
const LIFETIME_UPDATES: u64 = 2;
const NO_PARENT: u32 = u32::MAX;
const TOUCH_EDGE_FACTOR: i32 = 3;

/// Which groups (indices into `columns::GROUPS`) fill each panel, top to bottom, for a given
/// number of panels. The picker uses the first layout that fits the window, so the most panels win.
type PickerLayout = &'static [&'static [usize]];
const PICKER_LAYOUTS: [PickerLayout; 3] = [
    &[&[0, 1], &[2, 4], &[3, 5]],
    &[&[0, 1, 2], &[3, 4, 5]],
    &[&[0, 1, 2, 3, 4, 5]],
];

/// The width of each panel: its longest label, with room for the checkbox.
fn picker_panel_widths(c: &mut Canvas, layout: &[&[usize]]) -> Vec<i32> {
    layout
        .iter()
        .map(|groups| {
            let mut widest = 0;
            for g in groups.iter() {
                let (name, members) = columns::GROUPS[*g];
                widest = widest.max(c.measure(name(tr()), Font::Semi));
                for col in members.iter() {
                    widest = widest.max(c.measure(columns::def(*col).full(), Font::Small));
                }
            }
            // Checkbox, the gaps around it, and the right margin.
            widest + c.px(44)
        })
        .collect()
}

/// What `DetailsPage::reveal` found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reveal {
    /// The process is selected and scrolled into view.
    Shown,
    /// The filter hides the process.
    Filtered,
    /// The process is not in the list (yet).
    Missing,
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Proc(u32),
    Group(u32),
    Child(u32),
    /// A process in the tree.
    Node(u32),
    /// A process that ended a moment ago (an index into `ghosts`).
    Ghost(u32),
}

/// How a row of the view is drawn.
#[derive(Clone, Copy, Default)]
struct Meta {
    depth: u16,
    /// Shown only because a descendant matches the filter.
    faded: bool,
    /// A tree node with children to show or hide.
    branch: bool,
    open: bool,
}

/// A process that ended, kept in the list for a moment where it was.
struct Ghost {
    proc: Proc,
    hay: String,
    at: Instant,
    generation: u64,
    /// Its row in the view when it ended.
    index: usize,
    depth: u16,
}

#[derive(Clone, PartialEq)]
enum Sel {
    Proc((u32, i64)),
    Group(GroupId),
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    EndTask,
    EndTree,
    OpenLocation,
    Properties,
    CopyRow,
    PickColumns,
    ResetColumns,
    HideColumn(Col),
    View(ViewMode),
}

struct MenuItem {
    label: &'static str,
    act: Act,
    enabled: bool,
    /// For a choice: whether it is the current one.
    checked: Option<bool>,
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
        targets: Vec<(u32, i64)>,
        name: String,
        tree: bool,
        hover: Option<usize>,
        buttons: [Rect; 2],
    },
    Columns {
        hover: Option<usize>,
        /// Each checkbox's rectangle and the column it toggles.
        cells: Vec<(Rect, Col)>,
        hover_btn: Option<usize>,
        buttons: [Rect; 2],
        panel: Rect,
        /// Scroll offset of the list, its largest value, the list rectangle, and the pixels per wheel step.
        scroll: i32,
        max_scroll: i32,
        list: Rect,
        line: i32,
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
    aim_btn: Rect,
    view_btn: Rect,
    expand_btn: Rect,
    collapse_btn: Rect,
    chevron: i32,
    indent: i32,
}

pub struct DetailsPage {
    snap: Arc<Snapshot>,
    sorted: Vec<u32>,
    groups: Vec<Group>,
    in_group: Vec<bool>,
    expanded: HashSet<GroupId>,
    mode: ViewMode,
    /// The tree: each process's parent (`NO_PARENT` for none) and children, and the roots.
    parent: Vec<u32>,
    children: Vec<Vec<u32>>,
    roots: Vec<u32>,
    /// Tree nodes that are open, by process key. Not saved.
    open_nodes: HashSet<(u32, i64)>,
    top: Vec<Row>,
    view: Vec<Row>,
    meta: Vec<Meta>,
    /// Processes that started a moment ago: when, and at which update.
    born: HashMap<(u32, i64), (Instant, u64)>,
    ghosts: Vec<Ghost>,
    /// Updates seen, and when the previous one came.
    generation: u64,
    prev_at: Option<Instant>,
    filter: Vec<String>,
    haystacks: Vec<String>,
    selected: Option<Sel>,
    scroll_y: f32,
    target_y: f32,
    scroll_x: i32,
    pan_remainder: f32,
    fling: (f32, f32),
    hover_row: Option<usize>,
    hover_chevron: bool,
    hover_view: bool,
    hover_header: Option<usize>,
    hover_edge: bool,
    hover_vbar: bool,
    hover_hbar: bool,
    hover_end: bool,
    hover_col: bool,
    hover_aim: bool,
    hover_expand: bool,
    hover_collapse: bool,
    /// Aim mode is on (see `crate::app::aim`).
    aiming: bool,
    mouse: (i32, i32),
    drag: Drag,
    overlay: Overlay,
    l: Layout,
    cell: String,
    scale: columns::Scale,
    /// Highest total disk rate seen, the 100% mark of the disk percent column.
    disk_peak: f64,
}

impl DetailsPage {
    pub fn new() -> Self {
        Self {
            snap: Arc::new(Snapshot::default()),
            sorted: Vec::new(),
            groups: Vec::new(),
            in_group: Vec::new(),
            expanded: HashSet::new(),
            mode: ViewMode::Flat,
            parent: Vec::new(),
            children: Vec::new(),
            roots: Vec::new(),
            open_nodes: HashSet::new(),
            top: Vec::new(),
            view: Vec::new(),
            meta: Vec::new(),
            born: HashMap::new(),
            ghosts: Vec::new(),
            generation: 0,
            prev_at: None,
            filter: Vec::new(),
            haystacks: Vec::new(),
            selected: None,
            scroll_y: 0.0,
            target_y: 0.0,
            scroll_x: 0,
            pan_remainder: 0.0,
            fling: (0.0, 0.0),
            hover_row: None,
            hover_chevron: false,
            hover_view: false,
            hover_header: None,
            hover_edge: false,
            hover_vbar: false,
            hover_hbar: false,
            hover_end: false,
            hover_col: false,
            hover_aim: false,
            hover_expand: false,
            hover_collapse: false,
            aiming: false,
            mouse: (-1, -1),
            drag: Drag::None,
            overlay: Overlay::None,
            l: Layout::default(),
            cell: String::with_capacity(256),
            scale: columns::Scale::default(),
            disk_peak: 0.0,
        }
    }

    fn pan_x(&mut self, dx: f32) -> bool {
        self.pan_remainder += dx;
        let whole = self.pan_remainder.trunc();
        self.pan_remainder -= whole;
        let before = self.scroll_x;
        self.scroll_x -= whole as i32;
        self.clamp_scroll();
        whole == 0.0 || self.scroll_x != before
    }

    fn pan_y(&mut self, dy: f32) -> bool {
        let wanted = self.scroll_y - dy;
        self.scroll_y = wanted.clamp(0.0, self.max_scroll_y());
        self.target_y = self.scroll_y;
        self.scroll_y == wanted
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
        self.selected.as_ref()?;
        self.view.iter().position(|row| self.row_selected(*row))
    }

    fn selected_group(&self) -> Option<&Group> {
        let Some(Sel::Group(id)) = &self.selected else { return None };
        self.groups.iter().find(|g| g.id == *id)
    }

    /// The selected process, or the totals row of the selected group.
    fn selected_proc(&self) -> Option<&Proc> {
        match self.selected.as_ref()? {
            Sel::Proc(key) => self.snap.procs.iter().find(|p| p.key() == *key),
            Sel::Group(_) => self.selected_group().map(|g| &g.summary),
        }
    }

    fn select_index(&mut self, index: usize) {
        if let Some(row) = self.view.get(index).copied() {
            self.selected = Some(self.row_sel(row));
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

    /// The names the list shows, by PID, for aim mode's label.
    pub fn names(&self) -> HashMap<u32, String> {
        self.snap.procs.iter().map(|p| (p.pid, p.stat.name.clone())).collect()
    }

    pub fn set_aiming(&mut self, on: bool) {
        self.aiming = on;
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

/// Appends a cell of a row. `members` is the size of the group the row totals.
fn fill_cell(col: Col, p: &Proc, members: Option<usize>, scale: &columns::Scale, out: &mut String) {
    if members.is_some() && col == Col::Pid {
        return;
    }
    columns::cell(col, p, scale, out);
    if let (Some(n), Col::Name) = (members, col) {
        let _ = write!(out, " ({n})");
    }
}

impl Page for DetailsPage {
    fn demand(&self, settings: &Settings) -> crate::system::demand::Needs {
        crate::system::demand::Needs { shown: crate::system::demand::need::PROCS | columns::demand(&settings.columns), always: 0 }
    }

    fn nav(&self) -> Nav {
        Nav {
            icon: icons::PROCESSES,
            label: tr().details.title,
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
        if !snap.has_procs {
            return;
        }
        let disk_now: f64 = snap.sys.disks.iter().map(|d| d.read_bps + d.write_bps).sum();
        self.disk_peak = self.disk_peak.max(disk_now);
        self.scale = columns::Scale {
            mem_total: snap.mem_total,
            disk_full: self.disk_peak,
            net_full: snap.sys.nets.iter().map(|n| n.link_bps as f64 / 8.0).sum(),
        };
        self.take_snapshot(snap, cx.settings, Instant::now());
        self.hover_row = None;
        self.update_hover(cx);
        *cx.dirty = true;
    }

    fn on_enriched(&mut self, cx: &mut Cx) {
        self.haystacks.clear();
        // Group labels come from file descriptions, which arrive late.
        self.regroup(cx.settings);
        *cx.dirty = true;
    }

    fn animating(&self) -> bool {
        self.scroll_y != self.target_y || self.fling != (0.0, 0.0)
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        matches!(self.overlay, Overlay::None)
            && self.l.body.contains(x, y)
            && !self.vthumb().is_some_and(|t| t.contains(x, y))
            && !self.hthumb().is_some_and(|t| t.contains(x, y))
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
            Ev::Pan { dx, dy } => {
                self.fling = (0.0, 0.0);
                self.pan_x(dx);
                self.pan_y(dy);
                *cx.dirty = true;
            }
            Ev::Fling { vx, vy } => {
                self.fling = (vx, vy);
                *cx.dirty = true;
            }
            Ev::Wheel { dx, dy, .. } => {
                self.fling = (0.0, 0.0);
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
            Ev::Char { .. } => {}
            Ev::Tick { dt } => {
                if self.fling != (0.0, 0.0) {
                    let (vx, vy) = self.fling;
                    let decay = (-dt / FLING_DECAY_SECONDS).exp();
                    let vx = if self.pan_x(vx * dt) { vx * decay } else { 0.0 };
                    let vy = if self.pan_y(vy * dt) { vy * decay } else { 0.0 };
                    self.fling = if vx.hypot(vy) < FLING_STOP_SPEED { (0.0, 0.0) } else { (vx, vy) };
                    *cx.dirty = true;
                }
                if self.scroll_y != self.target_y {
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
        self.paint_toolbar(c, settings);
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
        self.regroup(settings);
    }

    /// Screenshot support.
    pub fn expand_all_for_test(&mut self) {
        self.expand_all();
    }

    pub fn filter_for_test(&mut self, text: &str) {
        self.filter = text.to_lowercase().split_whitespace().map(String::from).collect();
        self.refilter();
    }

    /// Screenshot support: an update in which two of the first rows end and a process starts.
    pub fn simulate_lifetime_for_test(&mut self, settings: &Settings) {
        let rows: Vec<u32> = self
            .view
            .iter()
            .filter_map(|r| match *r {
                Row::Proc(i) | Row::Child(i) | Row::Node(i) => Some(i),
                _ => None,
            })
            .take(8)
            .collect();
        if rows.len() < 6 {
            return;
        }
        let gone = [rows[1], rows[4]];
        let mut procs: Vec<Proc> = (0..self.snap.procs.len() as u32)
            .filter(|i| !gone.contains(i))
            .map(|i| self.snap.procs[i as usize].clone())
            .collect();
        let mut fresh = self.snap.procs[rows[2] as usize].clone();
        fresh.pid = 999_996;
        fresh.create_time = filetime_now();
        procs.push(fresh);
        let old = &self.snap;
        let snap = Arc::new(Snapshot {
            procs,
            has_procs: true,
            cpu_total: old.cpu_total,
            mem_total: old.mem_total,
            mem_used: old.mem_used,
            sys: old.sys.clone(),
        });
        self.take_snapshot(&snap, settings, Instant::now());
    }

    pub fn resort_for_test(&mut self, settings: &Settings) {
        self.resort(settings);
    }
}
