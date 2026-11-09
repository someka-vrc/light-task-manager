//! Reserved slot for the upcoming performance page.

use super::{glyph, icons, Nav, Page};
use crate::gfx::{rect, Align, Canvas, Font, Rect};
use crate::settings::Settings;
use crate::theme::THEME;

pub struct PerformancePage;

impl Page for PerformancePage {
    fn nav(&self) -> Nav {
        Nav {
            icon: icons::CHART,
            label: "パフォーマンス",
            bottom: false,
        }
    }

    fn paint(&mut self, c: &mut Canvas, area: Rect, _settings: &Settings) {
        let t = THEME;
        let mid = area.y + area.h / 2;
        c.text(
            &glyph(icons::CHART),
            Font::IconLarge,
            rect(area.x, mid - c.px(70), area.w, c.px(60)),
            t.text_faint,
            Align::Center,
        );
        c.text(
            "パフォーマンス",
            Font::Title,
            rect(area.x, mid, area.w, c.px(32)),
            t.text,
            Align::Center,
        );
        c.text(
            "このページは今後のバージョンで追加します",
            Font::Body,
            rect(area.x, mid + c.px(34), area.w, c.px(24)),
            t.text_dim,
            Align::Center,
        );
    }
}
