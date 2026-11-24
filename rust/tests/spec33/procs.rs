//! Finding the browser's processes from outside, as a user would with `ps`:
//! every process whose environment carries the wrapper's mark, and every
//! descendant of a browser launch. A Chromium sandboxed child may hide its
//! environment, so a test takes a snapshot while the browser runs (when
//! descendants can still be traced) and checks it again later.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

/// A process, told apart from a later one with the same pid by its start
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Proc {
    pub pid: u32,
    start: u64,
}

/// `(ppid, start time, state)` of `pid`, from /proc/<pid>/stat.
fn stat(pid: u32) -> Option<(u32, u64, char)> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = s.rsplit_once(')')?.1.trim_start();
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After the name: state (3), ppid (4), ..., starttime (22).
    let state = fields.first()?.chars().next()?;
    let ppid = fields.get(1)?.parse().ok()?;
    let start = fields.get(19)?.parse().ok()?;
    Some((ppid, start, state))
}

fn all_pids() -> Vec<u32> {
    fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

fn has_mark(pid: u32, mark: &str) -> bool {
    let needle = format!("HC33_MARK={mark}");
    fs::read(format!("/proc/{pid}/environ"))
        .map(|env| env.split(|b| *b == 0).any(|v| v == needle.as_bytes()))
        .unwrap_or(false)
}

/// The running processes that carry `mark`, or descend from one of `roots`
/// (the wrapper's pids) or from a marked process.
pub fn browser_procs(mark: &str, roots: &[u32]) -> BTreeSet<Proc> {
    let mut info = HashMap::new();
    for pid in all_pids() {
        if let Some((ppid, start, state)) = stat(pid)
            && state != 'Z'
            && state != 'X'
        {
            info.insert(pid, (ppid, start));
        }
    }
    let mut ours: BTreeSet<u32> = info
        .keys()
        .copied()
        .filter(|&pid| roots.contains(&pid) || has_mark(pid, mark))
        .collect();
    // Add descendants until nothing changes.
    loop {
        let more: Vec<u32> = info
            .iter()
            .filter(|(pid, (ppid, _))| !ours.contains(pid) && ours.contains(ppid))
            .map(|(pid, _)| *pid)
            .collect();
        if more.is_empty() {
            break;
        }
        ours.extend(more);
    }
    ours.into_iter()
        .map(|pid| Proc {
            pid,
            start: info[&pid].1,
        })
        .collect()
}

/// The running processes that descend from one of `roots`, the roots
/// included.
pub fn tree(roots: &[u32]) -> BTreeSet<Proc> {
    browser_procs("\0never", roots)
}

/// Whether `p` is still running (a zombie has ended).
pub fn running(p: Proc) -> bool {
    matches!(stat(p.pid), Some((_, start, state)) if start == p.start && state != 'Z' && state != 'X')
}

pub fn cmdline(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

/// What is left of `procs`, with their command lines, for messages.
pub fn describe(procs: &BTreeSet<Proc>) -> String {
    procs
        .iter()
        .map(|p| format!("{} {}", p.pid, cmdline(p.pid)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Kills every marked browser process still running: a failing test must
/// leave nothing behind. (The rest of a browser goes with it.)
pub fn reap(mark: &str) {
    for p in browser_procs(mark, &[]) {
        let _ = Command::new("kill")
            .args(["-KILL", &p.pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
    }
}
