//! Building the shown rows: grouping, the tree, sorting, filtering, and the tints of
//! processes that just started or ended.

use super::*;

pub(super) fn haystack(p: &Proc) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        p.stat.name,
        p.pid,
        p.stat.user,
        p.stat.description()
    )
    .to_lowercase()
}

impl DetailsPage {
    /// Rebuilds the groups or the tree from the current snapshot.
    pub(super) fn regroup(&mut self, settings: &Settings) {
        self.mode = settings.view;
        (self.groups, self.in_group) = if self.mode == ViewMode::Group {
            grouping::build(&self.snap.procs)
        } else {
            (Vec::new(), Vec::new())
        };
        if let Some(data) = &mut self.range {
            data.group(&self.groups);
        }
        self.build_tree();
        // A selected process must stay visible when it joins a collapsed group or branch.
        if let Some(i) = self.selected_index_in_snap() {
            self.open_to(i);
        }
        match self.mode {
            ViewMode::Group => {
                let groups = &self.groups;
                self.expanded.retain(|id| groups.iter().any(|g| g.id == *id));
            }
            ViewMode::Tree => {
                let alive: HashSet<(u32, i64)> = self.snap.procs.iter().map(Proc::key).collect();
                self.open_nodes.retain(|k| alive.contains(k));
            }
            ViewMode::Flat => {}
        }
        self.resort(settings);
        if self.selected.is_some() && self.selected_proc().is_none() && !self.ghost_selected() {
            self.selected = None;
        }
    }

    /// Links each process to its parent. A parent younger than its child means the PID
    /// was reused, so that process is not the parent.
    pub(super) fn build_tree(&mut self) {
        self.parent.clear();
        self.children.clear();
        self.roots.clear();
        if self.mode != ViewMode::Tree {
            return;
        }
        let procs = &self.snap.procs;
        let by_pid: HashMap<u32, u32> = procs.iter().enumerate().map(|(i, p)| (p.pid, i as u32)).collect();
        self.parent = procs
            .iter()
            .enumerate()
            .map(|(i, p)| match by_pid.get(&p.ppid) {
                Some(&j) if j as usize != i && procs[j as usize].create_time <= p.create_time => j,
                _ => NO_PARENT,
            })
            .collect();
        // Equal creation times could still make a loop; cut it.
        for i in 0..self.parent.len() {
            let mut at = self.parent[i];
            let mut steps = 0;
            while at != NO_PARENT && at as usize != i && steps < 256 {
                at = self.parent[at as usize];
                steps += 1;
            }
            if at != NO_PARENT {
                self.parent[i] = NO_PARENT;
            }
        }
        self.children = vec![Vec::new(); procs.len()];
        for (i, p) in self.parent.iter().enumerate() {
            if *p == NO_PARENT {
                self.roots.push(i as u32);
            } else {
                self.children[*p as usize].push(i as u32);
            }
        }
    }

    pub(super) fn resort(&mut self, settings: &Settings) {
        let procs = &self.snap.procs;
        let range = self.range.as_ref();
        // The process table sorts on a column of its processes; the range table on a
        // value of its rows, or on the name or PID of their processes.
        let (by, desc) = match range {
            Some(_) => match settings.range_sort.item {
                RItem::Name => (AnyCol::P(Col::Name), settings.range_desc),
                RItem::Pid => (AnyCol::P(Col::Pid), settings.range_desc),
                _ => (AnyCol::R(settings.range_sort), settings.range_desc),
            },
            None => (AnyCol::P(settings.sort), settings.descending),
        };
        // Each side: the row's process (or a group's totals) and which row it is.
        let compare = |pa: &Proc, a: Which, pb: &Proc, b: Which| {
            let order = match (by, range) {
                (AnyCol::R(col), Some(data)) => data.key(a, col).total_cmp(&data.key(b, col)),
                (AnyCol::P(col), _) => columns::compare(col, pa, pb),
                _ => std::cmp::Ordering::Equal,
            };
            let order = if desc { order.reverse() } else { order };
            order.then(pa.pid.cmp(&pb.pid))
        };
        let by_proc = |a: &u32, b: &u32| compare(&procs[*a as usize], Which::Proc(*a), &procs[*b as usize], Which::Proc(*b));
        self.sorted.clear();
        self.sorted.extend(0..procs.len() as u32);
        self.sorted.sort_unstable_by(by_proc);
        for g in &mut self.groups {
            g.members.sort_unstable_by(by_proc);
        }
        // The tree sorts only among siblings.
        self.roots.sort_unstable_by(by_proc);
        for kids in &mut self.children {
            kids.sort_unstable_by(by_proc);
        }

        let groups = &self.groups;
        let mut order: Vec<u32> = (0..groups.len() as u32).collect();
        let group = |g: u32| (&groups[g as usize].summary, Which::Group(g));
        order.sort_unstable_by(|a, b| {
            let ((pa, wa), (pb, wb)) = (group(*a), group(*b));
            compare(pa, wa, pb, wb)
        });
        let in_group = &self.in_group;
        let mut singles = self
            .sorted
            .iter()
            .copied()
            .filter(|i| !in_group.get(*i as usize).copied().unwrap_or(false))
            .peekable();
        let mut order = order.into_iter().peekable();
        self.top.clear();
        loop {
            let group_first = match (singles.peek(), order.peek()) {
                (Some(s), Some(g)) => {
                    let (pg, wg) = group(*g);
                    compare(pg, wg, &procs[*s as usize], Which::Proc(*s)).is_lt()
                }
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (None, None) => break,
            };
            let row = if group_first {
                order.next().map(Row::Group)
            } else {
                singles.next().map(Row::Proc)
            };
            self.top.extend(row);
        }
        self.refilter();
    }

