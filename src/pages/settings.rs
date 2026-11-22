//! Application settings page.

use super::licenses::{self, Licenses};
use super::{Cmd, Cx, Ev, Nav, Page};
use crate::i18n::{self, tr, Lang};
use crate::settings::{Settings, REC_SECONDS};
use crate::system::{actions, recorder};
use crate::ui::copyfmt::{Format, Scope};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons::{self, glyph};
use crate::ui::keys::{VK_BACK, VK_ESCAPE, VK_RETURN};
use crate::ui::theme::THEME;
use crate::ui::widgets::{item, Menu, MenuOut};
use std::sync::atomic::Ordering;

const SPEEDS: [(i18n::Text, u32); 4] = [
    (|s| s.settings.fast, 500),
    (|s| s.settings.normal, 1000),
    (|s| s.settings.slow, 5000),
    (|s| s.common.paused, 0),
];

/// How many days the recording may keep, at most (and at least one).
const MAX_DAYS: u32 = 3650;

/// The language picker's choices: following Windows, then each language.
fn language_choices() -> impl Iterator<Item = Option<Lang>> {
    std::iter::once(None).chain(Lang::ALL.into_iter().map(Some))
}

/// A choice of the language picker. Each language is named in itself, so it can be found
/// from any language.
fn choice_label(choice: Option<Lang>) -> &'static str {
    choice.map_or(tr().settings.lang_auto, Lang::native_name)
}

/// The controls of the page that are hit-tested the same way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Id {
    Language,
    Speed,
    Tray,
    TrayGraph,
    ElevateStart,
    Lifetime,
    RecEnabled,
    RecSeconds,
    RecDays,
    RecDetail,
    RecGpu,
    Format,
    Scope,
}

