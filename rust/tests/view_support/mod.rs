//! The user's desktop for the live-view tests: a fake Hyprland and a host
//! Wayland compositor (see `hyprland` and `host`), with the real daemon, shim
//! and `hypr-computer` command run against them in a throwaway runtime dir.
//! Nothing here can reach the real desktop: every program gets a cleared
//! environment whose Hyprland and Wayland sockets are these fakes.

// Each test file uses a different subset of these helpers.
#![allow(dead_code, unused_imports)]

pub mod host;
pub mod hyprland;
mod lua;
pub mod probe;
mod regex;

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::support::{self, Daemon, Env, Image, Shim};
use host::{Host, HostWindow, Picture};
use hyprland::FakeHyprland;

pub use hyprland::{USER_WINDOW, USER_WORKSPACE};

/// The workspace where the live views go.
pub const VIEWS_WORKSPACE: &str = "10";
const SIGNATURE: &str = "fakehypr";
const HOST_DISPLAY: &str = "wayland-host";

/// A binary of this workspace. Builds every binary once per test run, the
/// `hypr-computer` command included.
pub fn bin(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let dir = BUILT.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "--workspace", "--bins"])
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success(), "building the workspace's binaries failed");
        target.join("debug")
    });
    let path = dir.join(name);
    assert!(path.exists(), "there is no {} binary", path.display());
    path
}

pub struct Desktop {
    // Dropped in this order: the host first, so views see their desktop go.
    pub host: Host,
    pub hypr: FakeHyprland,
    fake_bin: PathBuf,
    pub env: Env,
}

