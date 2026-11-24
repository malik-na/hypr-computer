//! Removing the logs kept long enough: a session's log (its `.jsonl` file
//! and the dir of its pictures, named alike) goes once the session has ended
//! and the file was last written more than the kept days ago. Done when the
//! daemon starts and each time an agent connects; a running session's log is
//! never removed, however old, nor are the logs of another daemon still
//! running on the same state dir (with a runtime dir of its own).

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use tracing::{info, warn};

use super::ActionLog;

impl ActionLog {
    pub fn sweep(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let name = file_name.strip_suffix(".jsonl").unwrap_or(file_name);
            if self.running().contains(name) || self.another_daemons(name) {
                continue;
            }
            // Its age is its `.jsonl` file's, or its own once that is gone.
            let modified = fs::symlink_metadata(self.dir.join(format!("{name}.jsonl")))
                .or_else(|_| fs::symlink_metadata(&path))
                .and_then(|meta| meta.modified());
            let old = modified.is_ok_and(|when| {
                SystemTime::now()
                    .duration_since(when)
                    .is_ok_and(|age| age > self.keep)
            });
            if !old {
                continue;
            }
            let removed = match entry.file_type() {
                Ok(kind) if kind.is_dir() => fs::remove_dir_all(&path),
                _ => fs::remove_file(&path),
            };
            match removed {
                Ok(()) => info!("removed an old log, {file_name}"),
                // Another sweep got there first.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => warn!("can't remove the old log {}: {e}", path.display()),
            }
        }
    }

    /// Whether the log `name` is of another daemon that is still running:
    /// its run (`<start>-<pid>`) has a pid that is alive. A pid reused since
    /// only keeps the log longer.
    fn another_daemons(&self, name: &str) -> bool {
        let Some((run, _)) = name.split_once(".session-") else {
            return false;
        };
        let pid = run
            .rsplit('-')
            .next()
            .filter(|pid| pid.parse::<u32>().is_ok());
        run != self.run && pid.is_some_and(|pid| Path::new("/proc").join(pid).exists())
    }
}