/// The choices of a drop-down.
fn choices(id: Id) -> Vec<String> {
    match id {
        Id::Language => language_choices().map(|l| choice_label(l).to_string()).collect(),
        Id::Speed => SPEEDS.iter().map(|(l, _)| l(tr()).to_string()).collect(),
        Id::RecSeconds => REC_SECONDS.iter().map(|v| (tr().units.seconds)(*v as u64)).collect(),
        Id::Format => Format::ALL.iter().map(|f| f.label().to_string()).collect(),
        Id::Scope => Scope::ALL.iter().map(|s| s.label().to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Which choice of a drop-down is in force.
fn current(id: Id, s: &Settings) -> usize {
    match id {
        Id::Language => language_choices().position(|l| l == s.language).unwrap_or(0),
        Id::Speed => SPEEDS.iter().position(|(_, ms)| *ms == s.interval_ms).unwrap_or(1),
        Id::RecSeconds => REC_SECONDS.iter().position(|v| *v == s.rec_seconds).unwrap_or(0),
        Id::Format => Format::ALL.iter().position(|f| *f == s.copy_format).unwrap_or(0),
        Id::Scope => Scope::ALL.iter().position(|v| *v == s.copy_scope).unwrap_or(0),
        _ => 0,
    }
}

/// The value of a switch.
fn is_on(id: Id, s: &Settings) -> bool {
    match id {
        Id::ElevateStart => s.elevate_on_start,
        Id::Tray => s.tray,
        Id::TrayGraph => s.tray_graph,
        Id::Lifetime => s.highlight_lifetime,
        Id::RecEnabled => s.rec_enabled,
        Id::RecDetail => s.rec_detail,
        Id::RecGpu => s.rec_gpu,
        _ => false,
    }
}

/// The number being typed into the days box.
struct Edit {
    text: String,
    /// Just clicked: the whole number is selected, and the next digit replaces it.
    fresh: bool,
}

pub struct SettingsPage {
    /// The controls drawn at the last paint.
    hits: Vec<(Id, Rect)>,
    hover: Option<Id>,
    /// The open drop-down list.
    menu: Option<(Id, Menu)>,
    /// The days box while it is being typed in.
    edit: Option<Edit>,
    /// The recording's folder: browse, open, reset.
    dir_rects: [Rect; 3],
    hover_dir: Option<usize>,
    elevate_rect: Rect,
    hover_elevate: bool,
    elevated: bool,
    /// How far the page is scrolled, and how far it can be.
    scroll: i32,
    max_scroll: i32,
    /// Pixels per wheel notch.
    line: i32,
    /// The link to the licenses under the version, and the dialog it opens.
    license_link: Rect,
    hover_license: bool,
    licenses: Option<Licenses>,
}

impl SettingsPage {
    pub fn new() -> Self {
        Self {
            hits: Vec::new(),
            hover: None,
            menu: None,
            edit: None,
            dir_rects: [Rect::default(); 3],
            hover_dir: None,
            elevate_rect: Rect::default(),
            hover_elevate: false,
            elevated: actions::is_elevated(),
            scroll: 0,
            max_scroll: 0,
            line: 40,
            license_link: Rect::default(),
            hover_license: false,
            licenses: None,
        }
    }

    /// Takes the typed number as the days to keep. Anything else leaves the setting as it was.
    fn commit_days(&mut self, cx: &mut Cx) {
        let Some(edit) = self.edit.take() else { return };
        if let Ok(v) = edit.text.parse::<u32>() {
            let v = v.clamp(1, MAX_DAYS);
            if v != cx.settings.rec_days {
                cx.settings.rec_days = v;
                cx.settings.save_recording();
            }
        }
        *cx.dirty = true;
    }

    /// Takes a pick of a drop-down.
    fn apply(&mut self, id: Id, i: usize, cx: &mut Cx) {
        match id {
            Id::Language => {
                let choice = language_choices().nth(i).flatten();
                if choice != cx.settings.language {
                    cx.settings.language = choice;
                    cx.cmds.push(Cmd::Language);
                }
            }
            Id::Speed => {
                cx.settings.interval_ms = SPEEDS[i].1;
                cx.shared.interval_ms.store(SPEEDS[i].1, Ordering::Relaxed);
                cx.shared.poke();
            }
            Id::RecSeconds => {
                cx.settings.rec_seconds = REC_SECONDS[i];
                cx.settings.save_recording();
            }
            Id::Format => cx.settings.copy_format = Format::ALL[i],
            Id::Scope => cx.settings.copy_scope = Scope::ALL[i],
            _ => {}
        }
        *cx.dirty = true;
    }

    fn hit(&self, x: i32, y: i32) -> Option<Id> {
        self.hits.iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| *id)
    }
}

/// Draws a card with a title and, under it, a line of explanation. Returns where the control goes:
/// `ctl_w` wide, on the right, in the middle of the card's height.
fn row_card(c: &mut Canvas, card: Rect, title: &str, note: Option<&str>, ctl_w: i32) -> Rect {
    let t = THEME;
    c.round(card, c.px(8) as f32, t.field, 255);
    c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
    let text_w = card.w - c.px(32) - ctl_w - c.px(16);
    match note {
        Some(note) => {
            c.text(title, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), text_w, c.px(24)), t.text, Align::Left);
            c.text(note, Font::Small, rect(card.x + c.px(16), card.y + c.px(38), text_w, c.px(22)), t.text_dim, Align::Left);
        }
        None => c.text(title, Font::Semi, rect(card.x + c.px(16), card.y, text_w, card.h), t.text, Align::Left),
    }
    rect(card.r() - c.px(16) - ctl_w, card.y + (card.h - c.px(32)) / 2, ctl_w, c.px(32))
}

/// A switch with its state in words on the left.
fn paint_toggle(c: &mut Canvas, r: Rect, on: bool, hover: bool) {
    let t = THEME;
    let sw = rect(r.r() - c.px(44), r.y + (r.h - c.px(22)) / 2, c.px(44), c.px(22));
    let knob = c.px(14);
    if on {
        c.round(sw, c.px(11) as f32, t.accent, if hover { 225 } else { 255 });
        c.round(rect(sw.r() - knob - c.px(4), sw.y + c.px(4), knob, knob), knob as f32 / 2.0, t.on_accent, 255);
    } else {
        if hover {
            c.round(sw, c.px(11) as f32, t.hover, 255);
        }
        c.stroke(sw, c.px(11) as f32, 1.0, t.text_dim, 255);
        c.round(rect(sw.x + c.px(4), sw.y + c.px(4), knob, knob), knob as f32 / 2.0, t.text_dim, 255);
    }
    let word = if on { tr().common.on } else { tr().common.off };
    c.text(word, Font::Body, rect(r.x, r.y, sw.x - r.x - c.px(10), r.h), t.text, Align::Right);
}