    pub(super) fn refilter(&mut self) {
        self.view.clear();
        self.meta.clear();
        let filtering = !self.filter.is_empty();
        if filtering && self.haystacks.len() != self.snap.procs.len() {
            self.haystacks = self.snap.procs.iter().map(haystack).collect();
        }
        let (hay, terms) = (&self.haystacks, &self.filter);
        let hit = |i: u32| !filtering || terms.iter().all(|t| hay[i as usize].contains(t.as_str()));
        let flat = Meta::default();
        if self.mode == ViewMode::Tree {
            // While filtering: 2 for a match, 1 for an ancestor of one.
            let mut shown = vec![0u8; self.snap.procs.len()];
            if filtering {
                for i in 0..shown.len() as u32 {
                    if hit(i) {
                        shown[i as usize] = 2;
                        let mut at = self.parent[i as usize];
                        while at != NO_PARENT && shown[at as usize] == 0 {
                            shown[at as usize] = 1;
                            at = self.parent[at as usize];
                        }
                    }
                }
            }
            let mut stack: Vec<(u32, u16)> = self.roots.iter().rev().map(|r| (*r, 0)).collect();
            while let Some((i, depth)) = stack.pop() {
                if filtering && shown[i as usize] == 0 {
                    continue;
                }
                let kids = &self.children[i as usize];
                let branch = if filtering { kids.iter().any(|k| shown[*k as usize] > 0) } else { !kids.is_empty() };
                let open = branch && (filtering || self.open_nodes.contains(&self.snap.procs[i as usize].key()));
                self.view.push(Row::Node(i));
                self.meta.push(Meta { depth, faded: filtering && shown[i as usize] == 1, branch, open });
                if open {
                    stack.extend(kids.iter().rev().map(|k| (*k, depth + 1)));
                }
            }
        } else {
            for row in &self.top {
                match *row {
                    Row::Group(g) => {
                        let group = &self.groups[g as usize];
                        let open = filtering || self.expanded.contains(&group.id);
                        if filtering && !group.members.iter().any(|m| hit(*m)) {
                            continue;
                        }
                        self.view.push(*row);
                        self.meta.push(Meta { open, ..flat });
                        if open {
                            for m in group.members.iter().copied().filter(|m| hit(*m)) {
                                self.view.push(Row::Child(m));
                                self.meta.push(Meta { depth: 1, ..flat });
                            }
                        }
                    }
                    Row::Proc(i) if hit(i) => {
                        self.view.push(*row);
                        self.meta.push(flat);
                    }
                    _ => {}
                }
            }
        }
        // Ended processes stay where they were.
        let mut ghosts: Vec<usize> = (0..self.ghosts.len())
            .filter(|g| !filtering || terms.iter().all(|t| self.ghosts[*g].hay.contains(t.as_str())))
            .collect();
        ghosts.sort_by_key(|g| self.ghosts[*g].index);
        for g in ghosts {
            let ghost = &self.ghosts[g];
            let at = ghost.index.min(self.view.len());
            self.view.insert(at, Row::Ghost(g as u32));
            self.meta.insert(at, Meta { depth: ghost.depth, ..flat });
        }
        self.clamp_scroll();
    }

