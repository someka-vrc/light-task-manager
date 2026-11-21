//! The graph's settings dialog, and the question asked before turning the graph off.

use super::target::{enabled, toggle, Target, ORDER};
use super::*;

/// The settings dialog's hit areas.
#[derive(Default)]
pub struct Dialog {
    panel: Rect,
    checks: [Rect; ORDER.len()],
    /// The retention period's number box and its spinner buttons.
    field: Rect,
    up: Rect,
    down: Rect,
    close: Rect,
    /// 3 up, 4 down, 5 close, 6 the box, 10.. the checks.
    hover: Option<u8>,
    /// What is typed in the number box; applied on Enter, the spinner, or leaving the box.
    text: String,
    focused: bool,
    /// The whole number is selected, so the next digit replaces it.
    fresh: bool,
}

impl Dialog {
    pub fn new(minutes: u32) -> Self {
        Self { text: minutes.to_string(), focused: true, fresh: true, ..Default::default() }
    }

    /// The typed number within the allowed range, if any is typed.
    fn minutes(&self) -> Option<u32> {
        self.text.parse::<u32>().ok().map(|m| m.clamp(1, MAX_MINUTES))
    }
}

/// "Turn the graph off?", with "don't ask again".
#[derive(Default)]
pub struct OffConfirm {
    panel: Rect,
    check: Rect,
    buttons: [Rect; 2],
    /// 0 off, 1 cancel, 2 the check.
    hover: Option<u8>,
    dont_ask: bool,
}

impl Timeline {
    /// Applies the typed retention period, or restores the current one when nothing valid is typed.
    fn commit_minutes(&mut self, cx: &mut Cx) {
        let Some(d) = &mut self.dialog else { return };
        if let Some(m) = d.minutes() {
            cx.settings.tm_minutes = m;
        }
        d.text = cx.settings.tm_minutes.to_string();
        self.trim(cx.settings);
        *cx.dirty = true;
    }

    /// Steps the retention period by one minute.
    fn step_minutes(&mut self, delta: i32, cx: &mut Cx) {
        self.commit_minutes(cx);
        let m = (cx.settings.tm_minutes as i32 + delta).clamp(1, MAX_MINUTES as i32) as u32;
        cx.settings.tm_minutes = m;
        if let Some(d) = &mut self.dialog {
            d.text = m.to_string();
            d.focused = true;
            d.fresh = true;
        }
        self.trim(cx.settings);
        *cx.dirty = true;
    }

