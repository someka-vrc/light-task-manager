//! The process page: the graph of the last minutes on top (see `timeline`), and a
//! virtualized process table below it. The table shows the processes now, at a moment
//! picked on the graph, or summed up over a range picked on the graph (see `range`).

pub mod columns;
pub mod grouping;
mod input;
mod paint;
pub mod range;
mod rows;
pub mod timeline;

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
use range::{RCol, RItem, ROp, RangeData, Which};
use timeline::Timeline;
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
fn picker_panel_widths(c: &mut Canvas, layout: &[&[usize]], groups: &[PickGroup]) -> Vec<i32> {
    layout
        .iter()
        .map(|members| {
            let mut widest = 0;
            for g in members.iter() {
                let (name, items) = &groups[*g];
                widest = widest.max(c.measure(name, Font::Semi));
                for (_, label) in items.iter() {
                    widest = widest.max(c.measure(label, Font::Small));
                }
            }
            // Checkbox, the gaps around it, and the right margin.
            widest + c.px(44)
        })
        .collect()
}

/// A checkbox of a column picker: a column of the process table, or an item or an
/// operation of the range table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pick {
    Col(Col),
    Item(RItem),
    Op(ROp),
}

/// A heading of a column picker and its checkboxes.
type PickGroup = (String, Vec<(Pick, String)>);

/// A column of whichever table is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AnyCol {
    P(Col),
    R(RCol),
}

impl AnyCol {
    fn label(self) -> String {
        match self {
            AnyCol::P(c) => columns::def(c).label().to_string(),
            AnyCol::R(c) => c.label(),
        }
    }

    fn right(self) -> bool {
        match self {
            AnyCol::P(c) => columns::def(c).right,
            AnyCol::R(c) => c.right(),
        }
    }

    fn is_name(self) -> bool {
        matches!(self, AnyCol::P(Col::Name) | AnyCol::R(RCol { item: RItem::Name, .. }))
    }
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
    /// Range table: hide an item (all its columns), or an operation (of every item).
    HideItem(RItem),
    HideOp(ROp),
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
        /// Each checkbox's rectangle and what it toggles.
        cells: Vec<(Rect, Pick)>,
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
    /// The border between the graph and the table.
    Split { start_y: i32, start_h: u16 },
}

#[derive(Clone, Copy)]
struct ColBox {
    col: AnyCol,
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
    /// The graph button: its icon part turns the graph on and off, its arrow opens a menu.
    graph_btn: Rect,
    graph_arrow: Rect,
    /// The bar shown while the graph is paused, the graph, and the border below it.
    bar_area: Rect,
    graph: Rect,
    split: Rect,
    /// The height of the top heading row of the range table (0 for the process table).
    group_h: i32,
    chevron: i32,
    indent: i32,
}

pub struct DetailsPage {
    /// What the table shows: the latest snapshot, a past one, or the processes of a range.
    snap: Arc<Snapshot>,
    /// The latest snapshot, whatever the table shows.
    live: Arc<Snapshot>,
    /// The table shows a past moment or a range, not the latest snapshot.
    past: bool,
    /// The range table's values, while a range is shown.
    range: Option<RangeData>,
    tl: Timeline,
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
    /// 1 the graph button's icon, 2 its arrow.
    hover_graph: u8,
    hover_split: bool,
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
            live: Arc::new(Snapshot::default()),
            past: false,
            range: None,
            tl: Timeline::new(),
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
            hover_graph: 0,
            hover_split: false,
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

    /// The names of the running processes, by PID, for aim mode's label.
    pub fn names(&self) -> HashMap<u32, String> {
        self.live.procs.iter().map(|p| (p.pid, p.stat.name.clone())).collect()
    }

    /// Whether a process still runs: the same PID and creation time in the latest snapshot.
    fn alive(&self, key: (u32, i64)) -> bool {
        !self.past || self.live.procs.iter().any(|p| p.key() == key)
    }

    /// The columns of the table shown, with their widths.
    fn shown_columns(&self, s: &Settings) -> Vec<(AnyCol, u16)> {
        if self.range.is_some() {
            range::visible(&s.range_items, s.range_ops).into_iter().map(|(c, w)| (AnyCol::R(c), w)).collect()
        } else {
            s.columns.iter().map(|(c, w)| (AnyCol::P(*c), *w)).collect()
        }
    }

    /// Back to the latest snapshot, for whatever acts on running processes (showing one
    /// from another page, aim mode).
    pub fn go_live(&mut self, s: &Settings) {
        if self.tl.resume() || self.past {
            self.show_selection(s);
        }
    }

