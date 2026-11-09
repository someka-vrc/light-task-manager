//! Groups processes the way Task Manager's Processes tab does.
//!
//! Packaged apps group by package, so unrelated processes of one package join.
//! Everything else groups along parent links between processes of one
//! executable. Children running a different executable stay separate.

use crate::model::{Proc, ProcStatic, Status};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_DEPTH: usize = 64;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum GroupId {
    /// Package full name and session.
    Package(Arc<str>, u32),
    /// PID and creation time of the topmost process.
    Tree(u32, i64),
}

pub struct Group {
    pub id: GroupId,
    /// Indexes into the snapshot's process list.
    pub members: Vec<u32>,
    /// Totals of the members, labelled for the group.
    pub summary: Proc,
}

fn tree_root<'a>(procs: &'a [Proc], by_pid: &HashMap<u32, u32>, p: &'a Proc) -> Option<&'a Proc> {
    let image = p.stat.image.as_ref()?;
    let mut current = p;
    for _ in 0..MAX_DEPTH {
        let Some(index) = by_pid.get(&current.ppid) else { break };
        let parent = &procs[*index as usize];
        let same_image = parent.stat.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, image));
        // A parent younger than its child means the PID was reused.
        if parent.pid == current.pid || !same_image || parent.create_time > current.create_time {
            break;
        }
        current = parent;
    }
    Some(current)
}

fn summarize(procs: &[Proc], members: &[u32]) -> Proc {
    let lead = members
        .iter()
        .map(|m| &procs[*m as usize])
        .min_by_key(|p| (!p.has_window, p.create_time, p.pid))
        .expect("a group has members");
    let description = lead.stat.description();
    let name = if description.is_empty() { lead.stat.name.clone() } else { description.to_string() };
    let mut s = lead.clone();
    s.stat = Arc::new(ProcStatic {
        name,
        user: lead.stat.user.clone(),
        arch: lead.stat.arch,
        elevated: lead.stat.elevated,
        cmdline: lead.stat.cmdline.clone(),
        image: lead.stat.image.clone(),
        package: lead.stat.package.clone(),
    });
    s.cpu = 0.0;
    macro_rules! zero { ($($f:ident),*) => { $(s.$f = 0;)* } }
    macro_rules! add { ($p:ident, $($f:ident),*) => { $(s.$f += $p.$f;)* } }
    zero!(cpu_time, cycles, ws, peak_ws, ws_delta, private_ws, active_private, commit, paged, nonpaged,
        page_faults, pf_delta, handles, threads, user_objects, gdi_objects, io_reads, io_writes, io_other,
        io_read_bytes, io_write_bytes, io_other_bytes);
    let (mut hung, mut all_suspended) = (false, true);
    for m in members {
        let p = &procs[*m as usize];
        s.cpu += p.cpu;
        add!(p, cpu_time, cycles, ws, peak_ws, ws_delta, private_ws, active_private, commit, paged, nonpaged,
            page_faults, pf_delta, handles, threads, user_objects, gdi_objects, io_reads, io_writes, io_other,
            io_read_bytes, io_write_bytes, io_other_bytes);
        hung |= p.status == Status::NotResponding;
        all_suspended &= p.status == Status::Suspended;
    }
    s.status = if hung {
        Status::NotResponding
    } else if all_suspended {
        Status::Suspended
    } else {
        Status::Running
    };
    s
}

