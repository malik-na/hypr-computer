//! Helpers for the black-box tests of #26 ("Agent opens a web page"),
//! written from the spec alone by an independent test agent.
//!
//! They drive the real daemon, shim and screen as an agent does (MCP through
//! the shim), each test in a throwaway runtime dir and home with a cleared
//! environment, so nothing can reach the desktop of the machine running them.
//! Pages come from a tiny HTTP server in the test (`web`), which also records
//! every request the browser makes. The browser is the real Chromium of this
//! machine, started through a wrapper (`browser`) that notes each launch.
//!
//! The interface they assume, for the implementation to build to:
//! - the tool `open_url`, arguments `{"url": string, "private"?: bool}`; it
//!   accepts only http and https URLs, and returns once the page has loaded
//!   and is on the agent's screen;
//! - the daemon starts the browser named by `$HYPR_COMPUTER_BROWSER` (a path
//!   to a Chromium-family program) when that is set.
//!
//! They are kept apart from `support/` and `spec25/` on purpose: these tests
//! must not depend on helpers that another ticket's work may change.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

pub mod browser;
pub mod procs;
pub mod web;

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

/// How long a tool call may take: the first `open_url` starts a browser.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the tests wait for something on the screen or in the system.
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

/// A throwaway home and `$XDG_RUNTIME_DIR` under /tmp.
pub struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc26-")
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

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
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
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("LANG", "C.UTF-8")
            .env("RUST_BACKTRACE", "1");
        cmd
    }

    /// Writes an executable sh script at `path` and returns it.
    pub fn script_at(&self, path: &Path, body: &str) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_path_buf()
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
        thread::sleep(Duration::from_millis(20));
    }
}