    pub(super) fn dialog_event(&mut self, ev: &Ev, cx: &mut Cx) {
        if let Ev::Move { x, y } = *ev {
            self.mouse = (x, y);
        }
        let Some(d) = &mut self.dialog else { return };
        match *ev {
            Ev::Move { x, y } => {
                let mut h = d.checks.iter().position(|r| r.contains(x, y)).map(|i| 10 + i as u8);
                if d.up.contains(x, y) {
                    h = Some(3);
                } else if d.down.contains(x, y) {
                    h = Some(4);
                } else if d.close.contains(x, y) {
                    h = Some(5);
                } else if d.field.contains(x, y) {
                    h = Some(6);
                }
                if h != d.hover {
                    d.hover = h;
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } => {
                if d.up.contains(x, y) {
                    return self.step_minutes(1, cx);
                }
                if d.down.contains(x, y) {
                    return self.step_minutes(-1, cx);
                }
                if d.field.contains(x, y) {
                    // A click in the box puts the caret at the end of the number.
                    if !d.focused {
                        d.focused = true;
                        d.fresh = true;
                    } else {
                        d.fresh = false;
                    }
                    *cx.dirty = true;
                    return;
                }
                let was_focused = d.focused;
                d.focused = false;
                let close = d.close.contains(x, y) || !d.panel.contains(x, y);
                let check = d.checks.iter().position(|r| r.contains(x, y));
                if was_focused {
                    self.commit_minutes(cx);
                }
                if let Some(i) = check {
                    toggle(cx.settings, ORDER[i]);
                } else if close {
                    self.commit_minutes(cx);
                    self.dialog = None;
                }
                *cx.dirty = true;
            }
            Ev::Wheel { dy, .. } if d.field.contains(self.mouse.0, self.mouse.1) && dy != 0.0 => {
                let delta = if dy > 0.0 { 1 } else { -1 };
                self.step_minutes(delta, cx);
            }
            Ev::Char { ch } if d.focused => {
                if ch.is_ascii_digit() {
                    if d.fresh {
                        d.text.clear();
                        d.fresh = false;
                    }
                    if d.text.len() < 4 {
                        d.text.push(ch);
                    }
                    *cx.dirty = true;
                } else if ch as u32 == VK_BACK as u32 {
                    if d.fresh {
                        d.text.clear();
                        d.fresh = false;
                    } else {
                        d.text.pop();
                    }
                    *cx.dirty = true;
                }
            }
            Ev::Key { vk: VK_UP, .. } if d.focused => self.step_minutes(1, cx),
            Ev::Key { vk: VK_DOWN, .. } if d.focused => self.step_minutes(-1, cx),
            Ev::Key { vk: VK_RETURN, .. } => {
                self.commit_minutes(cx);
                self.dialog = None;
                *cx.dirty = true;
            }
            Ev::Key { vk: VK_ESCAPE, .. } => {
                // Escape drops what was typed but not applied.
                self.dialog = None;
                *cx.dirty = true;
            }
            _ => {}
        }
    }

    pub(super) fn confirm_event(&mut self, ev: &Ev, cx: &mut Cx) -> Out {
        let Some(d) = &mut self.confirm else { return Out::Pass };
        let hit = |d: &OffConfirm, x: i32, y: i32| {
            if d.check.contains(x, y) {
                Some(2)
            } else {
                d.buttons.iter().position(|b| b.contains(x, y)).map(|i| i as u8)
            }
        };
        match *ev {
            Ev::Move { x, y } => {
                let h = hit(d, x, y);
                if h != d.hover {
                    d.hover = h;
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } => {
                *cx.dirty = true;
                match hit(d, x, y) {
                    Some(0) => return self.confirm_off(cx),
                    Some(1) => self.confirm = None,
                    Some(_) => d.dont_ask = !d.dont_ask,
                    None if !d.panel.contains(x, y) => self.confirm = None,
                    None => {}
                }
            }
            Ev::Key { vk: VK_RETURN, .. } => return self.confirm_off(cx),
            Ev::Key { vk: VK_ESCAPE, .. } => {
                self.confirm = None;
                *cx.dirty = true;
            }
            _ => {}
        }
        Out::Taken
    }

    fn confirm_off(&mut self, cx: &mut Cx) -> Out {
        if let Some(d) = self.confirm.take() {
            if d.dont_ask {
                cx.settings.tm_confirm_off = false;
            }
        }
        self.turn_off(cx)
    }

    pub(super) fn paint_confirm(&mut self, c: &mut Canvas, area: Rect) {
        let Some(d) = &mut self.confirm else { return };
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(440).min(area.w - c.px(32));
        let h = c.px(196);
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
        d.panel = r;
        panel(c, r);
        let x = r.x + c.px(22);
        let iw = r.w - c.px(44);
        c.text(tr().tm.off_q, Font::Semi, rect(x, r.y + c.px(18), iw, c.px(26)), t.text, Align::Left);
        c.text(tr().tm.off_discards, Font::Body, rect(x, r.y + c.px(54), iw, c.px(24)), t.text_dim, Align::Left);

        let label_w = c.measure(tr().tm.dont_ask_again, Font::Body);
        let check = rect(x - c.px(6), r.y + c.px(90), label_w + c.px(44), c.px(30));
        d.check = check;
        if d.hover == Some(2) {
            c.round(check, c.px(4) as f32, t.hover, 255);
        }
        let bx = rect(check.x + c.px(6), check.y + (check.h - c.px(18)) / 2, c.px(18), c.px(18));
        if d.dont_ask {
            c.round(bx, c.px(4) as f32, t.accent, 255);
            c.text(&glyph(icons::CHECK), Font::IconSmall, bx, t.on_accent, Align::Center);
        } else {
            c.stroke(bx, c.px(4) as f32, 1.0, t.text_dim, 255);
        }
        c.text(tr().tm.dont_ask_again, Font::Body, rect(bx.r() + c.px(10), check.y, label_w + c.px(4), check.h), t.text, Align::Left);

        let (bw, bh) = (c.px(120), c.px(36));
        let by = r.b() - bh - c.px(16);
        d.buttons[1] = rect(r.r() - c.px(22) - bw, by, bw, bh);
        d.buttons[0] = rect(d.buttons[1].x - bw - c.px(8), by, bw, bh);
        let radius = c.px(6) as f32;
        c.round(d.buttons[0], radius, t.accent, if d.hover == Some(0) { 255 } else { 225 });
        c.text(tr().tm.off_ok, Font::Body, d.buttons[0], t.on_accent, Align::Center);
        c.round(d.buttons[1], radius, if d.hover == Some(1) { t.pressed } else { t.hover }, 255);
        c.text(tr().common.cancel, Font::Body, d.buttons[1], t.text, Align::Center);
    }

    pub(super) fn paint_dialog(&mut self, c: &mut Canvas, area: Rect, s: &Settings) {
        let typed = self.dialog.as_ref().and_then(|d| d.minutes()).unwrap_or(s.tm_minutes);
        let estimate = self.estimate(s, typed);
        let used = self.used();
        let kept = self.len();
        let procs = self.last_procs;
        let Some(d) = &mut self.dialog else { return };
        let t = THEME;
        c.fill_alpha(area, 0, 110);
        let w = c.px(480).min(area.w - c.px(32));
        let rows = Target::ROWS;
        let h = (c.px(330) + c.px(30) * rows).min(area.h - c.px(16));
        let r = rect(area.x + (area.w - w) / 2, area.y + (area.h - h).max(0) / 2, w, h);
        d.panel = r;
        panel(c, r);
        let x = r.x + c.px(22);
        let iw = r.w - c.px(44);
        c.text(tr().tm.settings_title, Font::Semi, rect(x, r.y + c.px(18), iw, c.px(26)), t.text, Align::Left);

        c.text(tr().tm.targets, Font::Body, rect(x, r.y + c.px(52), iw, c.px(24)), t.text_dim, Align::Left);
        // One row per device: the main line, then its detail.
        let col_w = iw / 2;
        for (i, tg) in ORDER.iter().enumerate() {
            let on = enabled(s, *tg);
            let (col, row) = tg.cell();
            let cell = rect(x + col_w * col, r.y + c.px(80) + c.px(30) * row, col_w - c.px(4), c.px(28));
            d.checks[i] = cell;
            if d.hover == Some(10 + i as u8) {
                c.round(cell, c.px(4) as f32, t.hover, 255);
            }
            let bx = rect(cell.x + c.px(6), cell.y + (cell.h - c.px(18)) / 2, c.px(18), c.px(18));
            if on {
                c.round(bx, c.px(4) as f32, t.accent, 255);
                c.text(&glyph(icons::CHECK), Font::IconSmall, bx, t.on_accent, Align::Center);
            } else {
                c.stroke(bx, c.px(4) as f32, 1.0, t.text_dim, 255);
            }
            c.text(tg.label(), Font::Body, rect(bx.r() + c.px(10), cell.y, cell.r() - bx.r() - c.px(12), cell.h), t.text, Align::Left);
        }

        let py = r.y + c.px(96) + c.px(30) * rows;
        c.text(tr().tm.retention, Font::Body, rect(x, py, iw, c.px(24)), t.text_dim, Align::Left);

        // A number box with a spinner on its right edge.
        let field = rect(x, py + c.px(28), c.px(110), c.px(32));
        let spin_w = c.px(24);
        let spin = rect(field.r() - spin_w, field.y + 1, spin_w - 1, field.h - 2);
        let up = rect(spin.x, spin.y, spin.w, spin.h / 2);
        let down = rect(spin.x, up.b(), spin.w, spin.h - up.h);
        d.field = field;
        d.up = up;
        d.down = down;
        c.round(field, c.px(6) as f32, t.field, 255);
        let border = if d.focused { t.accent } else if d.hover == Some(6) { t.text_faint } else { t.border };
        c.stroke(field, c.px(6) as f32, 1.0, border, 255);
        let text_area = rect(field.x + c.px(10), field.y, spin.x - field.x - c.px(14), field.h);
        let tw = c.measure(&d.text, Font::Body);
        if d.focused && d.fresh && !d.text.is_empty() {
            c.fill(rect(text_area.x - c.px(2), field.y + c.px(7), tw + c.px(4), field.h - c.px(14)), t.selection);
        }
        c.text(&d.text, Font::Body, text_area, t.text, Align::Left);
        if d.focused && !d.fresh {
            c.fill(rect(text_area.x + tw + 1, field.y + c.px(8), 1, field.h - c.px(16)), t.text);
        }
        let cur = d.minutes().unwrap_or(s.tm_minutes);
        c.fill(rect(spin.x - 1, spin.y + c.px(4), 1, spin.h - c.px(8)), t.border);
        for (k, (b, glyph_ch, enabled)) in [(up, icons::CHEVRON_UP, cur < MAX_MINUTES), (down, icons::CHEVRON_DOWN, cur > 1)].iter().enumerate() {
            if *enabled && d.hover == Some(3 + k as u8) {
                c.fill(b.inset(c.px(2), 1), t.hover);
            }
            c.text(&glyph(*glyph_ch), Font::IconSmall, *b, if *enabled { t.text } else { t.text_faint }, Align::Center);
            c.tip(*b, if k == 0 { tr().tm.longer } else { tr().tm.shorter });
        }
        c.text(tr().tm.min_unit, Font::Body, rect(field.r() + c.px(8), field.y, c.px(24), field.h), t.text, Align::Left);
        let hint = (tr().tm.minutes_range)(MAX_MINUTES);
        c.text(&hint, Font::Small, rect(field.r() + c.px(36), field.y, iw - field.w - c.px(36), field.h), t.text_dim, Align::Left);

        let mb = |b: f64| b / 1024.0 / 1024.0;
        let est = (tr().tm.mem_estimate)(mb(estimate));
        c.text(&est, Font::Body, rect(x, py + c.px(70), iw, c.px(24)), t.text, Align::Left);
        let now = (tr().tm.mem_now)(mb(used as f64), kept);
        c.text(&now, Font::Small, rect(x, py + c.px(94), iw, c.px(20)), t.text_dim, Align::Left);
        let interval = if s.interval_ms == 0 { 1000 } else { s.interval_ms };
        let basis = (tr().tm.estimate_basis)(procs, interval as f32 / 1000.0);
        c.text(&basis, Font::Small, rect(x, py + c.px(114), iw, c.px(20)), t.text_dim, Align::Left);

        let (bw, bh) = (c.px(120), c.px(36));
        let close = rect(r.r() - c.px(22) - bw, r.b() - bh - c.px(16), bw, bh);
        d.close = close;
        c.round(close, c.px(6) as f32, t.accent, if d.hover == Some(5) { 255 } else { 225 });
        c.text(tr().common.close, Font::Body, close, t.on_accent, Align::Center);
    }
}