/// A drop-down box showing its current choice.
fn paint_dropdown(c: &mut Canvas, b: Rect, label: &str, hover: bool, open: bool) {
    let t = THEME;
    c.round(b, c.px(6) as f32, if hover || open { t.hover } else { t.window }, 255);
    c.stroke(b, c.px(6) as f32, 1.0, t.border, 255);
    c.text(label, Font::Body, rect(b.x + c.px(10), b.y, b.w - c.px(36), b.h), t.text, Align::Left);
    c.text(&glyph(icons::CHEVRON_DOWN), Font::IconSmall, rect(b.r() - c.px(28), b.y, c.px(20), b.h), t.text_dim, Align::Center);
}

/// A card with a switch.
#[allow(clippy::too_many_arguments)]
fn toggle_row(c: &mut Canvas, hits: &mut Vec<(Id, Rect)>, card: Rect, title: &str, note: Option<&str>, id: Id, s: &Settings, hover: Option<Id>) {
    let r = row_card(c, card, title, note, c.px(110));
    hits.push((id, r));
    paint_toggle(c, r, is_on(id, s), hover == Some(id));
}

/// A card with a drop-down.
#[allow(clippy::too_many_arguments)]
fn drop_row(c: &mut Canvas, hits: &mut Vec<(Id, Rect)>, card: Rect, title: &str, note: Option<&str>, id: Id, s: &Settings, page: &SettingsPage) {
    let b = row_card(c, card, title, note, c.px(220));
    hits.push((id, b));
    let label = choices(id).into_iter().nth(current(id, s)).unwrap_or_default();
    let open = page.menu.as_ref().is_some_and(|(open, _)| *open == id);
    paint_dropdown(c, b, &label, page.hover == Some(id), open);
}

