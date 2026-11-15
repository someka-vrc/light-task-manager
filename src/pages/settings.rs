//! Application settings page.

use super::{Cmd, Cx, Ev, Nav, Page};
use crate::i18n::{self, tr, Lang};
use crate::settings::Settings;
use crate::system::actions;
use crate::ui::copyfmt::{Format, Scope};
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::icons::{self, glyph};
use crate::ui::theme::THEME;
use crate::ui::widgets::{item, Menu, MenuOut};
use std::sync::atomic::Ordering;

const SPEEDS: [(i18n::Text, u32); 4] = [
    (|s| s.settings.fast, 500),
    (|s| s.settings.normal, 1000),
    (|s| s.settings.slow, 5000),
    (|s| s.common.paused, 0),
];

/// The language picker's choices: following Windows, then each language.
fn language_choices() -> impl Iterator<Item = Option<Lang>> {
    std::iter::once(None).chain(Lang::ALL.into_iter().map(Some))
}

/// A choice of the language picker. Each language is named in itself, so it can be found
/// from any language.
fn choice_label(choice: Option<Lang>) -> &'static str {
    choice.map_or(tr().settings.lang_auto, Lang::native_name)
}

pub struct SettingsPage {
    /// The language drop-down, and its list while open.
    language_box: Rect,
    hover_language: bool,
    language_menu: Option<Menu>,
    speed_rects: [Rect; 4],
    format_rects: Vec<Rect>,
    scope_rects: Vec<Rect>,
    elevate_start_rects: Vec<Rect>,
    lifetime_rects: Vec<Rect>,
    elevate_rect: Rect,
    hover_speed: Option<usize>,
    hover_format: Option<usize>,
    hover_scope: Option<usize>,
    hover_elevate_start: Option<usize>,
    hover_lifetime: Option<usize>,
    hover_elevate: bool,
    elevated: bool,
    /// How far the page is scrolled, and how far it can be.
    scroll: i32,
    max_scroll: i32,
    /// Pixels per wheel notch.
    line: i32,
}

impl SettingsPage {
    pub fn new() -> Self {
        Self {
            language_box: Rect::default(),
            hover_language: false,
            language_menu: None,
            speed_rects: [Rect::default(); 4],
            format_rects: Vec::new(),
            scope_rects: Vec::new(),
            elevate_start_rects: Vec::new(),
            lifetime_rects: Vec::new(),
            elevate_rect: Rect::default(),
            hover_speed: None,
            hover_format: None,
            hover_scope: None,
            hover_elevate_start: None,
            hover_lifetime: None,
            hover_elevate: false,
            elevated: actions::is_elevated(),
            scroll: 0,
            max_scroll: 0,
            line: 40,
        }
    }
}

/// Draws a card with a title and a segmented control of `labels`. Returns the segments' rectangles.
fn segment_card(c: &mut Canvas, card: Rect, title: &str, labels: &[&str], selected: usize, hover: Option<usize>) -> Vec<Rect> {
    segment_card_with_note(c, card, title, None, labels, selected, hover)
}

