//! The process table under the graph: its columns and its rows for one sample or a range.

use super::*;

impl TimeMachinePage {
    pub(super) fn columns(mode: Mode, targets: &[Target]) -> (Vec<Column>, Vec<Group>, usize) {
        let mut cols = vec![col(|s| s.common.name, "Sample.name", 200, false), col(|_| "PID", "Sample.pid", 70, true)];
        let mut groups = Vec::new();
        let mut sort = 0;
        let hung = targets.contains(&Target::Hung);
        match mode {
            Mode::None => {}
            Mode::Single => {
                for t in targets.iter().filter(|t| t.per_process()) {
                    if sort == 0 {
                        sort = cols.len();
                    }
                    cols.push(col(t.group(), t.props()[0], t.widths().0, true));
                }
                if hung {
                    cols.push(Column { label: |s| s.common.not_responding, prop: "Sample.hung", width: 90, right: false, descending_first: true });
                }
            }
            Mode::Range => {
                cols.push(Column { label: |_| "+-", prop: "Sample.born_died", width: 44, right: false, descending_first: true });
                cols.push(Column { label: |s| s.tm.time, prop: "Sample.time", width: 150, right: false, descending_first: true });
                for t in targets.iter().filter(|t| t.per_process()) {
                    let first = cols.len();
                    let labels: [Text; 5] = [|s| s.tm.col_start, |s| s.tm.col_end, |s| s.tm.col_change, |s| s.tm.col_avg, |s| s.tm.col_max];
                    for (label, prop) in labels.into_iter().zip(&t.props()[1..]) {
                        cols.push(col(label, prop, t.widths().1, true));
                    }
                    groups.push(Group { label: t.group(), first, len: 5 });
                    if sort == 0 {
                        // The heaviest first: the range table is for finding who was busy.
                        sort = first + 4;
                    }
                }
                if hung {
                    cols.push(col(|s| s.tm.hung_secs, "Sample.hung_seconds", 110, true));
                }
            }
        }
        (cols, groups, sort)
    }

    pub(super) fn mode(&self) -> Mode {
        match self.sel {
            None => Mode::None,
            Some((a, b)) if a == b => Mode::Single,
            Some(_) => Mode::Range,
        }
    }

    /// Rebuilds the table for the selection.
    pub(super) fn update_table(&mut self, s: &Settings) {
        let mode = self.mode();
        let ts = targets(s);
        if self.built.as_ref() != Some(&(mode, ts.clone())) {
            let (cols, groups, sort) = Self::columns(mode, &ts);
            // Values sort heaviest first; with no values, names sort A to Z.
            self.table.set_columns(cols, groups, sort, sort != 0);
            self.built = Some((mode, ts.clone()));
        }
        self.pids.clear();
        // The rows borrow from the frozen ring while they are built.
        let frozen = self.frozen.take();
        let rows = match (self.sel, &frozen) {
            (Some((a, b)), Some(data)) if a == b => self.single_rows(&data[a], &ts),
            (Some((a, b)), Some(data)) => self.range_rows(&data[a..=b], &ts),
            _ => Vec::new(),
        };
        self.frozen = frozen;
        self.table.set_rows(rows);
    }

    pub(super) fn icon_of(&self, stat: &ProcStatic) -> Arc<ImageInfo> {
        stat.image.clone().unwrap_or_else(|| self.placeholder.clone())
    }

    pub(super) fn single_rows(&mut self, sample: &Sample, ts: &[Target]) -> Vec<Row> {
        let hung = ts.contains(&Target::Hung);
        let mut rows = Vec::with_capacity(sample.procs.len());
        for p in sample.procs.iter() {
            let id = row_id(p.pid, p.create_time);
            self.pids.insert(id, p.pid);
            let mut cells = vec![p.stat.name.clone(), p.pid.to_string()];
            let mut keys = vec![text(&p.stat.name), num(p.pid)];
            for t in ts.iter().filter(|t| t.per_process()) {
                let v = t.value(p);
                cells.push(t.text(v));
                keys.push(num(v));
            }
            if hung {
                let mark = if p.hung { tr().common.not_responding } else { "" };
                cells.push(mark.to_string());
                keys.push(text(mark));
            }
            let mut row = Row::new(id, cells, keys);
            row.icon = Some(self.icon_of(&p.stat));
            if p.hung && hung {
                row.colors = vec![(row.cells.len() - 1, DIED_COLOR)];
            }
            rows.push(row);
        }
        rows
    }

    pub(super) fn range_rows(&mut self, range: &[Arc<Sample>], ts: &[Target]) -> Vec<Row> {
        let values: Vec<Target> = ts.iter().copied().filter(|t| t.per_process()).collect();
        let show_hung = ts.contains(&Target::Hung);
        let mut rows = Vec::new();
        for a in range_stats(range, &values) {
            let (mark, time_text, time_key) = match (a.born, a.died) {
                (true, true) => ("+-", format!("{}〜{}", self.clock(a.born_at), self.clock(a.died_at)), a.born_at),
                (true, false) => ("+", self.clock(a.born_at), a.born_at),
                (false, true) => ("-", self.clock(a.died_at), a.died_at),
                (false, false) => ("", String::new(), 0),
            };
            let id = row_id(a.pid, a.create_time);
            self.pids.insert(id, a.pid);
            let mut cells = vec![a.stat.name.clone(), a.pid.to_string(), mark.to_string(), time_text];
            let mut keys = vec![text(&a.stat.name), num(a.pid), text(mark), num(time_key as f64)];
            for (j, t) in values.iter().enumerate() {
                let start = t.shown(a.start[j]);
                let end = t.shown(a.end[j]);
                let avg = t.shown(a.avg(j));
                let max = t.shown(a.max[j]);
                let delta = end - start;
                for (cell, key) in [
                    (t.text(start), start),
                    (t.text(end), end),
                    (t.delta(delta), delta),
                    (t.text(avg), avg),
                    (t.text(max), max),
                ] {
                    cells.push(cell);
                    keys.push(num(key));
                }
            }
            let mut colors = Vec::new();
            match (a.born, a.died) {
                (_, true) => colors.push((2, DIED_COLOR)),
                (true, false) => colors.push((2, BORN_COLOR)),
                _ => {}
            }
            if show_hung {
                let secs = a.hung as f64 / SEC as f64;
                cells.push(if a.hung > 0 { format!("{secs:.0}") } else { String::new() });
                keys.push(num(secs));
                if a.hung > 0 {
                    colors.push((cells.len() - 1, DIED_COLOR));
                }
            }
            let mut row = Row::new(id, cells, keys);
            row.icon = Some(self.icon_of(&a.stat));
            row.colors = colors;
            rows.push(row);
        }
        rows
    }
}