impl Page for SettingsPage {
    fn nav(&self) -> Nav {
        Nav {
            icon: icons::SETTINGS,
            label: tr().common.settings,
            bottom: true,
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, settings: &Settings) {
        let t = THEME;
        let pad = c.px(24);
        self.line = c.px(40);
        c.set_clip(area);
        let top = area.y - self.scroll;
        c.text(tr().common.settings, Font::Display, rect(area.x + pad, top + c.px(10), area.w - pad * 2, c.px(44)), t.text, Align::Left);

        let card_w = (area.w - pad * 2).min(c.px(720));
        let mut y = top + c.px(68);
        // A heading over a group of cards. Returns where its first card goes.
        let section = |c: &mut Canvas, title: &str, y: i32| {
            c.text(title, Font::Title, rect(area.x + pad + c.px(2), y + c.px(4), card_w, c.px(30)), t.text, Align::Left);
            y + c.px(46)
        };
        // Between the cards of a section, and before the next section.
        let (gap, section_gap) = (c.px(12), c.px(24));
        let row_h = c.px(72);
        let mut hits: Vec<(Id, Rect)> = Vec::new();
        let hover = self.hover;
        let s = &tr().settings;

        y = section(c, s.sec_general, y);
        let card = rect(area.x + pad, y, card_w, row_h);
        drop_row(c, &mut hits, card, s.language, None, Id::Language, settings, self);
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        drop_row(c, &mut hits, card, s.update_speed, None, Id::Speed, settings, self);
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.tray, Some(s.tray_note), Id::Tray, settings, hover);
        // The graph in the icon is offered only while there is an icon.
        if settings.tray {
            y += card.h + gap;
            let card = rect(area.x + pad, y, card_w, row_h);
            toggle_row(c, &mut hits, card, s.tray_graph, None, Id::TrayGraph, settings, hover);
        }
        y += card.h + section_gap;

        // Administrator: how it runs now, then how it starts.
        y = section(c, s.sec_admin, y);
        let card = rect(area.x + pad, y, card_w, row_h);
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text(s.privileges, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w / 2, c.px(22)), t.text, Align::Left);
        let note = if self.elevated { s.running_admin } else { s.admin_note };
        // The button is as wide as its label, which differs by language; the note takes the rest.
        let button_w = if self.elevated { 0 } else { c.measure(s.restart_admin, Font::Body) + c.px(48) };
        let note_w = card.w - c.px(32) - if button_w > 0 { button_w + c.px(16) } else { 0 };
        c.text(note, Font::Small, rect(card.x + c.px(16), card.y + c.px(36), note_w, c.px(22)), t.text_dim, Align::Left);
        self.elevate_rect = Rect::default();
        if !self.elevated {
            let r = rect(card.r() - c.px(16) - button_w, card.y + c.px(18), button_w, c.px(36));
            self.elevate_rect = r;
            c.round(r, c.px(6) as f32, if self.hover_elevate { t.pressed } else { t.hover }, 255);
            c.stroke(r, c.px(6) as f32, 1.0, t.border, 255);
            c.text(&glyph(icons::SETTINGS), Font::IconSmall, rect(r.x + c.px(12), r.y, c.px(16), r.h), t.text, Align::Left);
            c.text(s.restart_admin, Font::Body, rect(r.x + c.px(30), r.y, r.w - c.px(36), r.h), t.text, Align::Left);
        }
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.elevate_on_start, None, Id::ElevateStart, settings, hover);
        y += card.h + section_gap;

        y = section(c, s.sec_display, y);
        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.lifetime, Some(s.lifetime_note), Id::Lifetime, settings, hover);
        y += card.h + section_gap;

        // The recording to the database. The cards below the first apply when it is on.
        y = section(c, s.sec_record, y);
        c.text(s.record_restart, Font::Small, rect(area.x + pad + c.px(2), y - c.px(8), card_w, c.px(22)), t.text_dim, Align::Left);
        y += c.px(20);
        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.record, Some(s.record_note), Id::RecEnabled, settings, hover);
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        drop_row(c, &mut hits, card, s.record_interval, None, Id::RecSeconds, settings, self);
        y += card.h + gap;

        // The days to keep: a number box and its unit ("days" follows the number: 1 day, 3 days).
        let card = rect(area.x + pad, y, card_w, row_h);
        let ctl = row_card(c, card, s.record_days, None, c.px(150));
        let field = rect(ctl.x, ctl.y, c.px(80), ctl.h);
        hits.push((Id::RecDays, field));
        let focused = self.edit.is_some();
        let border = if focused { t.accent } else if hover == Some(Id::RecDays) { t.text_faint } else { t.border };
        c.round(field, c.px(6) as f32, t.window, 255);
        c.stroke(field, c.px(6) as f32, 1.0, border, 255);
        let (shown, fresh) = match &self.edit {
            Some(e) => (e.text.clone(), e.fresh),
            None => (settings.rec_days.to_string(), false),
        };
        let text_area = rect(field.x + c.px(10), field.y, field.w - c.px(20), field.h);
        let tw = c.measure(&shown, Font::Body);
        if fresh && !shown.is_empty() {
            c.fill(rect(text_area.x - c.px(2), field.y + c.px(7), tw + c.px(4), field.h - c.px(14)), t.selection);
        }
        c.text(&shown, Font::Body, text_area, t.text, Align::Left);
        if focused && !fresh {
            c.fill(rect(text_area.x + tw + 1, field.y + c.px(8), 1, field.h - c.px(16)), t.text);
        }
        let days = (tr().units.days)(settings.rec_days as u64);
        let unit = days.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == ' ');
        c.text(unit, Font::Body, rect(field.r() + c.px(10), field.y, ctl.r() - field.r() - c.px(10), field.h), t.text, Align::Left);
        y += card.h + gap;

        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.record_detail, Some(s.record_detail_note), Id::RecDetail, settings, hover);
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        toggle_row(c, &mut hits, card, s.record_gpu, Some(s.record_gpu_note), Id::RecGpu, settings, hover);
        y += card.h + gap;

        // Where the files go: the label of the folder in use, then the buttons and the notes.
        let running_failed = recorder::FAILED.load(Ordering::Relaxed).then_some(settings.rec_running.as_ref()).flatten();
        let card = rect(area.x + pad, y, card_w, c.px(180) + if running_failed.is_some() { c.px(26) } else { 0 });
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text(s.record_dir, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w - c.px(32), c.px(24)), t.text, Align::Left);
        let shown = recorder::folder(&settings.rec_dir).map_or(String::new(), |p| p.display().to_string());
        let shown = if settings.rec_dir.is_empty() { format!("{shown}  ({})", s.record_dir_default) } else { shown };
        c.text(&shown, Font::Body, rect(card.x + c.px(16), card.y + c.px(40), card.w - c.px(32), c.px(26)), t.text, Align::Left);
        let labels = [s.record_browse, s.record_open, s.record_reset];
        let mut x = card.x + c.px(16);
        for (i, label) in labels.iter().enumerate() {
            let w = c.measure(label, Font::Body) + c.px(36);
            let r = rect(x, card.y + c.px(74), w, c.px(34));
            self.dir_rects[i] = r;
            c.round(r, c.px(6) as f32, if self.hover_dir == Some(i) { t.pressed } else { t.hover }, 255);
            c.stroke(r, c.px(6) as f32, 1.0, t.border, 255);
            c.text(label, Font::Body, r, t.text, Align::Center);
            x += w + c.px(8);
        }
        let notes = rect(card.x + c.px(16), card.y + c.px(118), card.w - c.px(32), c.px(22));
        c.text(s.record_dir_note, Font::Small, notes, t.text_dim, Align::Left);
        c.text(s.record_wal_note, Font::Small, rect(notes.x, notes.y + c.px(24), notes.w, notes.h), t.text_dim, Align::Left);
        if let Some(dir) = running_failed {
            let text = (s.record_failed)(&dir.display().to_string());
            c.text(&text, Font::Small, rect(notes.x, notes.y + c.px(50), notes.w, notes.h), t.warn, Align::Left);
        }
        y += card.h + section_gap;

        // Copy options: the choices come from the enums, so a new format or scope shows up here.
        y = section(c, s.sec_copy, y);
        let card = rect(area.x + pad, y, card_w, row_h);
        drop_row(c, &mut hits, card, s.copy_format, None, Id::Format, settings, self);
        y += card.h + gap;
        let card = rect(area.x + pad, y, card_w, row_h);
        drop_row(c, &mut hits, card, s.copy_scope, None, Id::Scope, settings, self);
        y += card.h + section_gap;
        let about = concat!("Light Task Manager ", env!("CARGO_PKG_VERSION"));
        c.text(about, Font::Small, rect(area.x + pad, y, card_w, c.px(24)), t.text_faint, Align::Left);
        let about_w = c.measure(about, Font::Small);
        let link_w = c.measure(s.licenses, Font::Small) + c.px(4);
        let link = rect(area.x + pad + about_w + c.px(20), y, link_w, c.px(24));
        self.license_link = link;
        c.text(s.licenses, Font::Small, link, t.accent, Align::Left);
        if self.hover_license {
            c.fill(rect(link.x, link.y + c.px(19), link_w - c.px(4), 1), t.accent);
        }
        self.hits = hits;
        c.reset_clip();
        if let Some((_, m)) = &mut self.menu {
            m.paint(c, area);
        }
        if let Some(l) = &mut self.licenses {
            l.paint(c, area);
        }
        let content = y + c.px(40) - top;
        self.max_scroll = (content - area.h).max(0);
        // The window may have grown since the last paint.
        self.scroll = self.scroll.min(self.max_scroll);
    }

    fn badge(&self) -> bool {
        recorder::FAILED.load(Ordering::Relaxed)
    }

    fn captures_text(&self) -> bool {
        self.edit.is_some()
    }

    fn on_hide(&mut self, cx: &mut Cx) {
        self.menu = None;
        self.licenses = None;
        self.commit_days(cx);
    }

    fn pannable(&self, _x: i32, _y: i32) -> bool {
        self.max_scroll > 0
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        // The licenses take the input until they close.
        if let Some(l) = &mut self.licenses {
            if let licenses::Out::Close = l.event(ev, cx) {
                self.licenses = None;
                *cx.dirty = true;
            }
            return;
        }
        // The open list takes the input until it closes.
        if let Some((id, m)) = &mut self.menu {
            let id = *id;
            match m.event(ev, cx.dirty) {
                MenuOut::Pick(i) => {
                    self.menu = None;
                    self.apply(id, i as usize, cx);
                }
                MenuOut::Close => {
                    self.menu = None;
                    *cx.dirty = true;
                }
                MenuOut::Stay if matches!(ev, Ev::Wheel { .. } | Ev::Pan { .. }) => {
                    self.menu = None;
                    *cx.dirty = true;
                }
                MenuOut::Stay => {}
            }
            if !matches!(ev, Ev::Wheel { .. } | Ev::Pan { .. }) {
                return;
            }
        }
        match *ev {
            Ev::Move { x, y } => {
                let hover = self.hit(x, y);
                let hover_dir = self.dir_rects.iter().position(|r| r.contains(x, y));
                let elevate = self.elevate_rect.contains(x, y);
                let license = self.license_link.contains(x, y);
                if hover != self.hover || hover_dir != self.hover_dir || elevate != self.hover_elevate || license != self.hover_license {
                    self.hover = hover;
                    self.hover_dir = hover_dir;
                    self.hover_elevate = elevate;
                    self.hover_license = license;
                    *cx.dirty = true;
                }
            }
            Ev::Leave => {
                self.hover = None;
                self.hover_dir = None;
                self.hover_elevate = false;
                self.hover_license = false;
                *cx.dirty = true;
            }
            Ev::Down { x, y, right: false } => {
                let hit = self.hit(x, y);
                // Clicking anywhere but the days box ends the typing.
                if hit != Some(Id::RecDays) {
                    self.commit_days(cx);
                }
                if let Some(id) = hit {
                    match id {
                        Id::Language | Id::Speed | Id::RecSeconds | Id::Format | Id::Scope => {
                            let items = choices(id).into_iter().enumerate().map(|(i, l)| item(l, i as u32, true)).collect();
                            let b = self.hits.iter().find(|(h, _)| *h == id).map(|(_, r)| *r).unwrap_or_default();
                            self.menu = Some((id, Menu::new(b.x, b.b() + 2, items)));
                        }
                        Id::RecDays => match &mut self.edit {
                            // A click in the box puts the caret at the end of the number.
                            Some(e) => e.fresh = false,
                            None => self.edit = Some(Edit { text: cx.settings.rec_days.to_string(), fresh: true }),
                        },
                        _ => {
                            // The switches.
                            let on = !is_on(id, cx.settings);
                            match id {
                                Id::ElevateStart => cx.settings.elevate_on_start = on,
                                Id::Tray => cx.settings.tray = on,
                                Id::TrayGraph => cx.settings.tray_graph = on,
                                Id::Lifetime => cx.settings.highlight_lifetime = on,
                                Id::RecEnabled => cx.settings.rec_enabled = on,
                                Id::RecDetail => cx.settings.rec_detail = on,
                                Id::RecGpu => cx.settings.rec_gpu = on,
                                _ => {}
                            }
                            if matches!(id, Id::RecEnabled | Id::RecDetail | Id::RecGpu) {
                                cx.settings.save_recording();
                            }
                            if matches!(id, Id::Tray | Id::TrayGraph) {
                                cx.cmds.push(Cmd::Tray);
                            }
                        }
                    }
                    *cx.dirty = true;
                } else if let Some(i) = self.dir_rects.iter().position(|r| r.contains(x, y)) {
                    match i {
                        0 => cx.cmds.push(Cmd::PickRecDir),
                        1 => cx.cmds.push(Cmd::OpenRecDir),
                        _ => {
                            // Back to the default folder. Nothing moves, so nothing to ask.
                            cx.settings.rec_dir.clear();
                            cx.settings.save_recording();
                        }
                    }
                    *cx.dirty = true;
                } else if self.elevate_rect.contains(x, y) {
                    cx.cmds.push(Cmd::Elevate);
                } else if self.license_link.contains(x, y) {
                    self.licenses = Some(Licenses::default());
                    self.hover_license = false;
                    *cx.dirty = true;
                }
            }
            Ev::Char { ch } => {
                let Some(e) = &mut self.edit else { return };
                if ch.is_ascii_digit() {
                    if e.fresh {
                        e.text.clear();
                        e.fresh = false;
                    }
                    if e.text.len() < 4 {
                        e.text.push(ch);
                    }
                    *cx.dirty = true;
                } else if ch as u32 == VK_BACK as u32 {
                    if e.fresh {
                        e.text.clear();
                        e.fresh = false;
                    } else {
                        e.text.pop();
                    }
                    *cx.dirty = true;
                }
            }
            Ev::Key { vk: VK_RETURN, .. } if self.edit.is_some() => self.commit_days(cx),
            Ev::Key { vk: VK_ESCAPE, .. } if self.edit.is_some() => {
                // Escape drops what was typed.
                self.edit = None;
                *cx.dirty = true;
            }
            Ev::Wheel { dy, ctrl: false, .. } if dy != 0.0 => {
                let next = (self.scroll - (dy * self.line as f32) as i32).clamp(0, self.max_scroll);
                if next != self.scroll {
                    self.scroll = next;
                    *cx.dirty = true;
                }
            }
            Ev::Pan { dy, .. } => {
                self.scroll = (self.scroll - dy as i32).clamp(0, self.max_scroll);
                *cx.dirty = true;
            }
            _ => {}
        }
    }
}
