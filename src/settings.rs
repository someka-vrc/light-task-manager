//! Settings persisted to %APPDATA%\LightTaskManager\settings.ini.

use crate::i18n::{tr, Lang};
use crate::pages::details::columns::{self, Col};
use crate::pages::details::range::{self, RCol, RItem};
use crate::ui::copyfmt::{Format, Scope};
use std::path::PathBuf;

/// How the process page arranges its rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Flat,
    /// Task Manager style groups (see `crate::pages::details::grouping`).
    Group,
    /// Parent and child processes.
    Tree,
}

impl ViewMode {
    pub const ALL: [ViewMode; 3] = [ViewMode::Flat, ViewMode::Group, ViewMode::Tree];

    pub fn key(self) -> &'static str {
        match self {
            ViewMode::Flat => "flat",
            ViewMode::Group => "group",
            ViewMode::Tree => "tree",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Flat => tr().details.view_flat,
            ViewMode::Group => tr().details.view_group,
            ViewMode::Tree => tr().details.view_tree,
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }
}

pub struct Settings {
    pub columns: Vec<(Col, u16)>,
    pub sort: Col,
    pub descending: bool,
    pub view: ViewMode,
    /// Tint processes that just started or ended for a while.
    pub highlight_lifetime: bool,
    pub interval_ms: u32,
    pub sidebar_expanded: bool,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    /// Layout of the text copied from a row.
    pub copy_format: Format,
    /// Which properties of a row are copied.
    pub copy_scope: Scope,
    /// Start again as administrator when not already elevated.
    pub elevate_on_start: bool,
    /// Keep an icon in the notification area; the close button then hides the window there.
    pub tray: bool,
    /// The icon in the notification area is a bar graph of the CPU, memory and GPU. Kept
    /// while `tray` is off, so that turning it back on brings the same icon.
    pub tray_graph: bool,
    /// The graph over the process table is on, and keeps recording.
    pub tm_enabled: bool,
    /// Ask before turning the graph off (which discards what it recorded).
    pub tm_confirm_off: bool,
    /// How long the graph keeps samples, in minutes.
    pub tm_minutes: u32,
    /// The graph's lines, by key (see `pages::details::timeline::target`), in no particular order.
    pub tm_targets: Vec<String>,
    /// The graph's height, in unscaled pixels.
    pub tm_graph_h: u16,
    /// The range table's items with their widths, in column order.
    pub range_items: Vec<(RItem, u16)>,
    /// The range table's operations, one bit each (see `range::ROp::bit`).
    pub range_ops: u8,
    pub range_sort: RCol,
    pub range_desc: bool,
    /// The export's last choices: "table" / "process" / "system", "csv" / "tsv", "utf8" / "sjis".
    pub tm_export_kind: String,
    pub tm_export_format: String,
    pub tm_export_encoding: String,
    /// The language of the user interface, or None to follow Windows.
    pub language: Option<Lang>,
    /// The recording to the database (DBへの常時書き出し) is on.
    pub rec_enabled: bool,
    /// Its interval, in seconds.
    pub rec_seconds: u32,
    /// How many days of files are kept, today included.
    pub rec_days: u32,
    /// Record each process's path, command line and user name.
    pub rec_detail: bool,
    /// Record the GPU use.
    pub rec_gpu: bool,
    /// The folder of the files; empty for the default one.
    pub rec_dir: String,
    /// The folder the recording writes to in this run (not saved): the settings apply at the next start.
    pub rec_running: Option<PathBuf>,
}

/// The intervals and retention periods the settings page offers.
pub const REC_SECONDS: [u32; 5] = [1, 5, 10, 30, 60];

impl Default for Settings {
    fn default() -> Self {
        Self {
            columns: columns::defaults(),
            sort: Col::ActivePrivate,
            descending: true,
            view: ViewMode::Group,
            highlight_lifetime: true,
            interval_ms: 1000,
            sidebar_expanded: false,
            width: 1240,
            height: 760,
            maximized: false,
            copy_format: Format::Standard,
            copy_scope: Scope::Visible,
            elevate_on_start: false,
            tray: false,
            tray_graph: false,
            tm_enabled: true,
            tm_confirm_off: true,
            tm_minutes: 1,
            tm_targets: vec!["cpu".into(), "mem".into()],
            tm_graph_h: 220,
            range_items: range::default_items(),
            range_ops: range::DEFAULT_OPS,
            // The heaviest first: the range table is for finding who was busy.
            range_sort: RCol { item: RItem::Cpu, op: range::ROp::Max },
            range_desc: true,
            tm_export_kind: "table".into(),
            tm_export_format: "csv".into(),
            tm_export_encoding: "utf8".into(),
            language: None,
            rec_enabled: false,
            rec_seconds: 5,
            rec_days: 3,
            rec_detail: false,
            rec_gpu: false,
            rec_dir: String::new(),
            rec_running: None,
        }
    }
}

fn file() -> Option<PathBuf> {
    let dir = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(dir).join("LightTaskManager").join("settings.ini"))
}

