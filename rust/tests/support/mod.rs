//! Helpers for the black-box tests: start the real daemon and shim binaries
//! in a throwaway runtime dir, with a cleared environment, so a test can never
//! reach the desktop or the runtime dir of the machine running it.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

pub mod app;
pub mod hyprland;
mod image;

pub use image::Image;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Path to a binary of this workspace, built once per test run.
pub fn bin(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let dir = BUILT.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .expect("CARGO_TARGET_TMPDIR is inside the target dir")
            .to_path_buf();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "--bins",
                "-p",
                "daemon",
                "-p",
                "shim",
                "-p",
                "screen",
                "-p",
                "workspace-tests",
            ])
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("run cargo build");
        assert!(
            status.success(),
            "building the daemon, shim, screen and test app failed"
        );
        target.join("debug")
    });
    dir.join(name)
}

pub fn daemon_bin() -> PathBuf {
    bin("hypr-computer-daemon")
}

pub fn shim_bin() -> PathBuf {
    bin("hypr-computer-shim")
}

/// A throwaway home and `$XDG_RUNTIME_DIR` under /tmp (short, so the socket
/// path fits in a sockaddr on every OS).
pub struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc-")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["run", "home"] {
            let p = dir.path().join(sub);
            fs::create_dir(&p).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let env = Env { dir };
        if let Some(real) = std::env::var_os("XDG_RUNTIME_DIR") {
            assert_ne!(
                Path::new(&real),
                env.runtime_dir(),
                "never use the real runtime dir"
            );
        }
        env
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    pub fn daemon_dir(&self) -> PathBuf {
        self.runtime_dir().join("hypr-computer")
    }

    pub fn socket(&self) -> PathBuf {
        self.daemon_dir().join("daemon.sock")
    }

    /// Where session `n`'s screen keeps its files.
    pub fn screen_dir(&self, n: u64) -> PathBuf {
        self.daemon_dir().join(format!("screen-{n}"))
    }

    /// The Wayland socket of session `n`'s screen, for apps.
    pub fn screen_socket(&self, n: u64) -> PathBuf {
        self.screen_dir(n).join("wayland")
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A command with nothing from the caller's environment but PATH, and
    /// this env's home and runtime dir.
    pub fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("RUST_BACKTRACE", "1");
        cmd
    }
}

pub fn wait_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

/// Whether process `pid` exists, zombies included.
pub fn alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Sends SIGKILL to `pid`.
pub fn kill_9(pid: u32) {
    let ok = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(ok, "kill -KILL {pid} failed");
}

/// The files in `dir`, or `None` if it doesn't exist.
pub fn files_in(dir: &Path) -> Option<Vec<String>> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
    )
}

/// The real daemon, its stderr going to a log file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    pub fn start(env: &Env) -> Self {
        Self::spawn(env, env.command(&daemon_bin()), "daemon.log")
    }

    /// Starts `cmd` as the daemon and waits until it accepts connections.
    pub fn spawn(env: &Env, mut cmd: Command, log_name: &str) -> Self {
        let log = env.path(log_name);
        let file = fs::File::create(&log).unwrap();
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(file)
            .spawn()
            .unwrap();
        let mut daemon = Daemon { child, log };
        daemon.wait_until_listening();
        daemon
    }

    /// Waits for the daemon to say it listens. (Connecting to check would
    /// itself open a session.)
    fn wait_until_listening(&mut self) {
        let end = Instant::now() + TIMEOUT;
        while Instant::now() < end {
            if self.log().contains("listening on") {
                return;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "the daemon exited ({status}) before listening:\n{}",
                    self.log()
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the daemon didn't start listening:\n{}", self.log());
    }

    pub fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Waits until the daemon's log has a line containing `needle`.
    pub fn wait_for_log(&self, needle: &str) -> String {
        let end = Instant::now() + TIMEOUT;
        while Instant::now() < end {
            let log = self.log();
            if log.contains(needle) {
                return log;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "the daemon never logged {needle:?}; its log:\n{}",
            self.log()
        );
    }

    /// The pid of session `n`'s screen process, as the daemon logged it.
    pub fn screen_pid(&self, n: u64) -> u32 {
        let needle = format!("session {n}: starting its screen (pid ");
        let log = self.wait_for_log(&needle);
        let rest = &log[log.find(&needle).unwrap() + needle.len()..];
        rest[..rest.find(')').unwrap()].parse().unwrap()
    }

    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// Kills the daemon with SIGKILL, so it gets no chance to clean up, and
    /// waits for it to be gone.
    pub fn kill_9(&mut self) {
        kill_9(self.child.id());
        wait_exit(&mut self.child, TIMEOUT)
            .unwrap_or_else(|| panic!("the daemon survived SIGKILL:\n{}", self.log()));
    }

    pub fn terminate(&mut self) -> ExitStatus {
        let ok = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap()
            .success();
        assert!(ok, "kill -TERM failed");
        wait_exit(&mut self.child, TIMEOUT)
            .unwrap_or_else(|| panic!("the daemon ignored SIGTERM:\n{}", self.log()))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An agent whose screen is ready.
pub fn agent(env: &Env) -> Shim {
    let mut shim = Shim::start(env);
    shim.initialize();
    shim.screenshot();
    shim
}

/// The text of a tool's result.
pub fn text(result: &Value) -> String {
    let content = result["content"].as_array();
    let content = content.unwrap_or_else(|| panic!("no content: {result}"));
    content
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The real shim, driven over its stdin and stdout as an agent would.
pub struct Shim {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Shim {
    pub fn start(env: &Env) -> Self {
        let mut child = env
            .command(&shim_bin())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Shim {
            stdin: child.stdin.take(),
            child,
            lines,
            next_id: 0,
        }
    }

    pub fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Sends a request and returns the whole response.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|e| panic!("no answer to {method}: {e}"));
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("not JSON from the shim ({e}): {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    /// The names `tools/list` gives.
    pub fn tool_names(&mut self) -> Vec<String> {
        let list = self.request("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array();
        let tools = tools.unwrap_or_else(|| panic!("tools/list failed: {list}"));
        tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Calls a tool and returns its result; fails the test on a protocol error.
    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        let resp = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        assert!(resp.get("error").is_none(), "{name} failed: {resp}");
        resp["result"].clone()
    }

    /// Takes a screenshot as an agent would, waiting while the daemon says
    /// the screen is still starting.
    pub fn screenshot(&mut self) -> Image {
        let end = Instant::now() + TIMEOUT;
        loop {
            let result = self.call("screenshot", json!({}));
            if result["isError"] != json!(true) {
                return Image::from_result(&result);
            }
            assert!(
                result.to_string().contains("still starting") && Instant::now() < end,
                "screenshot failed: {result}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    /// The MCP handshake; returns the `initialize` result.
    pub fn initialize(&mut self) -> Value {
        let resp = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "workspace-tests", "version": "0"}
            }),
        );
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
        self.notify("notifications/initialized");
        resp["result"].clone()
    }

    pub fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    pub fn wait_exit(&mut self) -> ExitStatus {
        wait_exit(&mut self.child, TIMEOUT).expect("the shim didn't exit")
    }

    pub fn stderr(&mut self) -> String {
        let mut s = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            e.read_to_string(&mut s).unwrap();
        }
        s
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