    /// The row of the range table a row of the view is.
    pub(super) fn which(row: Row) -> Option<Which> {
        match row {
            Row::Proc(i) | Row::Child(i) | Row::Node(i) => Some(Which::Proc(i)),
            Row::Group(g) => Some(Which::Group(g)),
            Row::Ghost(_) => None,
        }
    }

    /// Appends the text of a cell. `members` is the size of the group the row totals.
    pub(super) fn cell_text(&self, col: AnyCol, row: Row, members: Option<usize>, out: &mut String) {
        match col {
            AnyCol::P(c) => fill_cell(c, self.row_proc(row), members, &self.scale, out),
            AnyCol::R(c) => {
                let p = self.row_proc(row);
                match c.item {
                    RItem::Name => fill_cell(Col::Name, p, members, &self.scale, out),
                    RItem::Pid => fill_cell(Col::Pid, p, members, &self.scale, out),
                    RItem::Time => {
                        let (Some(data), Some(w)) = (&self.range, Self::which(row)) else { return };
                        let l = data.life(w);
                        match (l.born, l.died) {
                            (true, true) => {
                                let _ = write!(out, "{}〜{}", self.tl.clock(l.born_at), self.tl.clock(l.died_at));
                            }
                            (true, false) => out.push_str(&self.tl.clock(l.born_at)),
                            (false, true) => out.push_str(&self.tl.clock(l.died_at)),
                            (false, false) => {}
                        }
                    }
                    _ => {
                        if let (Some(data), Some(w)) = (&self.range, Self::which(row)) {
                            out.push_str(&data.text(w, c));
                        }
                    }
                }
            }
        }
    }

    /// The table as shown, for a file: every process the filter lets through, in the
    /// table's order, with the shown columns in plain units.
    pub(super) fn table_dump(&self, s: &Settings) -> timeline::export::Table {
        // Files never carry paths, command lines or user names.
        let private = |c: &AnyCol| matches!(c, AnyCol::P(Col::Path | Col::CmdLine | Col::User));
        let cols: Vec<(AnyCol, u16)> = self.shown_columns(s).into_iter().filter(|(c, _)| !private(c)).collect();
        let head = cols.iter().map(|(c, _)| export_name(*c)).collect();
        let hit = |i: u32| self.filter.is_empty() || {
            let hay = haystack(&self.snap.procs[i as usize]);
            self.filter.iter().all(|t| hay.contains(t.as_str()))
        };
        let rows = self
            .sorted
            .iter()
            .copied()
            .filter(|i| hit(*i))
            .map(|i| cols.iter().map(|(c, _)| self.export_cell(*c, Which::Proc(i))).collect())
            .collect();
        timeline::export::Table { head, rows }
    }

    /// The whole record summed up as one range, for a file: heaviest first by the table's
    /// sort column, the filter applied.
    pub(super) fn whole_table(&self, samples: &[Arc<timeline::Sample>], s: &Settings) -> timeline::export::Table {
        let items: Vec<RItem> = s.range_items.iter().map(|(i, _)| *i).collect();
        let (data, procs) = RangeData::build(samples, &items, |r| self.tl.unpack(r));
        let cols: Vec<RCol> = range::visible(&s.range_items, s.range_ops).into_iter().map(|(c, _)| c).collect();
        let mut order: Vec<u32> = (0..procs.len() as u32)
            .filter(|i| {
                let hay = haystack(&procs[*i as usize]);
                self.filter.iter().all(|t| hay.contains(t.as_str()))
            })
            .collect();
        let sort = s.range_sort;
        order.sort_by(|a, b| {
            let (pa, pb) = (&procs[*a as usize], &procs[*b as usize]);
            let o = match sort.item {
                RItem::Name => columns::compare(Col::Name, pa, pb),
                RItem::Pid => pa.pid.cmp(&pb.pid),
                _ => data.key(Which::Proc(*a), sort).total_cmp(&data.key(Which::Proc(*b), sort)),
            };
            (if s.range_desc { o.reverse() } else { o }).then(pa.pid.cmp(&pb.pid))
        });
        let head = cols.iter().map(|c| export_name(AnyCol::R(*c))).collect();
        let rows = order
            .iter()
            .map(|i| cols.iter().map(|c| data.export(&procs[*i as usize], *c, Which::Proc(*i), self.tl.offset)).collect())
            .collect();
        timeline::export::Table { head, rows }
    }

