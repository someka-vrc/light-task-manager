//! Building the shown rows: grouping, the tree, sorting, filtering, and the tints of
//! processes that just started or ended.

use super::*;

fn haystack(p: &Proc) -> String {
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
        let (col, desc) = (settings.sort, settings.descending);
        let compare = |a: &Proc, b: &Proc| {
            let order = columns::compare(col, a, b);
            let order = if desc { order.reverse() } else { order };
            order.then(a.pid.cmp(&b.pid))
        };
        self.sorted.clear();
        self.sorted.extend(0..procs.len() as u32);
        self.sorted.sort_unstable_by(|a, b| compare(&procs[*a as usize], &procs[*b as usize]));
        for g in &mut self.groups {
            g.members.sort_unstable_by(|a, b| compare(&procs[*a as usize], &procs[*b as usize]));
        }
        // The tree sorts only among siblings.
        self.roots.sort_unstable_by(|a, b| compare(&procs[*a as usize], &procs[*b as usize]));
        for kids in &mut self.children {
            kids.sort_unstable_by(|a, b| compare(&procs[*a as usize], &procs[*b as usize]));
        }

        let groups = &self.groups;
        let mut order: Vec<u32> = (0..groups.len() as u32).collect();
        order.sort_unstable_by(|a, b| compare(&groups[*a as usize].summary, &groups[*b as usize].summary));
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
                (Some(s), Some(g)) => compare(&groups[*g as usize].summary, &procs[*s as usize]).is_lt(),
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

    /// Takes a new update.
    pub(super) fn take_snapshot(&mut self, snap: &Arc<Snapshot>, settings: &Settings, now: Instant) {
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