impl Desktop {
    pub fn new() -> Self {
        let env = Env::new();
        let fake_bin = env.path("fake-bin");
        fs::create_dir(&fake_bin).unwrap();
        // The notification service, faked: records what it was asked to show.
        let log = env.path("notifications.log");
        let script = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display());
        fs::write(fake_bin.join("notify-send"), script).unwrap();
        fs::set_permissions(
            fake_bin.join("notify-send"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let spawn_env = desktop_env(&env, &fake_bin);
        let hypr = FakeHyprland::start(&env.runtime_dir(), SIGNATURE, spawn_env, &env.path(""));
        let host = Host::start(&env.runtime_dir().join(HOST_DISPLAY), hypr.shared());
        Desktop {
            host,
            hypr,
            fake_bin,
            env,
        }
    }

    /// A command run in the user's session: it finds this fake Hyprland and
    /// this host compositor, and nothing else.
    pub fn command(&self, program: &Path) -> Command {
        let mut cmd = self.env.command(program);
        cmd.env_clear()
            .envs(desktop_env(&self.env, &self.fake_bin))
            .env("RUST_BACKTRACE", "1");
        cmd
    }

    pub fn start_daemon(&self) -> Daemon {
        Daemon::spawn(
            &self.env,
            self.command(&support::daemon_bin()),
            "daemon.log",
        )
    }

    /// An agent called `name` (its MCP clientInfo name), with its screen ready.
    pub fn agent(&self, name: &str) -> Shim {
        let mut shim = Shim::start(&self.env);
        let resp = shim.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": name, "version": "1.0.0"}
            }),
        );
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
        shim.notify("notifications/initialized");
        shim.screenshot();
        shim
    }

    /// Runs `hypr-computer <args>` as the user would, in a terminal of this
    /// desktop; fails the test if it runs for more than 10 s.
    pub fn hypr_computer(&self, args: &[&str]) -> Output {
        let mut child = self
            .command(&bin("hypr-computer"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let status = support::wait_exit(&mut child, support::TIMEOUT);
        if status.is_none() {
            let _ = child.kill();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            status.is_some(),
            "`hypr-computer {}` didn't finish in {:?}; it printed:\n{}{}",
            args.join(" "),
            support::TIMEOUT,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// The live views on the host: every window not opened by the test itself.
    pub fn views(&self) -> Vec<HostWindow> {
        let me = std::process::id() as i32;
        self.host
            .windows()
            .into_iter()
            .filter(|w| w.pid != Some(me))
            .collect()
    }

    /// The views that mapped (Hyprland placed them).
    pub fn mapped_views(&self) -> Vec<HostWindow> {
        self.views()
            .into_iter()
            .filter(|w| w.address.is_some())
            .collect()
    }

    /// Waits until exactly `n` views have mapped, and returns them.
    pub fn wait_for_views(&self, n: usize, daemon: &Daemon) -> Vec<HostWindow> {
        let end = Instant::now() + support::TIMEOUT;
        loop {
            let views = self.mapped_views();
            if views.len() == n {
                return views;
            }
            assert!(
                Instant::now() < end,
                "expected {n} live views on the desktop, found {}: {views:?}\n{}",
                views.len(),
                self.report(daemon)
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The text the user may have been shown: notifications, through
    /// Hyprland or the notification service, and the daemon's log.
    pub fn messages(&self, daemon: &Daemon) -> Vec<String> {
        let mut out = self.hypr.world().notifications.clone();
        if let Ok(text) = fs::read_to_string(self.env.path("notifications.log")) {
            out.extend(text.lines().map(str::to_owned));
        }
        out.extend(daemon.log().lines().map(str::to_owned));
        out
    }

    /// Everything that might explain a failure.
    pub fn report(&self, daemon: &Daemon) -> String {
        format!(
            "host windows: {:?}\n{}\ndaemon log:\n{}",
            self.host.windows(),
            self.hypr.report(),
            daemon.log()
        )
    }

    /// The Wayland socket of the agent screen of session `n`.
    pub fn screen(&self, n: u64) -> PathBuf {
        self.env.screen_socket(n)
    }
}

fn desktop_env(env: &Env, fake_bin: &Path) -> Vec<(String, OsString)> {
    let mut path = OsString::from(fake_bin);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    vec![
        ("PATH".to_owned(), path),
        ("HOME".to_owned(), env.path("home").into()),
        ("XDG_RUNTIME_DIR".to_owned(), env.runtime_dir().into()),
        ("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), SIGNATURE.into()),
        ("WAYLAND_DISPLAY".to_owned(), HOST_DISPLAY.into()),
        ("XDG_SESSION_TYPE".to_owned(), "wayland".into()),
        ("XDG_CURRENT_DESKTOP".to_owned(), "Hyprland".into()),
    ]
}

/// Where in the screen the comparisons look: the middle of each quadrant
/// and of each eighth, away from the edges between colours.
pub const SAMPLES: [(f64, f64); 16] = [
    (0.125, 0.125),
    (0.375, 0.125),
    (0.625, 0.125),
    (0.875, 0.125),
    (0.125, 0.375),
    (0.375, 0.375),
    (0.625, 0.375),
    (0.875, 0.375),
    (0.125, 0.625),
    (0.375, 0.625),
    (0.625, 0.625),
    (0.875, 0.625),
    (0.125, 0.875),
    (0.375, 0.875),
    (0.625, 0.875),
    (0.875, 0.875),
];

/// Whether the view shows the screenshot: the same colour at the same place,
/// scaled to the view's size.
pub fn shows(view: &Picture, shot: &Image) -> Result<(), String> {
    let mut wrong = Vec::new();
    for (fx, fy) in SAMPLES {
        let s = shot.pixel(
            (fx * f64::from(shot.width)) as u32,
            (fy * f64::from(shot.height)) as u32,
        );
        let v = view.pixel(
            (fx * f64::from(view.width)) as u32,
            (fy * f64::from(view.height)) as u32,
        );
        if s.iter().zip(v).any(|(a, b)| a.abs_diff(b) > 8) {
            wrong.push(format!("at {fx},{fy} the screen is {s:?}, the view {v:?}"));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the view ({}x{}) doesn't show the screenshot ({}x{}): {}",
            view.width,
            view.height,
            shot.width,
            shot.height,
            wrong.join("; ")
        ))
    }
}

/// Waits up to `timeout` for the view at `address` to show what the agent's
/// screenshot shows now; returns how long it took.
pub fn wait_until_view_shows(
    desk: &Desktop,
    agent: &mut Shim,
    address: &str,
    timeout: Duration,
) -> Result<Duration, String> {
    let start = Instant::now();
    let shot = agent.screenshot();
    loop {
        let result = desk.host.capture(address).and_then(|v| shows(&v, &shot));
        match result {
            Ok(()) => return Ok(start.elapsed()),
            Err(e) if start.elapsed() > timeout => {
                return Err(format!("after {:?}: {e}", start.elapsed()));
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// The numbers in `text`, e.g. `[2]` for "agent 2 (codex)".
pub fn numbers(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `pid` is a live process (not a zombie).
pub fn running(pid: i32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rfind(')')
            .and_then(|i| stat[i + 1..].split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X"),
        Err(_) => false,
    }
}

/// Whether `pid` runs the daemon binary.
pub fn is_daemon(pid: i32) -> bool {
    fs::read_link(format!("/proc/{pid}/exe"))
        .is_ok_and(|exe| exe.file_name() == support::daemon_bin().file_name())
}
