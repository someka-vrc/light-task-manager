//! The open source licenses: this app's own and those of the code built into it, shown from
//! the settings page. The texts are built into the executable, so they go wherever it goes.

use super::{Cx, Ev};
use crate::i18n::tr;
use crate::ui::gfx::{rect, Align, Canvas, Font, Rect};
use crate::ui::keys::VK_ESCAPE;
use crate::ui::theme::THEME;
use crate::ui::widgets::panel;

const OWN: &str = include_str!("../../LICENSE");
const THIRD_PARTY: &str = include_str!("../../THIRD-PARTY-NOTICES.txt");

/// A line of the text: a heading (a line starting with `# `), or one to set as it comes.
/// Lines wrapped by hand are joined back, so the text wraps to the dialog's width.
fn paragraphs() -> Vec<(String, bool)> {
    let own = format!("# Light Task Manager\n\n{}\n\n", OWN.trim_end());
    let mut out: Vec<(String, bool)> = Vec::new();
    // Whether the last line ran long enough to have been wrapped by hand.
    let mut open = false;
    for line in own.lines().chain(THIRD_PARTY.lines()) {
        let line = line.trim_end();
        if line.is_empty() {
            out.push((String::new(), false));
            open = false;
        } else if let Some(head) = line.strip_prefix("# ") {
            out.push((head.to_string(), true));
            open = false;
        } else {
            match out.last_mut() {
                Some((last, false)) if open => {
                    last.push(' ');
                    last.push_str(line.trim_start());
                }
                _ => out.push((line.trim_start().to_string(), false)),
            }
            open = line.chars().count() >= 60;
        }
    }
    out
}

/// Breaks a line at spaces to fit `width`; a word too long for a line stands alone.
fn wrap(c: &mut Canvas, text: &str, font: Font, width: i32, out: &mut Vec<(String, bool)>, head: bool) {
    let mut line = String::new();
    for word in text.split(' ') {
        let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && c.measure(&next, font) > width {
            out.push((std::mem::take(&mut line), head));
            line = word.to_string();
        } else {
            line = next;
        }
    }
    out.push((line, head));
}

#[derive(Default)]
pub struct Licenses {
    scroll: i32,
    max_scroll: i32,
    /// The lines set for a width and scale, kept until either changes.
    lines: Vec<(String, bool)>,
    set_for: (i32, i32),
    panel: Rect,
    close: Rect,
    hover_close: bool,
    line: i32,
}

pub enum Out {
    Stay,
    Close,
}

impl Licenses {
    pub fn paint(&mut self, c: &mut Canvas, area: Rect) {
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(720).min(area.w - c.px(32));
        let h = c.px(640).min(area.h - c.px(32));
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
        self.panel = r;
        panel(c, r);
        let x = r.x + c.px(22);
        let iw = r.w - c.px(44);
        c.text(tr().settings.licenses, Font::Semi, rect(x, r.y + c.px(18), iw, c.px(26)), t.text, Align::Left);

        let bw = c.px(120).max(c.measure(tr().common.close, Font::Body) + c.px(32));
        let bh = c.px(36);
        let close = rect(r.r() - c.px(22) - bw, r.b() - bh - c.px(16), bw, bh);
        self.close = close;
        c.round(close, c.px(6) as f32, t.accent, if self.hover_close { 255 } else { 225 });
        c.text(tr().common.close, Font::Body, close, t.on_accent, Align::Center);

        // The text, in a box of its own that scrolls.
        let body = rect(r.x + c.px(16), r.y + c.px(56), r.w - c.px(32), close.y - c.px(12) - (r.y + c.px(56)));
        c.round(body, c.px(6) as f32, t.window, 255);
        c.stroke(body, c.px(6) as f32, 1.0, t.border, 255);
        let text_w = body.w - c.px(32);
        if self.set_for != (text_w, c.px(100)) {
            self.set_for = (text_w, c.px(100));
            self.lines.clear();
            for (p, head) in paragraphs() {
                let font = if head { Font::SmallSemi } else { Font::Small };
                wrap(c, &p, font, text_w, &mut self.lines, head);
            }
        }
        let lh = c.px(20);
        self.line = lh * 3;
        let inner = body.inset(c.px(12), c.px(8));
        self.max_scroll = (self.lines.len() as i32 * lh - inner.h).max(0);
        self.scroll = self.scroll.clamp(0, self.max_scroll);
        c.set_clip(inner);
        let first = (self.scroll / lh) as usize;
        for (i, (text, head)) in self.lines.iter().enumerate().skip(first) {
            let y = inner.y + i as i32 * lh - self.scroll;
            if y > inner.b() {
                break;
            }
            let (font, color) = if *head { (Font::SmallSemi, t.text) } else { (Font::Small, t.text_dim) };
            c.text(text, font, rect(inner.x, y, inner.w - c.px(8), lh), color, Align::Left);
        }
        c.reset_clip();
        // How far down the text is.
        if self.max_scroll > 0 {
            let track = body.inset(c.px(3), c.px(6));
            let total = self.max_scroll + inner.h;
            let thumb_h = (track.h * inner.h / total).max(c.px(24));
            let thumb_y = track.y + (track.h - thumb_h) * self.scroll / self.max_scroll;
            c.round(rect(track.r() - c.px(3), thumb_y, c.px(3), thumb_h), c.px(2) as f32, t.text_faint, 255);
        }
    }

    pub fn event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        match *ev {
            Ev::Move { x, y } => {
                let hover = self.close.contains(x, y);
                if hover != self.hover_close {
                    self.hover_close = hover;
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } if self.close.contains(x, y) || !self.panel.contains(x, y) => return Out::Close,
            Ev::Key { vk: VK_ESCAPE, .. } => return Out::Close,
            Ev::Wheel { dy, ctrl: false, .. } if dy != 0.0 => self.scroll_by(-(dy * self.line as f32) as i32, cx),
            Ev::Pan { dy, .. } => self.scroll_by(-dy as i32, cx),
            _ => {}
        }
        Out::Stay
    }

    fn scroll_by(&mut self, d: i32, cx: &mut Cx) {
        let next = (self.scroll + d).clamp(0, self.max_scroll);
        if next != self.scroll {
            self.scroll = next;
            *cx.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_texts_are_set_as_paragraphs_with_headings() {
        let p = paragraphs();
        let heads: Vec<&str> = p.iter().filter(|(_, h)| *h).map(|(s, _)| s.as_str()).collect();
        assert_eq!(heads[0], "Light Task Manager");
        assert!(heads.iter().any(|h| h.starts_with("windows-sys")));
        assert!(heads.iter().any(|h| h.contains("Rust Standard Library")));
        // A hand-wrapped paragraph is one line again, and the short lines stay apart.
        assert!(p.iter().any(|(s, _)| s.starts_with("Permission is hereby granted") && s.ends_with("following conditions:")));
        assert!(p.iter().any(|(s, _)| s == "https://github.com/microsoft/windows-rs"));
        assert!(p.iter().any(|(s, _)| s == "Copyright (c) Microsoft Corporation."));
    }
}