/// Where the recording's settings wait for the next start.
fn pending_file() -> Option<PathBuf> {
    file().map(|p| p.with_file_name("recording.ini"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = file().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return s;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "columns" => {
                    let mut parsed = parse_columns(value);
                    if !parsed.is_empty() {
                        // The name column is always shown, even if an old file omits it.
                        if !parsed.iter().any(|(c, _)| *c == Col::Name) {
                            parsed.insert(0, (Col::Name, columns::def(Col::Name).width));
                        }
                        s.columns = parsed;
                    }
                }
                "sort" => {
                    if let Some(d) = columns::by_key(value) {
                        s.sort = d.id;
                    }
                }
                "descending" => s.descending = value == "1",
                // Before the view modes, grouping was on or off.
                "group" => s.view = if value == "1" { ViewMode::Group } else { ViewMode::Flat },
                "view" => s.view = ViewMode::from_key(value).unwrap_or(ViewMode::Group),
                "highlight_lifetime" => s.highlight_lifetime = value == "1",
                "interval" => s.interval_ms = value.parse().unwrap_or(1000),
                "sidebar" => s.sidebar_expanded = value == "1",
                "width" => s.width = value.parse().unwrap_or(s.width),
                "height" => s.height = value.parse().unwrap_or(s.height),
                "maximized" => s.maximized = value == "1",
                "copy_format" => s.copy_format = Format::from_key(value).unwrap_or_default(),
                "copy_scope" => s.copy_scope = Scope::from_key(value).unwrap_or_default(),
                "elevate_on_start" => s.elevate_on_start = value == "1",
                "tray" => s.tray = value == "1",
                "tray_graph" => s.tray_graph = value == "1",
                "tm_enabled" => s.tm_enabled = value == "1",
                "tm_confirm_off" => s.tm_confirm_off = value == "1",
                "tm_minutes" => s.tm_minutes = value.parse::<u32>().unwrap_or(1).max(1),
                "tm_graph_h" => s.tm_graph_h = value.parse::<u16>().unwrap_or(220).clamp(120, 2000),
                "range_items" => {
                    let mut items = parse_range_items(value);
                    if !items.is_empty() {
                        // The name is always shown.
                        if !items.iter().any(|(i, _)| *i == RItem::Name) {
                            items.insert(0, (RItem::Name, RItem::Name.default_width()));
                        }
                        s.range_items = items;
                    }
                }
                "range_ops" => s.range_ops = value.parse::<u8>().map(|v| v & 0b11_1111).ok().filter(|v| *v != 0).unwrap_or(range::DEFAULT_OPS),
                "range_sort" => s.range_sort = RCol::from_key(value).unwrap_or(s.range_sort),
                "range_desc" => s.range_desc = value == "1",
                "tm_export_kind" => s.tm_export_kind = value.to_string(),
                "tm_export_format" => s.tm_export_format = value.to_string(),
                "tm_export_encoding" => s.tm_export_encoding = value.to_string(),
                "language" => s.language = Lang::from_key(value),
                k if s.set_recording(k, value) => {}
                // Only the graph's lines: the items that were only in the table are dropped.
                "tm_targets" => {
                    use crate::pages::details::timeline::target::ORDER;
                    s.tm_targets = value.split(',').filter(|k| ORDER.iter().any(|t| t.key() == *k)).map(String::from).collect();
                }
                _ => {}
            }
        }
        s
    }

    /// Sets one of the recording's settings from a line of a file. False for any other key.
    fn set_recording(&mut self, key: &str, value: &str) -> bool {
        match key {
            "rec_enabled" => self.rec_enabled = value == "1",
            "rec_seconds" => self.rec_seconds = nearest(&REC_SECONDS, value.parse().unwrap_or(5)),
            "rec_days" => self.rec_days = value.parse::<u32>().unwrap_or(3).clamp(1, 3650),
            "rec_detail" => self.rec_detail = value == "1",
            "rec_gpu" => self.rec_gpu = value == "1",
            "rec_dir" => self.rec_dir = value.to_string(),
            _ => return false,
        }
        true
    }

    /// The recording's settings as lines of a file.
    fn recording_text(&self) -> String {
        format!(
            "rec_enabled={}\nrec_seconds={}\nrec_days={}\nrec_detail={}\nrec_gpu={}\nrec_dir={}\n",
            self.rec_enabled as u8, self.rec_seconds, self.rec_days, self.rec_detail as u8, self.rec_gpu as u8, self.rec_dir
        )
    }

    /// The recording's settings apply at the next start, and the app may end without saving
    /// settings.ini: a change is written to its own small file at once, and the next start takes
    /// it from there.
    pub fn save_recording(&self) {
        let Some(path) = pending_file() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.recording_text());
    }

    /// At the start: takes the recording's settings changed in the run before. They go to
    /// settings.ini at once, and only then does the small file go, so that nothing is lost
    /// if the app ends before it saves.
    pub fn apply_pending(&mut self) {
        let Some(path) = pending_file() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
            self.set_recording(key, value);
        }
        self.save();
        let _ = std::fs::remove_file(path);
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|(c, w)| format!("{}:{}", columns::def(*c).key, w))
            .collect();
        let range_items: Vec<String> = self.range_items.iter().map(|(i, w)| format!("{}:{}", i.key(), w)).collect();
        let text = format!(
            "columns={}\nsort={}\ndescending={}\nview={}\nhighlight_lifetime={}\ninterval={}\nsidebar={}\nwidth={}\nheight={}\nmaximized={}\ncopy_format={}\ncopy_scope={}\nelevate_on_start={}\ntray={}\ntray_graph={}\ntm_enabled={}\ntm_confirm_off={}\ntm_minutes={}\ntm_targets={}\ntm_graph_h={}\nrange_items={}\nrange_ops={}\nrange_sort={}\nrange_desc={}\ntm_export_kind={}\ntm_export_format={}\ntm_export_encoding={}\nlanguage={}\nrec_enabled={}\nrec_seconds={}\nrec_days={}\nrec_detail={}\nrec_gpu={}\nrec_dir={}\n",
            cols.join(","),
            columns::def(self.sort).key,
            self.descending as u8,
            self.view.key(),
            self.highlight_lifetime as u8,
            self.interval_ms,
            self.sidebar_expanded as u8,
            self.width,
            self.height,
            self.maximized as u8,
            self.copy_format.key(),
            self.copy_scope.key(),
            self.elevate_on_start as u8,
            self.tray as u8,
            self.tray_graph as u8,
            self.tm_enabled as u8,
            self.tm_confirm_off as u8,
            self.tm_minutes,
            self.tm_targets.join(","),
            self.tm_graph_h,
            range_items.join(","),
            self.range_ops,
            self.range_sort.key(),
            self.range_desc as u8,
            self.tm_export_kind,
            self.tm_export_format,
            self.tm_export_encoding,
            self.language.map_or("auto", Lang::key),
            self.rec_enabled as u8,
            self.rec_seconds,
            self.rec_days,
            self.rec_detail as u8,
            self.rec_gpu as u8,
            self.rec_dir,
        );
        let _ = std::fs::write(path, text);
    }
}

