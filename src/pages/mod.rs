//! Page abstraction. To add a page, implement `Page` and list it in `all()`.

pub mod details;
pub mod history;
mod licenses;
pub mod performance;
pub mod services;
pub mod settings;
pub mod startup;
pub mod users;

use crate::settings::Settings;
use crate::system::model::Snapshot;
use crate::system::sampler::Shared;
use crate::ui::gfx::{Canvas, Rect};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub enum Ev {
    Move { x: i32, y: i32 },
    Leave,
    Down { x: i32, y: i32, right: bool },
    Up { x: i32 },
    /// In wheel notches, positive is away from the user. `ctrl` is held for zooming.
    Wheel { dx: f32, dy: f32, ctrl: bool },
    /// A finger or pen dragged the content, in pixels.
    Pan { dx: f32, dy: f32 },
    /// The contact lifted while moving, in pixels per second. Zero stops a fling.
    Fling { vx: f32, vy: f32 },
    Key { vk: u16, ctrl: bool },
    /// A typed character, sent only while the page `captures_text`.
    Char { ch: char },
    Tick { dt: f32 },
}

pub enum Cmd {
    Toast(String),
    FocusFilter,
    Elevate,
    /// Ask for the recording's folder (`Settings::rec_dir`) and take it. Done by the window, since the
    /// folder dialog runs its own message loop.
    PickRecDir,
    /// Open the recording's folder in Explorer, making it first when it is not there yet.
    OpenRecDir,
    /// `Settings::language` changed: switch to it, with its fonts.
    Language,
    /// Switch to the details page filtered to this PID.
    ShowProcess(u32),
    /// Start aim mode: the next click on any window picks its process.
    Aim,
    /// Ask where to save a file, then write it. Done by the window once the event is over,
    /// since the file dialog runs its own message loop.
    Save(SaveRequest),
}

pub struct SaveRequest {
    /// The suggested file name, with its extension.
    pub name: String,
    /// The file type for the dialog's filter, like "CSV".
    pub kind: &'static str,
    /// The extension without the dot.
    pub ext: &'static str,
    /// Writes the content once a file is chosen.
    pub write: Box<WriteFn>,
}

/// Writes a file's content.
pub type WriteFn = dyn FnOnce(&mut dyn std::io::Write) -> std::io::Result<()>;

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
    SizeNS,
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
    /// What this page needs the sampler to read; see `crate::system::demand`.
    fn demand(&self, _settings: &Settings) -> crate::system::demand::Needs {
        crate::system::demand::Needs::default()
    }
    /// Typed characters go to the page (as `Ev::Char`) instead of the search box.
    fn captures_text(&self) -> bool {
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
    /// A "!" on the page's entry in the sidebar: something needs attention there.
    fn badge(&self) -> bool {
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
