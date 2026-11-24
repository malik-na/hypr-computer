//! Helpers for the black-box tests of #25 ("Agent uses an app on its
//! screen"), written from the spec alone by an independent test agent.
//!
//! They drive the real daemon, shim and screen as an agent does (MCP through
//! the shim), each test in a throwaway runtime dir with a cleared
//! environment, so nothing can reach the desktop of the machine running them.
//! They are kept apart from `support/` on purpose: these tests must not
//! depend on helpers that the implementation of #25 may change.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

pub mod probe;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// Path to a binary of this workspace, built once per test run.
pub fn bin(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let dir = BUILT.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let status = Command::new(env!("CARGO"))
            .args([
                "build", "--quiet", "--bins", "-p", "daemon", "-p", "shim", "-p", "screen",
            ])
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "building the daemon, shim and screen failed"
        );
        target.join("debug")
    });
    dir.join(name)
}

/// Whether `program` is on PATH: real-app tests need it installed.
pub fn installed(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// A throwaway home and `$XDG_RUNTIME_DIR` under /tmp.
pub struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc25-")
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

    /// The Wayland socket of session `n`'s screen, where apps connect.
    pub fn screen_socket(&self, n: u64) -> PathBuf {
        self.runtime_dir()
            .join("hypr-computer")
            .join(format!("screen-{n}"))
            .join("wayland")
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A command with nothing from the caller's environment but PATH, and
    /// this env's home, runtime dir and a UTF-8 locale (as a login session
    /// has).
    pub fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path("home"))
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("LANG", "C.UTF-8")
            .env("RUST_BACKTRACE", "1");
        cmd
    }

    /// Writes an executable sh script named `name` and returns its path.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

/// Waits until `cond` holds, or `timeout` passes; returns whether it held.
pub fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Whether process `pid` is still running (a zombie has ended).
pub fn running(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let state = stat.rsplit(')').next().unwrap_or("").trim_start();
            !state.starts_with('Z') && !state.starts_with('X')
        }
        Err(_) => false,
    }
}