/// The offered choice closest to `value`, so that a hand-edited file still gives a valid one.
fn nearest(choices: &[u32], value: u32) -> u32 {
    choices.iter().copied().min_by_key(|c| c.abs_diff(value)).unwrap_or(value)
}

fn parse_range_items(value: &str) -> Vec<(RItem, u16)> {
    let mut out: Vec<(RItem, u16)> = Vec::new();
    for entry in value.split(',') {
        let Some((key, width)) = entry.split_once(':') else { continue };
        if let (Some(item), Ok(w)) = (RItem::from_key(key), width.parse::<u16>()) {
            if !out.iter().any(|(i, _)| *i == item) {
                out.push((item, w.clamp(40, 2000)));
            }
        }
    }
    out
}

fn parse_columns(value: &str) -> Vec<(Col, u16)> {
    let mut out: Vec<(Col, u16)> = Vec::new();
    for item in value.split(',') {
        let Some((key, width)) = item.split_once(':') else {
            continue;
        };
        if let (Some(d), Ok(w)) = (columns::by_key(key), width.parse::<u16>()) {
            if !out.iter().any(|(c, _)| *c == d.id) {
                out.push((d.id, w.clamp(40, 2000)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_settings_wait_in_their_own_file_until_the_next_start() {
        let dir = std::env::temp_dir().join(format!("ltm-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("APPDATA", &dir);

        // A change is on disk at once, though settings.ini is not saved yet.
        let s = Settings { rec_enabled: true, rec_seconds: 30, rec_days: 7, rec_dir: "D:/history".into(), ..Default::default() };
        s.save_recording();
        assert!(pending_file().unwrap().exists());
        assert!(!file().unwrap().exists());

        // The next start takes them, saves settings.ini, and only then drops the small file.
        let mut next = Settings::load();
        assert!(!next.rec_enabled);
        next.apply_pending();
        assert!(next.rec_enabled && next.rec_seconds == 30 && next.rec_days == 7 && next.rec_dir == "D:/history");
        assert!(!pending_file().unwrap().exists());
        let again = Settings::load();
        assert!(again.rec_enabled && again.rec_seconds == 30 && again.rec_dir == "D:/history");
        // Nothing waiting: nothing changes.
        let mut other = Settings::load();
        other.apply_pending();
        assert!(other.rec_enabled);

        // The graph's and the range table's settings come back as they were.
        other.tm_confirm_off = false;
        other.tm_graph_h = 333;
        other.range_items = vec![(RItem::Name, 210), (RItem::IoReadBytes, 99), (RItem::Cpu, 61)];
        other.range_ops = range::ROp::Min.bit() | range::ROp::Max.bit();
        other.range_sort = RCol { item: RItem::IoReadBytes, op: range::ROp::Change };
        other.range_desc = false;
        other.save();
        let back = Settings::load();
        assert!(!back.tm_confirm_off && back.tm_graph_h == 333 && !back.range_desc);
        assert_eq!(back.range_items, other.range_items);
        assert_eq!(back.range_ops, other.range_ops);
        assert_eq!(back.range_sort, other.range_sort);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
