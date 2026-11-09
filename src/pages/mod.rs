//! Page abstraction. To add a page, implement `Page` and list it in `all()`.

pub mod details;
pub mod performance;
pub mod settings;

use crate::gfx::{Canvas, Rect};
use crate::model::Snapshot;
use crate::sampler::Shared;
use crate::settings::Settings;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub enum Ev {
    Move { x: i32, y: i32 },
    Leave,
    Down { x: i32, y: i32, right: bool },
    Up { x: i32 },
    /// In wheel notches, positive is away from the user.
    Wheel { dx: f32, dy: f32 },
    Key { vk: u16, ctrl: bool },
    Tick { dt: f32 },
}

pub enum Cmd {
    Toast(String),
    FocusFilter,
    Elevate,
}

pub struct Cx<'a> {
    pub settings: &'a mut Settings,
    pub shared: &'a Shared,
    pub cmds: &'a mut Vec<Cmd>,
    pub dirty: &'a mut bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    SizeWE,
    IBeam,
}

pub struct Nav {
    pub icon: char,
    pub label: &'static str,
    /// Pinned to the bottom of the sidebar.
    pub bottom: bool,
}

pub trait Page {
    fn nav(&self) -> Nav;
    fn paint(&mut self, c: &mut Canvas, area: Rect, settings: &Settings);
    fn event(&mut self, _ev: &Ev, _cx: &mut Cx) {}
    fn wants_filter(&self) -> bool {
        false
    }
    fn set_filter(&mut self, _text: &str, _cx: &mut Cx) {}
    fn on_snapshot(&mut self, _snap: &Arc<Snapshot>, _cx: &mut Cx) {}
    fn on_enriched(&mut self, _cx: &mut Cx) {}
    fn animating(&self) -> bool {
        false
    }
    fn cursor(&self) -> Cursor {
        Cursor::Arrow
    }
}

pub fn all() -> Vec<Box<dyn Page>> {
    vec![
        Box::new(details::DetailsPage::new()),
        Box::new(performance::PerformancePage),
        Box::new(settings::SettingsPage::new()),
    ]
}

pub mod icons {
    pub const MENU: char = '\u{E700}';
    pub const SEARCH: char = '\u{E721}';
    pub const CLOSE: char = '\u{E8BB}';
    pub const MINIMIZE: char = '\u{E921}';
    pub const MAXIMIZE: char = '\u{E922}';
    pub const RESTORE: char = '\u{E923}';
    pub const LIST: char = '\u{E8FD}';
    pub const CHART: char = '\u{E9D9}';
    pub const SETTINGS: char = '\u{E713}';
    pub const CHEVRON_UP: char = '\u{E70E}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const CHECK: char = '\u{E73E}';
    pub const BLOCK: char = '\u{E8D8}';
    pub const COLUMNS: char = '\u{E9D5}';
    pub const CLEAR: char = '\u{E711}';
}

pub fn glyph(ch: char) -> String {
    ch.to_string()
}
