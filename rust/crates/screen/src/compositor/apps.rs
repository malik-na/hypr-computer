//! The agent's apps. The screen starts them, so they are its children, and it
//! is their subreaper: what they start and leave behind becomes its child in
//! turn, whatever process group or session it moved to. So everything a
//! session started is below its screen, where the daemon's teardown finds it.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use rustix::process::{WaitOptions, getpid, set_child_subreaper, waitpid};
use screen::protocol::app_log;

/// What would lead an app to the user's desktop instead of this screen.
const USERS_DESKTOP: &[&str] = &["DISPLAY", "WAYLAND_SOCKET", "HYPRLAND_INSTANCE_SIGNATURE"];

pub struct Apps {
    /// The screen's Wayland socket; its directory holds the apps' output.
    socket: PathBuf,
    running: HashSet<u32>,
    /// The exit status of each app that ended, by pid.
    ended: HashMap<u32, String>,
}

impl Apps {
    pub fn new(socket: &Path) -> io::Result<Self> {
        set_child_subreaper(Some(getpid()))?;
        Ok(Apps {
            socket: socket.to_owned(),
            running: HashSet::new(),
            ended: HashMap::new(),
        })
    }

    /// Starts `program` with this screen as its display and none of the
    /// user's, and returns its pid.
    pub fn launch(&mut self, n: u64, program: &str, args: &[String]) -> io::Result<u32> {
        let dir = self.socket.parent().unwrap_or(Path::new("/"));
        let log = File::create(dir.join(app_log(n)))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env("WAYLAND_DISPLAY", &self.socket)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        for name in USERS_DESKTOP {
            command.env_remove(name);
        }
        // Not waited on through `Child`: `reap` collects every child.
        let pid = command.spawn()?.id();
        self.running.insert(pid);
        Ok(pid)
    }

    /// `None` while the app with this pid runs, then its exit status.
    pub fn ended(&mut self, pid: u32) -> Result<Option<&str>, String> {
        self.reap();
        if self.running.contains(&pid) {
            return Ok(None);
        }
        match self.ended.get(&pid) {
            Some(status) => Ok(Some(status)),
            None => Err(format!("no app with pid {pid} was started here")),
        }
    }

    /// Collects every child that ended: apps, and what they left behind.
    pub fn reap(&mut self) {
        while let Ok(Some((pid, status))) = waitpid(None, WaitOptions::NOHANG) {
            let pid = pid.as_raw_nonzero().get().unsigned_abs();
            if self.running.remove(&pid) {
                let status = ExitStatus::from_raw(status.as_raw()).to_string();
                self.ended.insert(pid, status);
            }
        }
    }
}