pub fn cmdline(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

/// Reads `file` once it has a line for each of `n` pids.
pub fn read_pids(file: &Path, n: usize) -> Vec<u32> {
    let mut pids = Vec::new();
    let ok = wait_until(TIMEOUT, || {
        pids = fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        pids.len() >= n
    });
    assert!(ok, "{} never listed {n} pids: {pids:?}", file.display());
    pids
}

/// The real daemon, its stderr going to a log file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    pub fn start(env: &Env) -> Self {
        Self::spawn(env, env.command(&bin("hypr-computer-daemon")))
    }

    /// Starts `cmd` as the daemon and waits until it says it listens.
    pub fn spawn(env: &Env, mut cmd: Command) -> Self {
        let log = env.path("daemon.log");
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let mut daemon = Daemon { child, log };
        let ok = wait_until(TIMEOUT, || {
            daemon.log().contains("listening on") || !daemon.is_running()
        });
        assert!(
            ok && daemon.is_running(),
            "the daemon didn't start:\n{}",
            daemon.log()
        );
        daemon
    }

    /// A daemon whose screens are held back until the file `go` exists.
    pub fn with_screen_held_back(env: &Env, go: &Path) -> Self {
        let screen = env.script(
            "held-back-screen.sh",
            &format!(
                "for i in $(seq 3000); do [ -e '{}' ] && exec '{}' \"$@\"; sleep 0.01; done; exit 1",
                go.display(),
                bin("hypr-computer-screen").display()
            ),
        );
        let mut cmd = env.command(&bin("hypr-computer-daemon"));
        cmd.env("HYPR_COMPUTER_SCREEN", screen);
        Self::spawn(env, cmd)
    }

    pub fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Daemon {
    /// Stops the daemon as systemd would (SIGTERM, so it takes its screens
    /// and apps with it), and kills it only if it won't go.
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        if !wait_until(Duration::from_secs(5), || !self.is_running()) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Kills, when dropped, whichever of these processes a test started and
/// that are still running, so a failing test leaves nothing behind.
pub struct Reap(pub Vec<u32>);

impl Drop for Reap {
    fn drop(&mut self) {
        for pid in &self.0 {
            let line = cmdline(*pid);
            if running(*pid) && ["sleep", "foot", "sh"].iter().any(|p| line.contains(p)) {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}

/// What a tool call came back with, as an agent sees it.
pub struct Reply {
    /// The whole JSON-RPC response.
    pub raw: Value,
}

impl Reply {
    /// Whether the call failed: a protocol error or a tool error.
    pub fn is_error(&self) -> bool {
        self.raw.get("error").is_some() || self.raw["result"]["isError"] == json!(true)
    }

    /// Every piece of text the agent was given.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(msg) = self.raw["error"]["message"].as_str() {
            text.push_str(msg);
        }
        if let Some(content) = self.raw["result"]["content"].as_array() {
            for block in content {
                if let Some(t) = block["text"].as_str() {
                    text.push('\n');
                    text.push_str(t);
                }
            }
        }
        text
    }
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
            .command(&bin("hypr-computer-shim"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
        let mut shim = Shim {
            stdin: child.stdin.take(),
            child,
            lines,
            next_id: 0,
        };
        let init = shim.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "spec25", "version": "0"}
            }),
        );
        assert!(init.get("error").is_none(), "initialize failed: {init}");
        shim.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        shim
    }

    /// An agent whose screen is ready (its first screenshot came back).
    pub fn ready(env: &Env) -> Self {
        let mut shim = Self::start(env);
        shim.screenshot();
        shim
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|e| panic!("no answer to {method} {params}: {e}"));
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("not JSON from the shim ({e}): {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    /// Calls a tool as an agent does, whatever comes back.
    pub fn call(&mut self, name: &str, arguments: Value) -> Reply {
        Reply {
            raw: self.request("tools/call", json!({"name": name, "arguments": arguments})),
        }
    }

    /// Calls a tool that must succeed.
    pub fn ok(&mut self, name: &str, arguments: Value) -> Reply {
        let reply = self.call(name, arguments.clone());
        assert!(
            !reply.is_error(),
            "{name} {arguments} failed: {}",
            reply.raw
        );
        reply
    }

    /// A screenshot, waiting while the screen is still starting.
    pub fn screenshot(&mut self) -> Image {
        let end = Instant::now() + TIMEOUT;
        loop {
            let reply = self.call("screenshot", json!({}));
            if !reply.is_error() {
                return Image::from_result(&reply.raw["result"]);
            }
            assert!(
                reply.text().contains("still starting") && Instant::now() < end,
                "screenshot failed: {}",
                reply.raw
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The agent goes away cleanly: its end of the pipe closes.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// SIGKILL: the agent's side gets no chance to clean up.
    pub fn kill_9(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A decoded screenshot, as 8-bit RGB.
#[derive(PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    rgb: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Image {}x{} (centre {:?})",
            self.width,
            self.height,
            self.pixel(640, 400)
        )
    }
}

impl Image {
    pub fn from_result(result: &Value) -> Self {
        let content = result["content"].as_array().expect("content");
        let block = content
            .iter()
            .find(|b| b["type"] == "image")
            .unwrap_or_else(|| panic!("no image in {result}"));
        assert_eq!(block["mimeType"], "image/png", "{block}");
        let png = STANDARD.decode(block["data"].as_str().unwrap()).unwrap();
        let mut reader = png::Decoder::new(png.as_slice()).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        let rgb = match info.color_type {
            png::ColorType::Rgb => buf,
            png::ColorType::Rgba => buf
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|[r, g, b, _]| [*r, *g, *b])
                .collect(),
            other => panic!("unexpected PNG colour type {other:?}"),
        };
        Image {
            width: info.width,
            height: info.height,
            rgb,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// A daemon, one agent whose screen is ready, and the probe on that screen.
/// (Fields drop in this order: the probe first, the env last.)
pub struct Agent {
    pub probe: probe::Probe,
    pub shim: Shim,
    pub daemon: Daemon,
    pub env: Env,
}

impl Agent {
    pub fn with_probe() -> Self {
        let env = Env::new();
        let daemon = Daemon::start(&env);
        let shim = Shim::ready(&env);
        let probe = probe::Probe::start(&env.screen_socket(1));
        assert_eq!((probe.width, probe.height), (WIDTH as i32, HEIGHT as i32));
        Agent {
            probe,
            shim,
            daemon,
            env,
        }
    }
}

/// Takes a screenshot right after `what` and checks it differs from
/// `before`; returns it.
pub fn assert_changed(shim: &mut Shim, before: &Image, what: &str) -> Image {
    let shot = shim.screenshot();
    if shot != *before {
        return shot;
    }
    thread::sleep(Duration::from_secs(1));
    let later = shim.screenshot();
    panic!(
        "the screenshot right after {what} was the same as the one before it; a second later it {}",
        if later != *before {
            "had changed (so the call returned before the app drew)"
        } else {
            "still hadn't changed"
        }
    );
}

/// Whether `e` is input an app gets (not focus coming or going).
pub fn is_input(e: &probe::Event) -> bool {
    use probe::Event::*;
    matches!(
        e,
        Motion { .. } | Button { .. } | Axis { .. } | Wheel { .. } | Key { .. } | Text(_)
    )
}

/// Whether `text` has `word` in it, not as part of a longer word.
pub fn mentions(text: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// Starts foot on the agent's screen through `launch_app`, running `script`
/// in sh. Returns `None` (and says so) if foot isn't installed here.
pub fn launch_foot(shim: &mut Shim, script: &str) -> Option<Reply> {
    if !installed("foot") {
        eprintln!("SKIPPED: foot isn't installed on this machine");
        return None;
    }
    Some(shim.ok(
        "launch_app",
        json!({"app": "foot", "args": ["sh", "-c", script]}),
    ))
}

/// Waits for `file` to hold `expected`, and returns what it holds.
pub fn wait_for_file(file: &Path, expected: &str) -> String {
    let mut text = String::new();
    wait_until(TIMEOUT, || {
        text = fs::read_to_string(file).unwrap_or_default();
        text == expected
    });
    text
}
