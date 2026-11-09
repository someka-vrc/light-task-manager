//! Application settings page.

use super::{glyph, icons, Cmd, Cx, Ev, Nav, Page};
use crate::actions;
use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::settings::Settings;
use crate::theme::THEME;
use std::sync::atomic::Ordering;

const SPEEDS: [(&str, u32); 4] = [
    ("高速 (0.5 秒)", 500),
    ("標準 (1 秒)", 1000),
    ("低速 (5 秒)", 5000),
    ("一時停止", 0),
];

pub struct SettingsPage {
    speed_rects: [Rect; 4],
    elevate_rect: Rect,
    hover_speed: Option<usize>,
    hover_elevate: bool,
    elevated: bool,
}

impl SettingsPage {
    pub fn new() -> Self {
        Self {
            speed_rects: [Rect::default(); 4],
            elevate_rect: Rect::default(),
            hover_speed: None,
            hover_elevate: false,
            elevated: actions::is_elevated(),
        }
    }
}

impl Page for SettingsPage {
    fn nav(&self) -> Nav {
        Nav {
            icon: icons::SETTINGS,
            label: "設定",
            bottom: true,
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, settings: &Settings) {
        let t = THEME;
        let pad = c.px(24);
        c.text("設定", Font::Title, rect(area.x + pad, area.y + c.px(12), area.w - pad * 2, c.px(36)), t.text, Align::Left);

        let card_w = (area.w - pad * 2).min(c.px(720));
        let mut y = area.y + c.px(64);

        let card = rect(area.x + pad, y, card_w, c.px(108));
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text("更新の速度", Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w - c.px(32), c.px(24)), t.text, Align::Left);

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
            c.text(label, Font::Body, r, color, Align::Center);
        }

        y += c.px(124);
        let card = rect(area.x + pad, y, card_w, c.px(72));
        c.round(card, c.px(8) as f32, t.field, 255);
        c.stroke(card, c.px(8) as f32, 1.0, t.border, 255);
        c.text("権限", Font::Semi, rect(card.x + c.px(16), card.y + c.px(12), card.w / 2, c.px(22)), t.text, Align::Left);
        let note = if self.elevated {
            "管理者として実行中です"
        } else {
            "システムプロセスの終了や、所有者などの取得に必要です"
        };
        c.text(note, Font::Small, rect(card.x + c.px(16), card.y + c.px(36), card.w - c.px(220), c.px(22)), t.text_dim, Align::Left);
        self.elevate_rect = Rect::default();
        if !self.elevated {
            let r = rect(card.r() - c.px(186), card.y + c.px(18), c.px(170), c.px(36));
            self.elevate_rect = r;
            c.round(r, c.px(6) as f32, if self.hover_elevate { t.pressed } else { t.hover }, 255);
            c.stroke(r, c.px(6) as f32, 1.0, t.border, 255);
            c.text(&glyph(icons::SETTINGS), Font::IconSmall, rect(r.x + c.px(12), r.y, c.px(16), r.h), t.text, Align::Left);
            c.text("管理者として再起動", Font::Body, rect(r.x + c.px(30), r.y, r.w - c.px(36), r.h), t.text, Align::Left);
        }

        y += c.px(88);
        let about = concat!("Light Task Manager ", env!("CARGO_PKG_VERSION"));
        c.text(about, Font::Small, rect(area.x + pad, y, card_w, c.px(24)), t.text_faint, Align::Left);
    }

    fn event(&mut self, ev: &Ev, cx: &mut Cx) {
        match *ev {
            Ev::Move { x, y } => {
                let hover = self.speed_rects.iter().position(|r| r.contains(x, y));
                let elevate = self.elevate_rect.contains(x, y);
                if hover != self.hover_speed || elevate != self.hover_elevate {
                    self.hover_speed = hover;
                    self.hover_elevate = elevate;
                    *cx.dirty = true;
                }
            }
            Ev::Leave => {
                self.hover_speed = None;
                self.hover_elevate = false;
                *cx.dirty = true;
            }
            Ev::Down { x, y, right: false } => {
                if let Some(i) = self.speed_rects.iter().position(|r| r.contains(x, y)) {
                    cx.settings.interval_ms = SPEEDS[i].1;
                    cx.shared.interval_ms.store(SPEEDS[i].1, Ordering::Relaxed);
                    cx.shared.poke();
                    *cx.dirty = true;
                } else if self.elevate_rect.contains(x, y) {
                    cx.cmds.push(Cmd::Elevate);
                }
            }
            _ => {}
        }
    }
}