/// The real daemon, its stderr going to a log file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    /// A daemon whose agents' browser is `browser`.
    pub fn with_browser(env: &Env, browser: &Path) -> Self {
        let mut cmd = env.command(&bin("hypr-computer-daemon"));
        cmd.env("HYPR_COMPUTER_BROWSER", browser);
        Self::spawn(env, cmd)
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
                "clientInfo": {"name": "spec26", "version": "0"}
            }),
        );
        assert!(init.get("error").is_none(), "initialize failed: {init}");
        shim.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        shim
    }

    /// An agent whose screen is ready (its first screenshot came back) and
    /// that has the `open_url` tool.
    pub fn ready(env: &Env) -> Self {
        let mut shim = Self::start(env);
        shim.assert_has_open_url();
        shim.screenshot();
        shim
    }

    /// Fails the test unless `tools/list` offers `open_url`.
    pub fn assert_has_open_url(&mut self) {
        let list = self.request("tools/list", json!({}));
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list failed: {list}"))
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(
            names.contains(&"open_url"),
            "the agent has no open_url tool; it has {names:?}"
        );
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
                .recv_timeout(CALL_TIMEOUT)
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

    /// Opens `url` with `open_url`, which must succeed.
    pub fn open(&mut self, url: &str) -> Reply {
        self.ok("open_url", json!({"url": url}))
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

    /// Takes screenshots until one satisfies `cond`, for up to `timeout`;
    /// returns it, or the last one taken.
    pub fn screenshot_until(
        &mut self,
        timeout: Duration,
        mut cond: impl FnMut(&Image) -> bool,
    ) -> (bool, Image) {
        let end = Instant::now() + timeout;
        loop {
            let shot = self.screenshot();
            if cond(&shot) {
                return (true, shot);
            }
            if Instant::now() >= end {
                return (false, shot);
            }
            thread::sleep(Duration::from_millis(100));
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

/// An RGB colour.
pub type Rgb = [u8; 3];

pub const MAGENTA: Rgb = [255, 0, 255];
pub const RED: Rgb = [255, 0, 0];
pub const GREEN: Rgb = [0, 170, 0];
pub const BLUE: Rgb = [0, 0, 255];
pub const ORANGE: Rgb = [255, 128, 0];
pub const CYAN: Rgb = [0, 255, 255];
pub const YELLOW: Rgb = [255, 255, 0];

/// The CSS for `c`.
pub fn css(c: Rgb) -> String {
    format!("rgb({},{},{})", c[0], c[1], c[2])
}

/// Whether `a` is `b`, give or take a little rounding.
pub fn close(a: Rgb, b: Rgb) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

/// A decoded screenshot, as 8-bit RGB.
#[derive(PartialEq, Eq, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    rgb: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Image {}x{} (centre {:?}; most common colour {:?})",
            self.width,
            self.height,
            self.pixel(self.width / 2, self.height / 2),
            self.most_common()
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

    pub fn pixel(&self, x: u32, y: u32) -> Rgb {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    fn pixels(&self) -> impl Iterator<Item = (u32, u32, Rgb)> + '_ {
        (0..self.height).flat_map(move |y| (0..self.width).map(move |x| (x, y, self.pixel(x, y))))
    }

    /// The share of the screen that is colour `c`.
    pub fn share(&self, c: Rgb) -> f64 {
        let n = self.pixels().filter(|&(_, _, p)| close(p, c)).count();
        n as f64 / (self.width * self.height) as f64
    }

    /// The share of the box `(left, top, right, bottom)` that is colour `c`.
    pub fn share_in(&self, (l, t, r, b): (u32, u32, u32, u32), c: Rgb) -> f64 {
        let n = (t..=b)
            .flat_map(|y| (l..=r).map(move |x| (x, y)))
            .filter(|&(x, y)| close(self.pixel(x, y), c))
            .count();
        n as f64 / ((r - l + 1) * (b - t + 1)) as f64
    }

    /// Whether colour `c` covers most of the screen: a page of that colour
    /// is what the agent sees.
    pub fn shows(&self, c: Rgb) -> bool {
        self.share(c) > 0.5
    }

    /// The box `(left, top, right, bottom)`, inclusive, around every pixel
    /// of colour `c`, if there is one.
    pub fn bbox(&self, c: Rgb) -> Option<(u32, u32, u32, u32)> {
        self.bbox_where(|_, _, p| close(p, c))
    }

    /// The box around every pixel that differs from `other`, outside `skip`.
    pub fn changed_box(
        &self,
        other: &Image,
        skip: (u32, u32, u32, u32),
    ) -> Option<(u32, u32, u32, u32)> {
        let (l, t, r, b) = skip;
        self.bbox_where(|x, y, p| {
            let inside = x >= l && x <= r && y >= t && y <= b;
            !inside && !close(p, other.pixel(x, y))
        })
    }

    /// How many pixels differ from `other`.
    pub fn changed_pixels(&self, other: &Image) -> usize {
        self.pixels()
            .filter(|&(x, y, p)| !close(p, other.pixel(x, y)))
            .count()
    }

    fn bbox_where(&self, f: impl Fn(u32, u32, Rgb) -> bool) -> Option<(u32, u32, u32, u32)> {
        let mut found: Option<(u32, u32, u32, u32)> = None;
        for (x, y, p) in self.pixels() {
            if f(x, y, p) {
                found = Some(match found {
                    None => (x, y, x, y),
                    Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(x), b.max(y)),
                });
            }
        }
        found
    }

    /// The most common colour, for messages.
    pub fn most_common(&self) -> Rgb {
        let mut counts = std::collections::HashMap::new();
        for (_, _, p) in self.pixels().step_by(7) {
            *counts.entry(p).or_insert(0) += 1;
        }
        counts
            .into_iter()
            .max_by_key(|&(_, n)| n)
            .map(|(p, _)| p)
            .unwrap_or_default()
    }
}

/// The centre of a box.
pub fn centre((l, t, r, b): (u32, u32, u32, u32)) -> (u32, u32) {
    ((l + r) / 2, (t + b) / 2)
}

/// A daemon whose browser is this machine's Chromium (through the wrapper),
/// the test's web server, and one agent whose screen is ready and that has
/// `open_url`. (Fields drop in this order: the agent first, the env last.)
pub struct Agent {
    pub shim: Shim,
    pub daemon: Daemon,
    pub web: web::Web,
    pub browser: browser::Wrapper,
    found: bool,
    pub env: Env,
}

impl Agent {
    pub fn start() -> Self {
        Self::start_with(&[])
    }

    /// As `start`, with `extra` arguments that the system's browser command
    /// adds before the ones the daemon gives it.
    pub fn start_with(extra: &[String]) -> Self {
        let env = Env::new();
        let real = browser::find();
        let found = real.is_some();
        let real = real.unwrap_or_else(|| PathBuf::from("/nonexistent/chromium"));
        let wrapper = browser::Wrapper::new(&env, &real, extra);
        let daemon = Daemon::with_browser(&env, wrapper.path());
        let shim = Shim::ready(&env);
        Agent {
            shim,
            daemon,
            web: web::Web::start(),
            browser: wrapper,
            found,
            env,
        }
    }

    /// Whether this machine has a browser; if not, says the test is
    /// skipped.
    pub fn has_browser(&self) -> bool {
        if !self.found {
            eprintln!("SKIPPED: no Chromium-family browser is installed on this machine");
        }
        self.found
    }

    /// The browser's processes running now.
    pub fn browser_procs(&self) -> std::collections::BTreeSet<procs::Proc> {
        procs::browser_procs(self.browser.mark(), &self.browser.launches())
    }

    /// A second agent of the same daemon.
    pub fn another(&self) -> Shim {
        let mut shim = Shim::start(&self.env);
        shim.screenshot();
        shim
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        procs::reap(self.browser.mark());
    }
}