    /// Shows what the graph's selection asks for: the latest snapshot, a past moment, or a range.
    fn show_selection(&mut self, s: &Settings) {
        self.born.clear();
        self.ghosts.clear();
        self.range = None;
        match self.tl.selection() {
            None => {
                self.past = false;
                self.snap = self.live.clone();
                // The next update marks nothing as started or ended: the gap is unknown.
                self.prev_at = None;
            }
            Some([one]) => {
                self.past = true;
                self.snap = Arc::new(self.tl.snapshot(one));
            }
            Some(samples) => {
                self.past = true;
                let items: Vec<RItem> = s.range_items.iter().map(|(i, _)| *i).collect();
                let (data, procs) = RangeData::build(samples, &items, |r| self.tl.unpack(r));
                let last = samples.last().expect("a range has samples");
                self.snap = Arc::new(Snapshot {
                    taken: last.time,
                    stale: false,
                    procs,
                    has_procs: true,
                    cpu_total: last.cpu,
                    mem_total: last.mem_total,
                    mem_used: last.mem_used,
                    sys: Default::default(),
                });
                self.range = Some(data);
            }
        }
        self.haystacks.clear();
        self.regroup(s);
        if let Some(i) = self.selected_index() {
            self.select_index(i);
        }
    }

    /// What the graph or its dialogs asked for.
    fn after_timeline(&mut self, out: timeline::Out, cx: &mut Cx) {
        match out {
            timeline::Out::Selection => {
                self.show_selection(cx.settings);
                *cx.dirty = true;
            }
            timeline::Out::ExportChanged => self.refresh_estimate(cx.settings),
            timeline::Out::ExportStart => {
                if let Some(job) = self.tl.export.as_ref().and_then(|d| self.export_job(cx.settings, d.choices())) {
                    self.tl.save(job, cx);
                }
            }
            timeline::Out::Pass | timeline::Out::Taken => {}
        }
    }

    /// Works out the file size for the export dialog's choices, when they changed.
    fn refresh_estimate(&mut self, s: &Settings) {
        let Some(d) = &self.tl.export else { return };
        let key = d.choices();
        if d.estimate.is_some_and(|(k, _)| k == key) {
            return;
        }
        let size = self.export_job(s, key).map_or(0, |j| j.estimate());
        if let Some(d) = &mut self.tl.export {
            d.estimate = Some((key, size));
        }
    }

    /// What to write, taken from what the page shows now.
    fn export_job(&self, s: &Settings, choices: (timeline::Scope, timeline::export::Kind, timeline::export::Format, timeline::export::Encoding)) -> Option<timeline::export::Job> {
        use timeline::{export, Scope};
        let (scope, kind, format, encoding) = choices;
        let range = match scope {
            Scope::Selection => self.tl.selection()?.to_vec(),
            Scope::All => self.tl.all().to_vec(),
        };
        if range.is_empty() {
            return None;
        }
        let table = match kind {
            export::Kind::Table if scope == Scope::All => self.whole_table(&range, s),
            export::Kind::Table => self.table_dump(s),
            _ => export::Table { head: Vec::new(), rows: Vec::new() },
        };
        let adapters = self.tl.shown_adapters().into_iter().map(|(i, rank)| (i, rank, self.tl.adapters[i].total)).collect();
        let mut disks: Vec<(u32, usize)> = self.tl.disks.iter().enumerate().filter_map(|(i, d)| Some((d.id.parse().ok()?, i))).collect();
        disks.sort_unstable();
        Some(export::Job { kind, format, encoding, range, table, terms: self.filter.clone(), offset: self.tl.offset, disks, adapters })
    }
}

/// The name of a column in exported files: snake_case, the unit at the end.
fn export_name(col: AnyCol) -> String {
    match col {
        AnyCol::P(c) => match c {
            Col::Cpu => "cpu_ratio".into(),
            Col::GpuUsage => "gpu_ratio".into(),
            Col::CpuTime => "cpu_time_seconds".into(),
            Col::MemActivePct | Col::MemPrivatePct | Col::DiskPct | Col::NetPct => format!("{}_ratio", columns::def(c).key),
            Col::DiskRate | Col::Network => format!("{}_bytes_per_second", columns::def(c).key),
            Col::WsDelta
            | Col::ActivePrivate
            | Col::Private
            | Col::Ws
            | Col::PeakWs
            | Col::SharedWs
            | Col::Commit
            | Col::Paged
            | Col::NonPaged
            | Col::IoReadBytes
            | Col::IoWriteBytes
            | Col::IoOtherBytes
            | Col::GpuDedicated
            | Col::GpuShared => format!("{}_bytes", columns::def(c).key),
            _ => columns::def(c).key.to_string(),
        },
        AnyCol::R(c) => {
            let unit = match c.item {
                RItem::Cpu | RItem::Gpu => "_ratio",
                RItem::Disk | RItem::Network => "_bytes_per_second",
                RItem::Faults => "_per_second",
                RItem::CpuTime | RItem::Hung => "_seconds",
                RItem::Mem
                | RItem::Private
                | RItem::Ws
                | RItem::Commit
                | RItem::Paged
                | RItem::NonPaged
                | RItem::GpuMem
                | RItem::GpuShared
                | RItem::IoReadBytes
                | RItem::IoWriteBytes
                | RItem::IoOtherBytes => "_bytes",
                _ => "",
            };
            match c.op {
                ROp::None => format!("{}{unit}", c.item.key()),
                op => format!("{}_{}{unit}", c.item.key(), op.key()),
            }
        }
    }
}