/// Like `segment_card`, with a line of explanation under the title (the card is 24 px taller).
fn segment_card_with_note(
    c: &mut Canvas,
    card: Rect,
    title: &str,
    note: Option<&str>,
    labels: &[&str],
    selected: usize,
    hover: Option<usize>,
) -> Vec<Rect> {
    let t = THEME;
    c.round(card, c.px(8) as f32, t.field, 255);
    c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
    c.text(title, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w - c.px(32), c.px(24)), t.text, Align::Left);
    let mut seg_y = card.y + c.px(50);
    if let Some(note) = note {
        c.text(note, Font::Small, rect(card.x + c.px(16), card.y + c.px(38), card.w - c.px(32), c.px(22)), t.text_dim, Align::Left);
        seg_y += c.px(24);
    }
    let seg = rect(card.x + c.px(16), seg_y, card.w - c.px(32), c.px(40));
    c.round(seg, c.px(6) as f32, t.window, 255);
    let each = seg.w / labels.len().max(1) as i32;
    let mut rects = Vec::with_capacity(labels.len());
    for (i, label) in labels.iter().enumerate() {
        let r = rect(seg.x + i as i32 * each, seg.y, each, seg.h);
        rects.push(r);
        let is_selected = i == selected;
        if is_selected {
            c.round(r.inset(c.px(3), c.px(3)), c.px(4) as f32, t.accent, 255);
        } else if hover == Some(i) {
            c.round(r.inset(c.px(3), c.px(3)), c.px(4) as f32, t.hover, 255);
        }
        let color = if is_selected { t.on_accent } else { t.text };
        c.text(label, Font::Body, r, color, Align::Center);
    }
    rects
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
        c.text(tr().common.settings, Font::Title, rect(area.x + pad, top + c.px(12), area.w - pad * 2, c.px(36)), t.text, Align::Left);

        let card_w = (area.w - pad * 2).min(c.px(720));
        let mut y = top + c.px(56);
        // A heading over a group of cards. Returns where its first card goes.
        let section = |c: &mut Canvas, title: &str, y: i32| {
            c.text(title, Font::Semi, rect(area.x + pad + c.px(2), y + c.px(8), card_w, c.px(24)), t.text_dim, Align::Left);
            y + c.px(40)
        };
        // Between the cards of a section, and before the next section.
        let (gap, section_gap) = (c.px(16), c.px(16));

        y = section(c, tr().settings.sec_general, y);
        let card = rect(area.x + pad, y, card_w, c.px(72));
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text(tr().settings.language, Font::Semi, rect(card.x + c.px(16), card.y, card.w - c.px(260), card.h), t.text, Align::Left);
        let b = rect(card.r() - c.px(236), card.y + c.px(20), c.px(220), c.px(32));
        self.language_box = b;
        let open = self.language_menu.is_some();
        c.round(b, c.px(6) as f32, if self.hover_language || open { t.hover } else { t.window }, 255);
        c.stroke(b, c.px(6) as f32, 1.0, t.border, 255);
        c.text(choice_label(settings.language), Font::Body, rect(b.x + c.px(10), b.y, b.w - c.px(36), b.h), t.text, Align::Left);
        c.text(&glyph(icons::CHEVRON_DOWN), Font::IconSmall, rect(b.r() - c.px(28), b.y, c.px(20), b.h), t.text_dim, Align::Center);
        y += card.h + gap;

        let card = rect(area.x + pad, y, card_w, c.px(108));
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text(tr().settings.update_speed, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w - c.px(32), c.px(24)), t.text, Align::Left);

        let seg = rect(card.x + c.px(16), card.y + c.px(50), card.w - c.px(32), c.px(40));
        c.round(seg, c.px(6) as f32, t.window, 255);
        let each = seg.w / SPEEDS.len() as i32;
        for (i, (label, ms)) in SPEEDS.iter().enumerate() {
            let r = rect(seg.x + i as i32 * each, seg.y, each, seg.h);
            self.speed_rects[i] = r;
            let selected = settings.interval_ms == *ms;
            if selected {
                c.round(r.inset(c.px(3), c.px(3)), c.px(4) as f32, t.accent, 255);
            } else if self.hover_speed == Some(i) {
                c.round(r.inset(c.px(3), c.px(3)), c.px(4) as f32, t.hover, 255);
            }
            let color = if selected { t.on_accent } else { t.text };
            c.text(label(tr()), Font::Body, r, color, Align::Center);
        }
        y += card.h + section_gap;

        // Administrator: how it runs now, then how it starts.
        y = section(c, tr().settings.sec_admin, y);
        let card = rect(area.x + pad, y, card_w, c.px(72));
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text(tr().settings.privileges, Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w / 2, c.px(22)), t.text, Align::Left);
        let note = if self.elevated {
            tr().settings.running_admin
        } else {
            tr().settings.admin_note
        };
        // The button is as wide as its label, which differs by language; the note takes the rest.
        let button_w = if self.elevated { 0 } else { c.measure(tr().settings.restart_admin, Font::Body) + c.px(48) };
        let note_w = card.w - c.px(32) - if button_w > 0 { button_w + c.px(16) } else { 0 };
        c.text(note, Font::Small, rect(card.x + c.px(16), card.y + c.px(36), note_w, c.px(22)), t.text_dim, Align::Left);
        self.elevate_rect = Rect::default();
        if !self.elevated {
            let r = rect(card.r() - c.px(16) - button_w, card.y + c.px(18), button_w, c.px(36));
            self.elevate_rect = r;
            c.round(r, c.px(6) as f32, if self.hover_elevate { t.pressed } else { t.hover }, 255);
            c.stroke(r, c.px(6) as f32, 1.0, t.border, 255);
            c.text(&glyph(icons::SETTINGS), Font::IconSmall, rect(r.x + c.px(12), r.y, c.px(16), r.h), t.text, Align::Left);
            c.text(tr().settings.restart_admin, Font::Body, rect(r.x + c.px(30), r.y, r.w - c.px(36), r.h), t.text, Align::Left);
        }
        y += card.h + gap;

        let on_off = [tr().common.off, tr().common.on];
        let card = rect(area.x + pad, y, card_w, c.px(108));
        self.elevate_start_rects = segment_card(
            c,
            card,
            tr().settings.elevate_on_start,
            &on_off,
            settings.elevate_on_start as usize,
            self.hover_elevate_start,
        );
        y += card.h + section_gap;

        y = section(c, tr().settings.sec_display, y);
        let card = rect(area.x + pad, y, card_w, c.px(132));
        self.lifetime_rects = segment_card_with_note(
            c,
            card,
            tr().settings.lifetime,
            Some(tr().settings.lifetime_note),
            &on_off,
            settings.highlight_lifetime as usize,
            self.hover_lifetime,
        );
        y += card.h + section_gap;

        // Copy options: the choices come from the enums, so a new format or scope shows up here.
        y = section(c, tr().settings.sec_copy, y);
        let labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
        let selected = Format::ALL.iter().position(|f| *f == settings.copy_format).unwrap_or(0);
        let card = rect(area.x + pad, y, card_w, c.px(108));
        self.format_rects = segment_card(c, card, tr().settings.copy_format, &labels, selected, self.hover_format);
        y += card.h + gap;

        let labels: Vec<&str> = Scope::ALL.iter().map(|s| s.label()).collect();
        let selected = Scope::ALL.iter().position(|s| *s == settings.copy_scope).unwrap_or(0);
        let card = rect(area.x + pad, y, card_w, c.px(108));
        self.scope_rects = segment_card(c, card, tr().settings.copy_scope, &labels, selected, self.hover_scope);
        y += card.h + section_gap;
        let about = concat!("Light Task Manager ", env!("CARGO_PKG_VERSION"));
        c.text(about, Font::Small, rect(area.x + pad, y, card_w, c.px(24)), t.text_faint, Align::Left);
        c.reset_clip();
        if let Some(m) = &mut self.language_menu {
            m.paint(c, area);
        }
        let content = y + c.px(40) - top;
        self.max_scroll = (content - area.h).max(0);
        // The window may have grown since the last paint.
        self.scroll = self.scroll.min(self.max_scroll);
    }

    fn on_hide(&mut self, _cx: &mut Cx) {
        self.language_menu = None;
    }

    fn pannable(&self, _x: i32, _y: i32) -> bool {
        self.max_scroll > 0
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        let index_of = |rects: &[Rect], x: i32, y: i32| rects.iter().position(|r| r.contains(x, y));
        // The open list takes the input until it closes.
        if let Some(m) = &mut self.language_menu {
            match m.event(ev, cx.dirty) {
                MenuOut::Pick(i) => {
                    self.language_menu = None;
                    let choice = language_choices().nth(i as usize).flatten();
                    if choice != cx.settings.language {
                        cx.settings.language = choice;
                        cx.cmds.push(Cmd::Language);
                    }
                    *cx.dirty = true;
                }
                MenuOut::Close => {
                    self.language_menu = None;
                    *cx.dirty = true;
                }
                MenuOut::Stay if matches!(ev, Ev::Wheel { .. } | Ev::Pan { .. }) => {
                    self.language_menu = None;
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
                let hover_language = self.language_box.contains(x, y);
                let hover = index_of(&self.speed_rects, x, y);
                let hover_format = index_of(&self.format_rects, x, y);
                let hover_scope = index_of(&self.scope_rects, x, y);
                let hover_elevate_start = index_of(&self.elevate_start_rects, x, y);
                let hover_lifetime = index_of(&self.lifetime_rects, x, y);
                let elevate = self.elevate_rect.contains(x, y);
                if hover_language != self.hover_language
                    || hover != self.hover_speed
                    || hover_format != self.hover_format
                    || hover_scope != self.hover_scope
                    || hover_elevate_start != self.hover_elevate_start
                    || hover_lifetime != self.hover_lifetime
                    || elevate != self.hover_elevate
                {
                    self.hover_language = hover_language;
                    self.hover_speed = hover;
                    self.hover_format = hover_format;
                    self.hover_scope = hover_scope;
                    self.hover_elevate_start = hover_elevate_start;
                    self.hover_lifetime = hover_lifetime;
                    self.hover_elevate = elevate;
                    *cx.dirty = true;
                }
            }
            Ev::Leave => {
                self.hover_language = false;
                self.hover_speed = None;
                self.hover_format = None;
                self.hover_scope = None;
                self.hover_elevate_start = None;
                self.hover_lifetime = None;
                self.hover_elevate = false;
                *cx.dirty = true;
            }
            Ev::Down { x, y, right: false } => {
                if self.language_box.contains(x, y) {
                    let items = language_choices().enumerate().map(|(i, l)| item(choice_label(l), i as u32, true)).collect();
                    let b = self.language_box;
                    self.language_menu = Some(Menu::new(b.x, b.b() + 2, items));
                    *cx.dirty = true;
                } else if let Some(i) = index_of(&self.speed_rects, x, y) {
                    cx.settings.interval_ms = SPEEDS[i].1;
                    cx.shared.interval_ms.store(SPEEDS[i].1, Ordering::Relaxed);
                    cx.shared.poke();
                    *cx.dirty = true;
                } else if let Some(i) = index_of(&self.format_rects, x, y) {
                    cx.settings.copy_format = Format::ALL[i];
                    *cx.dirty = true;
                } else if let Some(i) = index_of(&self.scope_rects, x, y) {
                    cx.settings.copy_scope = Scope::ALL[i];
                    *cx.dirty = true;
                } else if let Some(i) = index_of(&self.elevate_start_rects, x, y) {
                    cx.settings.elevate_on_start = i == 1;
                    *cx.dirty = true;
                } else if let Some(i) = index_of(&self.lifetime_rects, x, y) {
                    cx.settings.highlight_lifetime = i == 1;
                    *cx.dirty = true;
                } else if self.elevate_rect.contains(x, y) {
                    cx.cmds.push(Cmd::Elevate);
                }
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
