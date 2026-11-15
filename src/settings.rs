//! Settings persisted to %APPDATA%\LightTaskManager\settings.ini.

use crate::i18n::{tr, Lang};
use crate::pages::details::columns::{self, Col};
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
    /// The time machine keeps recording.
    pub tm_enabled: bool,
    /// How long the time machine keeps samples, in minutes.
    pub tm_minutes: u32,
    /// The time machine's targets, by key (see `pages::timemachine`), in no particular order.
    pub tm_targets: Vec<String>,
    /// The time machine export's last choices: "table" / "process" / "system", "csv" / "tsv", "utf8" / "sjis".
    pub tm_export_kind: String,
    pub tm_export_format: String,
    pub tm_export_encoding: String,
    /// The language of the user interface, or None to follow Windows.
    pub language: Option<Lang>,
}

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
            tm_enabled: true,
            tm_minutes: 5,
            tm_targets: vec!["cpu".into(), "mem".into()],
            tm_export_kind: "table".into(),
            tm_export_format: "csv".into(),
            tm_export_encoding: "utf8".into(),
            language: None,
        }
    }
}

fn file() -> Option<PathBuf> {
    let dir = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(dir).join("LightTaskManager").join("settings.ini"))
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
                "tm_enabled" => s.tm_enabled = value == "1",
                "tm_minutes" => s.tm_minutes = value.parse::<u32>().unwrap_or(5).max(1),
                "tm_export_kind" => s.tm_export_kind = value.to_string(),
                "tm_export_format" => s.tm_export_format = value.to_string(),
                "tm_export_encoding" => s.tm_export_encoding = value.to_string(),
                "language" => s.language = Lang::from_key(value),
                "tm_targets" => s.tm_targets = value.split(',').filter(|k| !k.is_empty()).map(String::from).collect(),
                // Before the list, each target had its own key.
                "tm_cpu" | "tm_mem" | "tm_gpu" => {
                    let k = &key[3..];
                    s.tm_targets.retain(|t| t != k);
                    if value == "1" {
                        s.tm_targets.push(k.to_string());
                    }
                }
                _ => {}
            }
        }
        s
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
        let text = format!(
            "columns={}\nsort={}\ndescending={}\nview={}\nhighlight_lifetime={}\ninterval={}\nsidebar={}\nwidth={}\nheight={}\nmaximized={}\ncopy_format={}\ncopy_scope={}\nelevate_on_start={}\ntm_enabled={}\ntm_minutes={}\ntm_targets={}\ntm_export_kind={}\ntm_export_format={}\ntm_export_encoding={}\nlanguage={}\n",
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
            self.tm_enabled as u8,
            self.tm_minutes,
            self.tm_targets.join(","),
            self.tm_export_kind,
            self.tm_export_format,
            self.tm_export_encoding,
            self.language.map_or("auto", Lang::key),
        );
        let _ = std::fs::write(path, text);
    }
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
