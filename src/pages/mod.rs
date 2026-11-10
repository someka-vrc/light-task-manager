//! Page abstraction. To add a page, implement `Page` and list it in `all()`.

pub mod details;
pub mod history;
pub mod performance;
pub mod services;
pub mod settings;
pub mod startup;
pub mod users;
pub mod widgets;

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
    /// A finger or pen dragged the content, in pixels.
    Pan { dx: f32, dy: f32 },
    /// The contact lifted while moving, in pixels per second. Zero stops a fling.
    Fling { vx: f32, vy: f32 },
    Key { vk: u16, ctrl: bool },
    Tick { dt: f32 },
}

pub enum Cmd {
    Toast(String),
    FocusFilter,
    Elevate,
    /// Switch to the details page filtered to this PID.
    ShowProcess(u32),
}

pub struct Cx<'a> {
    pub settings: &'a mut Settings,
    pub shared: &'a Shared,
    pub cmds: &'a mut Vec<Cmd>,
    pub dirty: &'a mut bool,
    /// The latest input came from a finger, so hit areas should be generous.
    pub touch: bool,
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
    /// Called when the page becomes the active page.
    fn on_show(&mut self, _cx: &mut Cx) {}
    /// Called when another page becomes active.
    fn on_hide(&mut self, _cx: &mut Cx) {}
    /// Called once when the window closes, before settings are saved.
    fn on_exit(&mut self) {}
    fn animating(&self) -> bool {
        false
    }
    fn cursor(&self) -> Cursor {
        Cursor::Arrow
    }
    /// Whether dragging a finger or pen at (x, y) should scroll the content.
    fn pannable(&self, _x: i32, _y: i32) -> bool {
        false
    }
}

pub fn all() -> Vec<Box<dyn Page>> {
    vec![
        Box::new(details::DetailsPage::new()),
        Box::new(performance::PerformancePage::new()),
        Box::new(history::HistoryPage::new()),
        Box::new(startup::StartupPage::new()),
        Box::new(users::UsersPage::new()),
        Box::new(services::ServicesPage::new()),
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
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
    pub const GROUP: char = '\u{F168}';
    pub const CHECK: char = '\u{E73E}';
    pub const BLOCK: char = '\u{E8D8}';
    pub const COLUMNS: char = '\u{E9D5}';
    pub const CLEAR: char = '\u{E711}';
    pub const HISTORY: char = '\u{E81C}';
    pub const STARTUP: char = '\u{EC4A}';
    pub const PEOPLE: char = '\u{E716}';
    pub const SERVICES: char = '\u{E912}';
    pub const PLAY: char = '\u{E768}';
    pub const STOP: char = '\u{E71A}';
    pub const REFRESH: char = '\u{E72C}';
    pub const FOLDER: char = '\u{E838}';
    pub const DISCONNECT: char = '\u{E8CD}';
    pub const SIGN_OUT: char = '\u{F3B1}';
    pub const DELETE: char = '\u{E74D}';
}

pub fn glyph(ch: char) -> String {
    ch.to_string()
}
