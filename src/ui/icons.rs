//! Segoe Fluent Icons code points used across the UI.

pub const MENU: char = '\u{E700}';
pub const SEARCH: char = '\u{E721}';
pub const CLOSE: char = '\u{E8BB}';
pub const MINIMIZE: char = '\u{E921}';
pub const MAXIMIZE: char = '\u{E922}';
pub const RESTORE: char = '\u{E923}';
pub const PROCESSES: char = '\u{ECAA}';
pub const CHART: char = '\u{E9D9}';
pub const SETTINGS: char = '\u{E713}';
pub const CHEVRON_UP: char = '\u{E70E}';
pub const CHEVRON_DOWN: char = '\u{E70D}';
pub const CHEVRON_RIGHT: char = '\u{E76C}';
pub const CHECK: char = '\u{E73E}';
pub const BLOCK: char = '\u{E8D8}';
pub const COLUMNS: char = '\u{E9D5}';
pub const CLEAR: char = '\u{E711}';
pub const HISTORY: char = '\u{E81C}';
pub const TIMELINE: char = '\u{E9D2}';
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
pub const SAVE: char = '\u{E74E}';

pub fn glyph(ch: char) -> String {
    ch.to_string()
}