/// Returns the groups of two or more processes, and whether each process is in one.
pub fn build(procs: &[Proc]) -> (Vec<Group>, Vec<bool>) {
    let by_pid: HashMap<u32, u32> =
        procs.iter().enumerate().map(|(i, p)| (p.pid, i as u32)).collect();
    let mut found: HashMap<GroupId, Vec<u32>> = HashMap::new();
    for (i, p) in procs.iter().enumerate() {
        let id = match &p.stat.package {
            Some(package) => Some(GroupId::Package(package.clone(), p.session)),
            None => tree_root(procs, &by_pid, p).map(|r| GroupId::Tree(r.pid, r.create_time)),
        };
        if let Some(id) = id {
            found.entry(id).or_default().push(i as u32);
        }
    }
    let mut in_group = vec![false; procs.len()];
    let mut groups = Vec::new();
    for (id, members) in found {
        if members.len() < 2 {
            continue;
        }
        for m in &members {
            in_group[*m as usize] = true;
        }
        let summary = summarize(procs, &members);
        groups.push(Group { id, members, summary });
    }
    (groups, in_group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImageInfo;
    use std::sync::OnceLock;

    fn image(path: &str) -> Arc<ImageInfo> {
        Arc::new(ImageInfo { path: path.into(), description: OnceLock::new(), icon: OnceLock::new() })
    }

    fn proc(pid: u32, ppid: u32, created: i64, image: &Arc<ImageInfo>, package: Option<&str>) -> Proc {
        Proc {
            pid,
            ppid,
            create_time: created,
            stat: Arc::new(ProcStatic {
                name: format!("p{pid}.exe"),
                user: Arc::from("u"),
                arch: "x64",
                elevated: None,
                cmdline: String::new(),
                image: Some(image.clone()),
                package: package.map(Arc::from),
            }),
            status: Status::Running,
            session: 1,
            has_window: false,
            cpu: 1.5,
            cpu_time: 1,
            cycles: 1,
            ws: 100,
            peak_ws: 1,
            ws_delta: 1,
            private_ws: 10,
            active_private: 10,
            commit: 1,
            paged: 1,
            nonpaged: 1,
            page_faults: 1,
            pf_delta: 1,
            base_priority: 8,
            handles: 1,
            threads: 2,
            user_objects: 0,
            gdi_objects: 0,
            io_reads: 0,
            io_writes: 0,
            io_other: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            io_other_bytes: 0,
        }
    }

    #[test]
    fn same_image_parent_chain_is_one_group() {
        let code = image("code.exe");
        let procs = vec![proc(10, 1, 1, &code, None), proc(11, 10, 2, &code, None), proc(12, 11, 3, &code, None)];
        let (groups, in_group) = build(&procs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 3);
        assert_eq!(groups[0].id, GroupId::Tree(10, 1));
        assert!(in_group.iter().all(|g| *g));
    }

    #[test]
    fn different_image_child_stays_out() {
        let (code, helper) = (image("code.exe"), image("helper.exe"));
        let procs = vec![
            proc(10, 1, 1, &code, None),
            proc(11, 10, 2, &code, None),
            proc(20, 10, 3, &helper, None),
        ];
        let (groups, in_group) = build(&procs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(in_group, [true, true, false]);
    }

    #[test]
    fn package_joins_unrelated_processes() {
        let (a, b) = (image("terminal.exe"), image("dllhost.exe"));
        let mut procs = vec![
            proc(30, 2, 1, &b, Some("Pkg")),
            proc(40, 2, 2, &a, Some("Pkg")),
            proc(50, 2, 3, &a, None),
        ];
        procs[1].has_window = true;
        let (groups, in_group) = build(&procs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(in_group, [true, true, false]);
        // The process with a window names the group, even if it started later.
        assert_eq!(groups[0].summary.pid, 40);
    }

    #[test]
    fn reused_parent_pid_is_not_a_parent() {
        let code = image("code.exe");
        let procs = vec![proc(10, 1, 9, &code, None), proc(11, 10, 2, &code, None)];
        assert!(build(&procs).0.is_empty());
    }

    #[test]
    fn single_process_is_not_a_group() {
        let code = image("code.exe");
        let (groups, in_group) = build(&[proc(10, 1, 1, &code, None)]);
        assert!(groups.is_empty());
        assert_eq!(in_group, [false]);
    }

    #[test]
    fn summary_adds_members() {
        let code = image("code.exe");
        let mut procs = vec![proc(10, 1, 1, &code, None), proc(11, 10, 2, &code, None)];
        procs[1].status = Status::NotResponding;
        let (groups, _) = build(&procs);
        let s = &groups[0].summary;
        assert_eq!(s.ws, 200);
        assert_eq!(s.private_ws, 20);
        assert_eq!(s.threads, 4);
        assert!((s.cpu - 3.0).abs() < 0.001);
        assert!(s.status == Status::NotResponding);
    }
}