    /// A cell for a file: a plain number in base units, or text.
    fn export_cell(&self, col: AnyCol, w: Which) -> String {
        let Which::Proc(i) = w else { return String::new() };
        let p = &self.snap.procs[i as usize];
        match (col, &self.range) {
            (AnyCol::P(c), _) => columns::export_value(c, p, &self.scale),
            (AnyCol::R(c), Some(data)) => data.export(p, c, w, self.tl.offset),
            (AnyCol::R(_), None) => String::new(),
        }
    }

    pub(super) fn row_proc(&self, row: Row) -> &Proc {
        match row {
            Row::Proc(i) | Row::Child(i) | Row::Node(i) => &self.snap.procs[i as usize],
            Row::Group(g) => &self.groups[g as usize].summary,
            Row::Ghost(g) => &self.ghosts[g as usize].proc,
        }
    }

    pub(super) fn row_sel(&self, row: Row) -> Sel {
        match row {
            Row::Group(g) => Sel::Group(self.groups[g as usize].id.clone()),
            _ => Sel::Proc(self.row_proc(row).key()),
        }
    }

    pub(super) fn row_selected(&self, row: Row) -> bool {
        match (row, self.selected.as_ref()) {
            (Row::Group(g), Some(Sel::Group(id))) => self.groups[g as usize].id == *id,
            (Row::Group(_), _) | (_, Some(Sel::Group(_))) | (_, None) => false,
            (_, Some(Sel::Proc(key))) => self.row_proc(row).key() == *key,
        }
    }

    /// The selection is a process that ended a moment ago.
    pub(super) fn ghost_selected(&self) -> bool {
        let Some(Sel::Proc(key)) = &self.selected else { return false };
        self.ghosts.iter().any(|g| g.proc.key() == *key)
    }

    pub(super) fn toggle_group(&mut self, g: u32) {
        // Filtering shows every matching member regardless of this state.
        if !self.filter.is_empty() {
            return;
        }
        let group = &self.groups[g as usize];
        let id = group.id.clone();
        if self.expanded.remove(&id) {
            if let Some(Sel::Proc(key)) = self.selected {
                let procs = &self.snap.procs;
                if group.members.iter().any(|m| procs[*m as usize].key() == key) {
                    self.selected = Some(Sel::Group(id));
                }
            }
        } else {
            self.expanded.insert(id);
        }
        self.refilter();
    }

    /// Whether process `i` lies below process `ancestor` in the tree.
    pub(super) fn descends_from(&self, i: usize, ancestor: u32) -> bool {
        let mut at = self.parent.get(i).copied().unwrap_or(NO_PARENT);
        while at != NO_PARENT {
            if at == ancestor {
                return true;
            }
            at = self.parent[at as usize];
        }
        false
    }

    /// The selected process as an index into the snapshot.
    pub(super) fn selected_index_in_snap(&self) -> Option<usize> {
        let Some(Sel::Proc(key)) = &self.selected else { return None };
        self.snap.procs.iter().position(|p| p.key() == *key)
    }

    pub(super) fn toggle_node(&mut self, i: u32) {
        if !self.filter.is_empty() {
            return;
        }
        let key = self.snap.procs[i as usize].key();
        if self.open_nodes.remove(&key) {
            // A selection inside the closed branch moves up to it.
            if self.selected_index_in_snap().is_some_and(|s| self.descends_from(s, i)) {
                self.selected = Some(Sel::Proc(key));
            }
        } else {
            self.open_nodes.insert(key);
        }
        self.refilter();
    }

    pub(super) fn expand_all(&mut self) {
        match self.mode {
            ViewMode::Group => self.expanded = self.groups.iter().map(|g| g.id.clone()).collect(),
            ViewMode::Tree => {
                let procs = &self.snap.procs;
                self.open_nodes = self
                    .children
                    .iter()
                    .enumerate()
                    .filter(|(_, kids)| !kids.is_empty())
                    .map(|(i, _)| procs[i].key())
                    .collect();
            }
            ViewMode::Flat => {}
        }
        self.refilter();
    }

    pub(super) fn collapse_all(&mut self) {
        match self.mode {
            ViewMode::Group => {
                self.expanded.clear();
                if let Some(Sel::Proc(key)) = self.selected {
                    let procs = &self.snap.procs;
                    if let Some(g) = self.groups.iter().find(|g| g.members.iter().any(|m| procs[*m as usize].key() == key)) {
                        self.selected = Some(Sel::Group(g.id.clone()));
                    }
                }
            }
            ViewMode::Tree => {
                self.open_nodes.clear();
                if let Some(mut i) = self.selected_index_in_snap() {
                    while self.parent[i] != NO_PARENT {
                        i = self.parent[i] as usize;
                    }
                    self.selected = Some(Sel::Proc(self.snap.procs[i].key()));
                }
            }
            ViewMode::Flat => {}
        }
        self.refilter();
    }

