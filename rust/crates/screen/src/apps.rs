//! Which app a window on the screen belongs to, for the user's app limits:
//! the program `launch_app` started, or the browser `open_url` did, whose
//! process, or one of its children's, made the window.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// How far up from a window's process to look for the app that started it.
const MAX_DEPTH: usize = 64;

/// The processes started on a screen: for each pid, the app it was started
/// as and when it started, so a pid that comes back as another process's
/// isn't taken for the app.
#[derive(Default)]
pub(crate) struct Apps(Mutex<HashMap<u32, (String, u64)>>);

impl Apps {
    /// Process `pid` was started as app `name`.
    pub(crate) fn started(&self, pid: u32, name: &str) {
        let Some(since) = stat(pid).map(|s| s.start) else {
            return;
        };
        if let Ok(mut apps) = self.0.lock() {
            apps.retain(|pid, (_, since)| stat(*pid).is_some_and(|s| s.start == *since));
            apps.insert(pid, (name.to_owned(), since));
        }
    }

    /// The app process `pid` belongs to: the one it was started as, or the
    /// one its nearest started ancestor was.
    pub(crate) fn of(&self, pid: u32) -> Option<String> {
        let apps = self.0.lock().ok()?;
        let mut pid = pid;
        for _ in 0..MAX_DEPTH {
            let now = stat(pid)?;
            if let Some((name, since)) = apps.get(&pid)
                && now.start == *since
            {
                return Some(name.clone());
            }
            pid = Some(now.parent).filter(|parent| *parent > 1)?;
        }
        None
    }
}

/// When process `pid` started, in clock ticks since boot: with its pid, what
/// tells it from a process that gets the same pid later.
pub fn start_time(pid: u32) -> Option<u64> {
    stat(pid).map(|s| s.start)
}

/// What `/proc/<pid>/stat` says of a process.
struct Stat {
    parent: u32,
    /// When it started, in clock ticks since boot.
    start: u64,
}

fn stat(pid: u32) -> Option<Stat> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, in parentheses, may itself hold spaces and parentheses.
    let (_, after) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    Some(Stat {
        parent: fields.get(1)?.parse().ok()?,
        start: fields.get(19)?.parse().ok()?,
    })
}

/// The app name of `program`, as launch_app gives it: its file name.
pub(crate) fn name_of(program: &str) -> String {
    Path::new(program).file_name().map_or_else(
        || program.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}
