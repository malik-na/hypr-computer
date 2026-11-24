//! Which app a window on the screen belongs to, for the user's app limits:
//! the program `launch_app` started, or the browser `open_url` did, whose
//! process, or one of its children's, made the window.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// How far up from a window's process to look for the app that started it.
const MAX_DEPTH: usize = 64;

/// The processes started on a screen, and the app each one is.
#[derive(Default)]
pub(crate) struct Apps(Mutex<HashMap<u32, String>>);

impl Apps {
    /// Process `pid` was started as app `name`.
    pub(crate) fn started(&self, pid: u32, name: &str) {
        if let Ok(mut apps) = self.0.lock() {
            // A process that is gone can't make a window; its pid may come back as another's.
            apps.retain(|pid, _| Path::new(&format!("/proc/{pid}")).exists());
            apps.insert(pid, name.to_owned());
        }
    }

    /// The app process `pid` belongs to: the one it was started as, or the
    /// one its nearest started ancestor was.
    pub(crate) fn of(&self, pid: u32) -> Option<String> {
        let apps = self.0.lock().ok()?;
        let mut pid = pid;
        for _ in 0..MAX_DEPTH {
            if let Some(name) = apps.get(&pid) {
                return Some(name.clone());
            }
            pid = parent(pid).filter(|parent| *parent > 1)?;
        }
        None
    }
}

/// The parent of process `pid`, from `/proc/<pid>/stat`.
fn parent(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, in parentheses, may itself hold spaces and parentheses.
    let (_, after) = stat.rsplit_once(')')?;
    after.split_whitespace().nth(1)?.parse().ok()
}

/// The app name of `program`, as launch_app gives it: its file name.
pub(crate) fn name_of(program: &str) -> String {
    Path::new(program).file_name().map_or_else(
        || program.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}