    /// Notes which processes started and ended since the previous update. `self.snap` and
    /// the view still hold the previous update.
    pub(super) fn track_lifetime(&mut self, snap: &Snapshot, settings: &Settings, now: Instant) {
        self.generation += 1;
        let generation = self.generation;
        if !settings.highlight_lifetime {
            self.born.clear();
            self.ghosts.clear();
            self.prev_at = Some(now);
            return;
        }
        // The first list marks nothing: everything would be new.
        if let Some(prev) = self.prev_at.filter(|_| !self.snap.procs.is_empty()) {
            // After a long gap (minimized, paused) the moment of a change is unknown.
            let interval = Duration::from_millis(settings.interval_ms.max(500) as u64);
            let recent = now.duration_since(prev) <= (interval * 2).max(LIFETIME_HOLD + Duration::from_secs(1));
            let before: HashSet<(u32, i64)> = self.snap.procs.iter().map(Proc::key).collect();
            let after: HashSet<(u32, i64)> = snap.procs.iter().map(Proc::key).collect();
            let clock = filetime_now();
            for p in &snap.procs {
                let young = clock - p.create_time < LIFETIME_HOLD.as_secs() as i64 * SEC;
                if !before.contains(&p.key()) && (recent || young) {
                    self.born.insert(p.key(), (now, generation));
                }
            }
            if recent {
                for (index, row) in self.view.iter().enumerate() {
                    let (Row::Proc(i) | Row::Child(i) | Row::Node(i)) = *row else { continue };
                    let p = &self.snap.procs[i as usize];
                    if !after.contains(&p.key()) {
                        self.ghosts.push(Ghost {
                            proc: p.clone(),
                            hay: haystack(p),
                            at: now,
                            generation,
                            index,
                            depth: self.meta[index].depth,
                        });
                    }
                }
            }
        }
        let keep = |at: Instant, g: u64| now.duration_since(at) < LIFETIME_HOLD || generation - g < LIFETIME_UPDATES;
        self.born.retain(|_, (at, g)| keep(*at, *g));
        self.ghosts.retain(|g| keep(g.at, g.generation));
        self.prev_at = Some(now);
    }

    /// Takes a new update. While the table shows the past, only the latest is kept.
    pub(super) fn take_snapshot(&mut self, snap: &Arc<Snapshot>, settings: &Settings, now: Instant) {
        self.live = snap.clone();
        if self.past {
            return;
        }
        self.track_lifetime(snap, settings, now);
        self.snap = snap.clone();
        self.haystacks.clear();
        self.regroup(settings);
    }

    /// Whether the filter lets a process (an index into the snapshot) through.
    pub(super) fn matches(&mut self, i: usize) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        if self.haystacks.len() != self.snap.procs.len() {
            self.haystacks = self.snap.procs.iter().map(haystack).collect();
        }
        let hay = &self.haystacks[i];
        self.filter.iter().all(|t| hay.contains(t.as_str()))
    }

    /// Opens whatever hides a process (an index into the snapshot) in the list.
    pub(super) fn open_to(&mut self, i: usize) {
        if let Some(g) = self.groups.iter().find(|g| g.members.contains(&(i as u32))) {
            self.expanded.insert(g.id.clone());
        }
        let mut at = self.parent.get(i).copied().unwrap_or(NO_PARENT);
        while at != NO_PARENT {
            self.open_nodes.insert(self.snap.procs[at as usize].key());
            at = self.parent[at as usize];
        }
    }

    /// Selects a process by PID and scrolls it to the middle of the list.
    pub fn reveal(&mut self, pid: u32) -> Reveal {
        let Some(i) = self.snap.procs.iter().position(|p| p.pid == pid) else { return Reveal::Missing };
        if !self.matches(i) {
            return Reveal::Filtered;
        }
        self.open_to(i);
        self.selected = Some(Sel::Proc(self.snap.procs[i].key()));
        self.refilter();
        let Some(index) = self.selected_index() else { return Reveal::Missing };
        let visible = self.l.body.h - self.hbar_h();
        let y = (index as i32 * self.l.row_h - (visible - self.l.row_h) / 2) as f32;
        self.target_y = y.clamp(0.0, self.max_scroll_y());
        self.scroll_y = self.target_y;
        Reveal::Shown
    }
}
