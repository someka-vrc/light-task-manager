//! Settings persisted to %APPDATA%\LightTaskManager\settings.ini.

use crate::columns::{self, Col};
use std::path::PathBuf;

pub struct Settings {
    pub columns: Vec<(Col, u16)>,
    pub sort: Col,
    pub descending: bool,
    pub interval_ms: u32,
    pub sidebar_expanded: bool,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            columns: columns::defaults(),
            sort: Col::ActivePrivate,
            descending: true,
            interval_ms: 1000,
            sidebar_expanded: false,
            width: 1240,
            height: 760,
            maximized: false,
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
                    let parsed = parse_columns(value);
                    if !parsed.is_empty() {
                        s.columns = parsed;
                    }
                }
                "sort" => {
                    if let Some(d) = columns::by_key(value) {
                        s.sort = d.id;
                    }
                }
                "descending" => s.descending = value == "1",
                "interval" => s.interval_ms = value.parse().unwrap_or(1000),
                "sidebar" => s.sidebar_expanded = value == "1",
                "width" => s.width = value.parse().unwrap_or(s.width),
                "height" => s.height = value.parse().unwrap_or(s.height),
                "maximized" => s.maximized = value == "1",
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
            "columns={}\nsort={}\ndescending={}\ninterval={}\nsidebar={}\nwidth={}\nheight={}\nmaximized={}\n",
            cols.join(","),
            columns::def(self.sort).key,
            self.descending as u8,
            self.interval_ms,
            self.sidebar_expanded as u8,
            self.width,
            self.height,
            self.maximized as u8,
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
