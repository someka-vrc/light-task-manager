//! Input on the graph, the toolbar and the dialogs, and the actions it runs.

use super::*;

pub(super) trait Slop {
    fn px_slop(&self) -> i32;
}

impl Slop for Cx<'_> {
    fn px_slop(&self) -> i32 {
        if self.touch {
            CLICK_SLOP * 3
        } else {
            CLICK_SLOP
        }
    }
}

impl TimeMachinePage {
    /// Works out the file size for the dialog's current choices, when they changed.
    pub(super) fn refresh_estimate(&mut self, s: &Settings) {
        let Some(d) = &self.export else { return };
        let key = (d.scope, d.kind, d.format, d.encoding);
        if d.estimate.is_some_and(|(k, _)| k == key) {
            return;
        }
        let size = self.export_job(s, key.0, key.1, key.2, key.3).map_or(0, |j| j.estimate());
        if let Some(d) = &mut self.export {
            d.estimate = Some((key, size));
        }
    }

    /// Hands the file to the window, which asks where to save it.
    pub(super) fn start_export(&mut self, cx: &mut Cx) {
        let Some(d) = self.export.take() else { return };
        cx.settings.tm_export_kind = d.kind.key().into();
        cx.settings.tm_export_format = d.format.key().into();
        cx.settings.tm_export_encoding = d.encoding.key().into();
        if let Some(job) = self.export_job(cx.settings, d.scope, d.kind, d.format, d.encoding) {
            cx.cmds.push(Cmd::Save(crate::pages::SaveRequest {
                name: job.file_name(),
                kind: d.format.label(),
                ext: d.format.key(),
                write: Box::new(move |w| job.write(w)),
            }));
        }
        *cx.dirty = true;
    }

    pub(super) fn export_event(&mut self, ev: &Ev, cx: &mut Cx) {
        let Some(d) = &mut self.export else { return };
        // An open drop-down takes the input first.
        if let Some((which, m)) = &mut d.list {
            let which = *which;
            match m.event(ev, cx.dirty) {
                MenuOut::Stay => {}
                MenuOut::Close => {
                    d.list = None;
                    *cx.dirty = true;
                }
                MenuOut::Pick(id) => {
                    d.list = None;
                    if which == 3 {
                        d.format = Format::ALL[id as usize];
                    } else {
                        d.encoding = Encoding::ALL[id as usize];
                    }
                    self.refresh_estimate(cx.settings);
                    *cx.dirty = true;
                }
            }
            return;
        }
        match *ev {
            Ev::Move { x, y } => {
                let h = d.hit(x, y);
                if h != d.hover {
                    d.hover = h;
                    *cx.dirty = true;
                }
            }
            Ev::Down { x, y, right: false } => {
                match d.hit(x, y) {
                    Some(i @ 0..=2) => {
                        d.kind = Kind::ALL[i as usize];
                        self.refresh_estimate(cx.settings);
                    }
                    Some(i @ 7..=8) => {
                        d.scope = if i == 7 { Scope::Selection } else { Scope::All };
                        self.refresh_estimate(cx.settings);
                    }
                    Some(3) => {
                        let items = Format::ALL.iter().enumerate().map(|(i, f)| item(f.label(), i as u32, true)).collect();
                        d.list = Some((3, Menu::new(d.format_box.x, d.format_box.b() + 2, items)));
                    }
                    Some(4) => {
                        let items = Encoding::ALL.iter().enumerate().map(|(i, e)| item(e.label(), i as u32, true)).collect();
                        d.list = Some((4, Menu::new(d.encoding_box.x, d.encoding_box.b() + 2, items)));
                    }
                    Some(5) => self.export = None,
                    Some(6) => return self.start_export(cx),
                    _ if !d.panel.contains(x, y) => self.export = None,
                    _ => {}
                }
                *cx.dirty = true;
            }
            Ev::Key { vk: VK_ESCAPE, .. } => {
                self.export = None;
                *cx.dirty = true;
            }
            Ev::Key { vk: VK_RETURN, .. } => self.start_export(cx),
            _ => {}
        }
    }

    pub(super) fn update_hover(&mut self, cx: &mut Cx) {
        let (x, y) = self.mouse;
        let h = if self.switch_rect.contains(x, y) {
            Hover::Switch
        } else if self.settings_rect.contains(x, y) {
            Hover::Settings
        } else if self.export_rect.contains(x, y) {
            Hover::Export
        } else if self.resume.hit(x, y).is_some() {
            Hover::Resume
        } else {
            Hover::None
        };
        let changed = self.resume.hover_at(x, y);
        if h != self.hover || changed {
            self.hover = h;
            *cx.dirty = true;
        }
    }