impl DetailsPage {
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
    /// While the graph is on it records wherever the user is, minimized too: the machine,
    /// the processes, and what the shown columns need, so that the past has them too.
    fn demand(&self, s: &Settings) -> crate::system::demand::Needs {
        use crate::system::demand::need;
        let cols = columns::demand(&s.columns);
        let always = if s.tm_enabled {
            let gpu = timeline::target::targets(s).iter().any(|t| t.uses_gpu());
            let items = s.range_items.iter().fold(0, |n, (i, _)| {
                n | match i {
                    RItem::Gpu | RItem::GpuMem | RItem::GpuShared => need::PROC_GPU,
                    RItem::Network => need::PROC_NET,
                    RItem::UserObjects | RItem::GdiObjects => need::GUI_COUNTS,
                    _ => 0,
                }
            });
            need::SYS | need::PROCS | cols | items | if gpu { need::SYS_GPU | need::PROC_GPU } else { 0 }
        } else {
            0
        };
        crate::system::demand::Needs { shown: need::PROCS | cols, always }
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

    fn captures_text(&self) -> bool {
        self.tl.captures_text()
    }

    fn on_snapshot(&mut self, snap: &Arc<Snapshot>, cx: &mut Cx) {
        if self.tl.wants(snap, cx.settings) {
            self.tl.record(snap, cx.settings);
        }
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

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.drag = Drag::None;
    }

    fn pannable(&self, x: i32, y: i32) -> bool {
        matches!(self.overlay, Overlay::None)
            && !self.tl.overlay_open()
            && self.l.body.contains(x, y)
            && !self.vthumb().is_some_and(|t| t.contains(x, y))
            && !self.hthumb().is_some_and(|t| t.contains(x, y))
    }

    fn cursor(&self) -> Cursor {
        if self.hover_edge || matches!(self.drag, Drag::Resize { .. }) {
            Cursor::SizeWE
        } else if self.hover_split || matches!(self.drag, Drag::Split { .. }) {
            Cursor::SizeNS
        } else {
            Cursor::Arrow
        }
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Ev::Move { x, y } = *ev {
            self.mouse = (x, y);
        }
        // The graph's dialogs and menus first, then the table's.
        let out = self.tl.overlay_event(ev, cx);
        if out != timeline::Out::Pass {
            return self.after_timeline(out, cx);
        }
        if !matches!(ev, Ev::Tick { .. }) && self.overlay_event(ev, cx) {
            return;
        }
        // The graph and the paused bar take what happens over them; a move goes to both.
        let to_graph = match *ev {
            Ev::Move { .. } | Ev::Leave | Ev::Up { .. } => true,
            Ev::Down { x, y, .. } => self.l.graph.contains(x, y) || self.l.bar_area.contains(x, y),
            Ev::Wheel { .. } => self.tl.over_plot(self.mouse.0, self.mouse.1),
            Ev::Key { vk: VK_ESCAPE, .. } => true,
            _ => false,
        };
        if to_graph && self.drag == Drag::None {
            let out = self.tl.event(ev, cx);
            if !matches!(ev, Ev::Move { .. } | Ev::Leave) && out != timeline::Out::Pass {
                return self.after_timeline(out, cx);
            }
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
        if settings.tm_enabled {
            self.tl.paint_bar(c, self.l.bar_area);
            self.tl.paint_graph(c, self.l.graph, settings);
            if self.hover_split || matches!(self.drag, Drag::Split { .. }) {
                let s = self.l.split;
                let w = c.px(40);
                c.round(rect(s.x + (s.w - w) / 2, s.y + s.h / 2 - 1, w, c.px(3)), 1.5, THEME.text_faint, 255);
            }
        }
        self.paint_header(c, settings);
        self.paint_rows(c);
        if self.range.is_some() {
            self.paint_time_tip(c);
        }
        // Over the table: the tooltip sits below the pointer and may run past the graph.
        self.tl.paint_hover(c, area, settings);
        self.paint_overlay(c, settings);
        self.tl.paint_overlays(c, area, settings);
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
            taken: old.taken,
            stale: old.stale,
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

#[cfg(test)]
mod tests {
    use super::timeline::export::{Encoding, Format, Kind};
    use super::timeline::tests::{proc_of, rec};
    use super::timeline::Scope;
    use super::*;

    /// A page that recorded three snapshots: 1 runs throughout, 2 ends, 3 starts.
    fn recorded(s: &Settings) -> DetailsPage {
        let mut page = DetailsPage::new();
        for pids in [&[1u32, 2][..], &[1, 2], &[1, 3]] {
            let mut snap = Snapshot { has_procs: true, procs: pids.iter().map(|p| proc_of(&rec(*p, *p as f32, 100 * *p))).collect(), ..Default::default() };
            snap.sys.elapsed = 1.0;
            let snap = Arc::new(snap);
            page.tl.record(&snap, s);
            page.live = snap;
        }
        page
    }

    fn csv(page: &DetailsPage, s: &Settings, scope: Scope, kind: Kind) -> Vec<String> {
        let job = page.export_job(s, (scope, kind, Format::Csv, Encoding::Utf8Bom)).expect("a job");
        let mut buf = Vec::new();
        job.write(&mut buf).unwrap();
        String::from_utf8(buf[3..].to_vec()).unwrap().split("\r\n").filter(|l| !l.is_empty()).map(String::from).collect()
    }

    #[test]
    fn a_picked_time_shows_that_moment_and_ends_only_what_still_runs() {
        let s = Settings::default();
        let mut page = recorded(&s);
        page.tl.select_for_test(0, 0, &s);
        page.show_selection(&s);
        assert!(page.past && page.range.is_none());
        let pids: Vec<u32> = page.snap.procs.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [1, 2]);
        // 2 has ended since: it is shown but cannot be ended.
        assert!(page.alive(page.snap.procs[0].key()));
        assert!(!page.alive(page.snap.procs[1].key()));
        // Showing a running process (from another page, or aim mode) goes back to live first:
        // 3 started after the picked time.
        assert_eq!(page.reveal(3), Reveal::Missing);
        page.go_live(&s);
        assert!(!page.past && page.snap.procs.iter().any(|p| p.pid == 3));
        assert_eq!(page.reveal(3), Reveal::Shown);
    }

    #[test]
    fn exports_of_a_range() {
        let s = Settings { view: ViewMode::Flat, ..Default::default() };
        let mut page = recorded(&s);
        page.tl.select_for_test(0, 2, &s);
        page.show_selection(&s);
        assert!(page.range.is_some());

        // The range table as shown: the default items and operations, heaviest CPU first.
        let table = csv(&page, &s, Scope::Selection, Kind::Table);
        assert_eq!(table[0], "name,pid,mark,time,cpu_start_ratio,cpu_end_ratio,cpu_delta_ratio,cpu_avg_ratio,cpu_max_ratio,mem_start_bytes,mem_end_bytes,mem_delta_bytes,mem_avg_bytes,mem_max_bytes");
        assert!(table[1].starts_with("p3,3,+,"));
        assert!(table[2].starts_with("p2,2,-,"));
        assert!(table[3].starts_with("p1,1,,,0.01,0.01,0,0.01,0.01,1048576,"));
        assert_eq!(table.len(), 4);
        // The whole record summed up, the same way.
        assert_eq!(csv(&page, &s, Scope::All, Kind::Table), table);
        // Each process at each sample, and the machine at each sample.
        assert_eq!(csv(&page, &s, Scope::Selection, Kind::Process).len(), 1 + 6);
        assert_eq!(csv(&page, &s, Scope::Selection, Kind::System).len(), 1 + 3);
    }

    #[test]
    fn a_picked_time_exports_the_shown_columns_without_private_ones() {
        let s = Settings { columns: vec![(Col::Name, 200), (Col::User, 100), (Col::Cpu, 60), (Col::ActivePrivate, 100)], ..Default::default() };
        let mut page = recorded(&s);
        page.tl.select_for_test(2, 2, &s);
        page.show_selection(&s);
        let table = csv(&page, &s, Scope::Selection, Kind::Table);
        // No user name; the CPU as a ratio, memory in bytes.
        assert_eq!(table[0], "name,cpu_ratio,active_bytes");
        assert_eq!(table.len(), 3);
        assert!(table.contains(&"p3,0.03,1048576".to_string()));
    }

    #[test]
    fn while_on_it_reads_what_the_shown_columns_need() {
        use crate::system::demand::need;
        let mut s = Settings { columns: vec![(Col::Name, 200), (Col::GpuUsage, 70)], ..Default::default() };
        let page = DetailsPage::new();
        let d = page.demand(&s);
        assert_eq!(d.always & (need::PROCS | need::PROC_GPU), need::PROCS | need::PROC_GPU);
        // The range table's items count too.
        s.columns = columns::defaults();
        s.range_items.push((RItem::Network, 80));
        assert_ne!(page.demand(&s).always & need::PROC_NET, 0);
        // Off, it reads nothing while not shown.
        s.tm_enabled = false;
        assert_eq!(page.demand(&s).always, 0);
    }
}
