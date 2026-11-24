//! A fake desktop for the live views, in the test's own dir: a Wayland
//! compositor for the views to open on (the agent's screen program, run as
//! the user's desktop), with a fake Hyprland next to it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::hyprland::{Hyprland, SIGNATURE};
use super::{Env, Image, TIMEOUT, bin};

/// The app id every live view has.
pub const VIEW_APP_ID: &str = "hypr-computer-view";

/// A window on the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub app_id: String,
    pub title: String,
}

pub struct Desktop {
    compositor: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    socket: PathBuf,
    pub hyprland: Hyprland,
}

/// The desktop's compositor, started on `socket`, once it is ready.
fn compositor(socket: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut compositor = Command::new(bin("hypr-computer-screen"))
        .arg(socket)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = compositor.stdin.take().unwrap();
    let mut stdout = BufReader::new(compositor.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n", "the desktop's compositor didn't start");
    (compositor, stdin, stdout)
}

impl Desktop {
    pub fn start(env: &Env) -> Desktop {
        let dir = env.path("desktop");
        std::fs::create_dir(&dir).unwrap();
        let socket = dir.join("wayland-0");
        let (compositor, stdin, stdout) = compositor(&socket);
        Desktop {
            compositor,
            stdin,
            stdout,
            socket,
            hyprland: Hyprland::start(&dir),
        }
    }

    /// Restarts the compositor, as when Hyprland restarts: every window on
    /// the desktop goes.
    pub fn restart(&mut self) {
        let _ = self.compositor.kill();
        let _ = self.compositor.wait();
        let lock = self.socket.with_extension("lock");
        for file in [&self.socket, &lock] {
            let _ = std::fs::remove_file(file);
        }
        (self.compositor, self.stdin, self.stdout) = compositor(&self.socket);
    }

    /// Points `cmd` (the daemon) at this desktop, as a Hyprland session does.
    pub fn apply(&self, cmd: &mut Command) {
        cmd.env("WAYLAND_DISPLAY", &self.socket)
            .env("HYPRLAND_INSTANCE_SIGNATURE", SIGNATURE);
    }

    /// The desktop's Wayland socket, for apps of the user's.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn ask(&mut self, request: &str) -> Vec<u8> {
        writeln!(self.stdin, "{request}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let len: usize = line
            .split_once(' ')
            .and_then(|(_, n)| n.trim_end().parse().ok())
            .unwrap_or_else(|| panic!("the desktop answered {line:?}"));
        let mut bytes = vec![0; len];
        self.stdout.read_exact(&mut bytes).unwrap();
        bytes
    }

    /// The windows on the desktop, oldest first.
    pub fn windows(&mut self) -> Vec<Window> {
        let list: Value = serde_json::from_slice(&self.ask("windows")).unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .map(|w| Window {
                app_id: w["app_id"].as_str().unwrap().to_owned(),
                title: w["title"].as_str().unwrap().to_owned(),
            })
            .collect()
    }

    /// The live views on the desktop, oldest first.
    pub fn views(&mut self) -> Vec<Window> {
        let mut views = self.windows();
        views.retain(|w| w.app_id == VIEW_APP_ID);
        views
    }

    /// Waits until there are `n` live views on the desktop, and returns them.
    pub fn wait_for_views(&mut self, n: usize) -> Vec<Window> {
        let end = Instant::now() + TIMEOUT;
        loop {
            let views = self.views();
            if views.len() == n {
                return views;
            }
            assert!(
                Instant::now() < end,
                "wanted {n} views on the desktop, got {views:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// What the desktop shows.
    pub fn screenshot(&mut self) -> Image {
        Image::from_png(&self.ask("screenshot"))
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.compositor.kill();
        let _ = self.compositor.wait();
    }
}
