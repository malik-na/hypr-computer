//! Which processes are below which, as `/proc` has it now.

use std::collections::HashMap;
use std::fs;

pub(crate) struct Process {
    pub(crate) parent: u32,
    /// It has ended, and waits for its parent to collect its exit status.
    pub(crate) zombie: bool,
}

/// Every process, by pid.
pub(crate) fn processes() -> HashMap<u32, Process> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return HashMap::new();
    };
    entries
        .filter_map(|entry| {
            let pid = entry.ok()?.file_name().to_str()?.parse().ok()?;
            Some((pid, read(pid)?))
        })
        .collect()
}

/// From `/proc/<pid>/stat`, the state and the parent's pid: the fields
/// after the command's name, which is in parentheses and may hold any.
fn read(pid: u32) -> Option<Process> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after_name) = stat.rsplit_once(')')?;
    let mut fields = after_name.split_whitespace();
    let state = fields.next()?;
    Some(Process {
        parent: fields.next()?.parse().ok()?,
        zombie: state == "Z",
    })
}

/// Everything below `roots`, at any depth, not counting `roots`.
pub(crate) fn below(processes: &HashMap<u32, Process>, roots: &[u32]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut next = roots.to_vec();
    while let Some(parent) = next.pop() {
        for (&pid, _) in processes.iter().filter(|(_, p)| p.parent == parent) {
            found.push(pid);
            next.push(pid);
        }
    }
    found
}

/// Whether any of `pids` is still running, not just waiting to be collected.
pub(crate) fn any_running(processes: &HashMap<u32, Process>, pids: &[u32]) -> bool {
    pids.iter()
        .any(|pid| processes.get(pid).is_some_and(|p| !p.zombie))
}