    pub(super) fn toggle(&mut self, cx: &mut Cx) {
        cx.settings.tm_enabled = !cx.settings.tm_enabled;
        if !cx.settings.tm_enabled {
            // Turning it off discards what was recorded.
            self.live.clear();
            self.adapters.clear();
            self.disks.clear();
            self.resume_live(cx);
        }
        *cx.dirty = true;
    }

    /// Applies the typed retention period, or restores the current one when nothing valid is typed.
    pub(super) fn commit_minutes(&mut self, cx: &mut Cx) {
        let Some(d) = &mut self.dialog else { return };
        if let Some(m) = d.minutes() {
            cx.settings.tm_minutes = m;
        }
        d.text = cx.settings.tm_minutes.to_string();
        self.trim(cx.settings);
        *cx.dirty = true;
    }

    /// Steps the retention period by one minute.
    pub(super) fn step_minutes(&mut self, delta: i32, cx: &mut Cx) {
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
                    toggle(cx.settings, DIALOG_ORDER[i]);
                    self.update_table(cx.settings);
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

    /// Whether the selected row's process is still running, so the details page can show it.
    pub(super) fn selected_alive(&self) -> bool {
        let Some(id) = self.table.selected else { return false };
        self.live.last().is_some_and(|s| s.procs.iter().any(|p| row_id(p.pid, p.create_time) == id))
    }

    pub(super) fn run(&mut self, act: u32, cx: &mut Cx) {
        *cx.dirty = true;
        match act {
            ACT_COPY => {
                if let Some(t) = self.table.selected_text(cx.settings.copy_format) {
                    actions::set_clipboard(&t);
                }
            }
            ACT_SHOW => {
                if let Some(pid) = self.table.selected.and_then(|id| self.pids.get(&id).copied()) {
                    cx.cmds.push(Cmd::ShowProcess(pid));
                }
            }
            _ => {}
        }
    }

    pub(super) fn graph_event(&mut self, ev: &Ev, cx: &mut Cx) -> bool {
        let inner = self.plot.inset(1, 1);
        match *ev {
            Ev::Down { x, y, right: false } if inner.contains(x, y) && cx.settings.tm_enabled => {
                self.freeze(cx.settings);
                let t = self.time_at(x, cx.settings);
                if let Some(i) = Self::nearest(self.data(), t) {
                    self.drag = Some((i, x));
                    self.sel = Some((i, i));
                }
                *cx.dirty = true;
                true
            }
            Ev::Move { x, .. } if self.drag.is_some() => {
                let (anchor, x0) = self.drag.unwrap();
                if (x - x0).abs() >= cx.px_slop() {
                    let t = self.time_at(x, cx.settings);
                    if let Some(i) = Self::nearest(self.data(), t) {
                        self.sel = Some((anchor.min(i), anchor.max(i)));
                    }
                }
                *cx.dirty = true;
                true
            }
            Ev::Up { .. } if self.drag.is_some() => {
                self.drag = None;
                self.table.selected = None;
                self.update_table(cx.settings);
                *cx.dirty = true;
                true
            }
            Ev::Wheel { dx, dy, ctrl } if inner.contains(self.mouse.0, self.mouse.1) && cx.settings.tm_enabled => {
                let full = Self::full_span(cx.settings);
                if ctrl {
                    if dy == 0.0 {
                        return true;
                    }
                    // Wheel down zooms in, up zooms out, around the time under the mouse.
                    if dy < 0.0 {
                        self.freeze(cx.settings);
                    }
                    if self.frozen.is_none() {
                        return true;
                    }
                    let (start, end) = self.view(cx.settings);
                    let anchor = self.time_at(self.mouse.0, cx.settings);
                    let frac = (anchor - start) as f64 / (end - start).max(1) as f64;
                    let span = ((end - start) as f64 * ZOOM_STEP.powf(dy as f64)) as i64;
                    let span = span.clamp(Self::min_span(cx.settings), full);
                    self.span = span;
                    self.view_end = anchor + ((1.0 - frac) * span as f64) as i64;
                } else if self.frozen.is_some() {
                    let (start, end) = self.view(cx.settings);
                    if end - start < full {
                        let notches = dx + dy;
                        self.view_end = end - (notches as f64 * (end - start) as f64 / 10.0) as i64;
                        self.span = end - start;
                    }
                }
                *cx.dirty = true;
                true
            }
            _ => false,
        }
    }
}
