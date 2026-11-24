//! What an app started that left the screen's process group, as `setsid`
//! and daemons do: found by the runtime dir that every process on the
//! screen inherits from its app, and killed with the rest at teardown.

use std::fs;
use std::os::unix::ffi::OsStrExt;

use rustix::process::{Pid, Signal, kill_process};

use crate::process::ScreenChild;

impl ScreenChild {
    /// Sends SIGKILL to every process that has this screen's runtime dir;
    /// `true` if there was one.
    pub(crate) fn kill_strays(&self) -> bool {
        let mut mark = b"XDG_RUNTIME_DIR=".to_vec();
        mark.extend(self.app_runtime_dir().as_os_str().as_bytes());
        let Ok(processes) = fs::read_dir("/proc") else {
            return false;
        };
        let mut found = false;
        for process in processes.flatten() {
            let pid = process.file_name().to_str().and_then(|n| n.parse().ok());
            let Some(pid) = pid.and_then(Pid::from_raw) else {
                continue;
            };
            // Another user's, or gone: not ours either way.
            let Ok(environ) = fs::read(process.path().join("environ")) else {
                continue;
            };
            if environ.split(|b| *b == 0).any(|var| var == mark.as_slice()) {
                let _ = kill_process(pid, Signal::KILL);
                found = true;
            }
        }
        found
    }
}
