//! Helpers for the black-box tests of #33 ("Saved browser profiles"),
//! written from the spec (and specs/Profiles.tla) by an independent test
//! agent.
//!
//! They drive the real daemon, shim and screen as an agent does (MCP through
//! the shim) and the `hypr-computer` command as the user runs it, each test
//! in a throwaway home, runtime dir and data dir with a cleared environment,
//! so nothing can reach the desktop or the data of the machine running them.
//! Pages come from a tiny HTTP server in the test (`web`), which records the
//! cookies the browser sends. The browser is the real Chromium of this
//! machine, through a wrapper (`browser`) that notes each launch. Tests that
//! need it pass with a SKIPPED note where there is none (`HC33_BROWSER=none`).
//!
//! # The interface these tests assume
//!
//! **Fresh by default.** Without a profile, each session's browser starts
//! fresh, as in #26, and the agent can't reach a saved profile.
//!
//! **The user makes profiles; the agent only names them.** Profiles are made
//! and removed only by the user's command:
//!
//! - `hypr-computer profile new <name>` makes an empty profile. Exit 0, and
//!   its output names it. A name taken already: exit 1, and the profile is
//!   left as it was.
//! - `hypr-computer profile list` prints the names, one per line, sorted, and
//!   nothing else on stdout (none: empty). Exit 0.
//! - `hypr-computer profile delete <name>` deletes the profile and all its
//!   files. Exit 0. No such profile: exit 1. In use by an agent's browser:
//!   exit 1, a message that says it is "in use", and nothing deleted.
//! - A name is 1 to 64 ASCII letters, digits, '.', '_' and '-', the first a
//!   letter or a digit (`domain::profiles::ProfileName`). Any other: exit 2,
//!   a message saying what a name may hold (it mentions "letters"), and
//!   nothing made. `profile` without a known subcommand and its one name,
//!   exit 2, as any usage error. `hypr-computer help` lists the commands.
//! - These work whether or not the daemon runs: the user may set profiles up
//!   before any agent starts.
//!
//! **Where they live.** `$XDG_DATA_HOME/hypr-computer/profiles/<name>`, or
//! `~/.local/share/hypr-computer/profiles/<name>` without `$XDG_DATA_HOME`:
//! user data that outlives sessions and daemons, never under the user's own
//! browser's folders (`~/.config/chromium`). The `hypr-computer` and
//! `profiles` folders and every profile are open to the user only (0700),
//! and no file in a profile is open to anyone else (no group or other bits),
//! whoever wrote it. The daemon reads them where the command made them, with
//! the same environment.
//!
//! **The agent names one.** `open_url {url, profile?, private?}`: `profile`
//! is a profile's name (a string, listed in `open_url`'s input schema). The
//! session's browser starts with that profile as its `--user-data-dir`, so
//! its cookies (logins) are the ones the profile kept, and what it sets is
//! kept for the next session that names it. The browser is shut down so
//! that it writes them, whether the agent ends cleanly or is killed (kill -9)
//! — the daemon does the shutting down, not the agent. A session's profile is
//! fixed by its first `open_url`, named or fresh, until it ends: later calls
//! may leave `profile` out (or repeat it); naming another one is refused,
//! saying which profile the session has and that a session uses one.
//! Refused, with an error naming the profile, and nothing started or made:
//! - no profile of that name: the error says the user makes one with
//!   `hypr-computer profile new <name>`, and it doesn't list the profiles
//!   that do exist (the agent uses only names the user gave it);
//! - a name that isn't one, or isn't a string: the error names `profile`.
//!
//! **One session at a time.** While a session has a profile, another
//! session's `open_url` naming it is refused with an error that names it and
//! says it is "in use"; the first session is unaffected, and the refused one
//! may ask again later. The profile is free again once the first session has
//! ended (its browser gone): when its agent leaves, is killed, or its daemon
//! is killed. Of two sessions asking at once, exactly one gets it.
//!
//! **Why this design.** It is the smallest that meets #33 and keeps the agent
//! from picking arbitrary profiles. The agent can't make, list or delete
//! profiles, only use one the user made and told it the name of; an error
//! never shows it the others. Selecting by an `open_url` argument (rather
//! than a setting per agent name) needs no new configuration and matches how
//! the user hands it over: "use my `work` profile". Fixing the profile per
//! session keeps one browser per session, so the lock is held for the
//! session's whole life and released in one place, its teardown.
//!
//! They are kept apart from other tickets' helpers on purpose: these tests
//! must not depend on helpers that another ticket's work may change.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

pub mod browser;
pub mod procs;
pub mod web;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the tests wait for something on the screen or in the system.
pub const TIMEOUT: Duration = Duration::from_secs(30);

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
                "build", "--quiet", "--bins", "-p", "daemon", "-p", "shim", "-p", "screen", "-p",
                "cli",
            ])
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "building the daemon, shim, screen and hypr-computer failed"
        );
        target.join("debug")
    });
    dir.join(name)
}

/// A throwaway home, `$XDG_RUNTIME_DIR` and (if asked for) `$XDG_DATA_HOME`
/// under /tmp.
pub struct Env {
    dir: tempfile::TempDir,
    data_home: Option<PathBuf>,
}

impl Env {
    /// Without `$XDG_DATA_HOME`: profiles go under the home.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc33-")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["run", "home"] {
            let p = dir.path().join(sub);
            fs::create_dir(&p).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let env = Env {
            dir,
            data_home: None,
        };
        if let Some(real) = std::env::var_os("XDG_RUNTIME_DIR") {
            assert_ne!(
                Path::new(&real),
                env.runtime_dir(),
                "never use the real runtime dir"
            );
        }
        if let Some(real) = std::env::var_os("HOME") {
            assert_ne!(Path::new(&real), env.home(), "never use the real home");
        }
        env
    }

    /// With `$XDG_DATA_HOME` set to a folder of its own, outside the home.
    pub fn with_data_home() -> Self {
        let mut env = Self::new();
        let data = env.path("data");
        fs::create_dir(&data).unwrap();
        env.data_home = Some(data);
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

    /// Where the user's data lives: `$XDG_DATA_HOME`, or ~/.local/share.
    pub fn data_home(&self) -> PathBuf {
        self.data_home
            .clone()
            .unwrap_or_else(|| self.home().join(".local/share"))
    }

    /// The folder that holds the profiles.
    pub fn profiles_dir(&self) -> PathBuf {
        self.data_home().join("hypr-computer/profiles")
    }

    /// Profile `name`'s folder.
    pub fn profile(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(name)
    }

    /// A command with nothing from the caller's environment but PATH, and
    /// this env's home, runtime dir, data dir if any, and a UTF-8 locale (as a
    /// login session has). No WAYLAND_DISPLAY: nothing here can reach a real
    /// desktop.
    pub fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("LANG", "C.UTF-8")
            .env("RUST_BACKTRACE", "1");
        if let Some(data) = &self.data_home {
            cmd.env("XDG_DATA_HOME", data);
        }
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

/// What `hypr-computer <args>` printed, and how it exited.
#[derive(Debug)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// The non-empty lines of stdout, trimmed.
    pub fn lines(&self) -> Vec<String> {
        self.stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Everything it printed, lowercased, for messages.
    pub fn said(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr).to_lowercase()
    }
}

/// Runs `hypr-computer <args>` as the user runs it, and returns once it has
/// exited.
pub fn hypr_computer(env: &Env, args: &[&str]) -> Ran {
    let mut cli = env
        .command(&bin("hypr-computer"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = cli.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= end {
            let _ = cli.kill();
            let _ = cli.wait();
            panic!("hypr-computer {args:?} didn't finish within {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    cli.stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    cli.stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    Ran {
        code: status.code(),
        stdout,
        stderr,
    }
}

/// Runs `hypr-computer <args>`, which must exit 0.
pub fn user_runs(env: &Env, args: &[&str]) -> Ran {
    let ran = hypr_computer(env, args);
    assert_eq!(
        ran.code,
        Some(0),
        "`hypr-computer {}` failed: {ran:?}",
        args.join(" ")
    );
    ran
}

/// `hypr-computer profile new <name>`, which must succeed and make it.
pub fn new_profile(env: &Env, name: &str) {
    let ran = user_runs(env, &["profile", "new", name]);
    assert!(
        env.profile(name).is_dir(),
        "`hypr-computer profile new {name}` succeeded ({ran:?}) but there is no {}",
        env.profile(name).display()
    );
}

/// `hypr-computer profile list`, which must succeed: the names it lists.
pub fn listed(env: &Env) -> Vec<String> {
    user_runs(env, &["profile", "list"]).lines()
}

/// Every file and folder under `dir` (not following links), with its mode
/// bits, for checking who may open them. Symbolic links are left out: their
/// own mode means nothing.
pub fn tree_modes(dir: &Path) -> Vec<(PathBuf, u32, bool)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                todo.push(path.clone());
            }
            out.push((path, meta.mode() & 0o7777, meta.is_dir()));
        }
    }
    out
}

/// The mode bits of `path`.
pub fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .mode()
        & 0o7777
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

    /// Starts `cmd` as the daemon and waits until it says it listens. Each
    /// daemon of a test logs to a file of its own.
    pub fn spawn(env: &Env, mut cmd: Command) -> Self {
        let mut n = 0;
        let log = loop {
            let log = env.path(&format!("daemon-{n}.log"));
            if !log.exists() {
                break log;
            }
            n += 1;
        };
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

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Stops the daemon as systemd does (SIGTERM), and waits until it has
    /// exited.
    pub fn stop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let gone = wait_until(TIMEOUT, || !self.is_running());
        assert!(gone, "the daemon didn't exit on SIGTERM:\n{}", self.log());
    }

    /// SIGKILL: the daemon gets no chance to clean up.
    pub fn kill_9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    /// Stops the daemon as systemd would (SIGTERM, so it takes its screens
    /// and apps with it), and kills it only if it won't go.
    fn drop(&mut self) {
        if !self.is_running() {
            return;
        }
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        if !wait_until(Duration::from_secs(10), || !self.is_running()) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// What a tool call came back with, as an agent sees it.
#[derive(Clone, Debug)]
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
                "clientInfo": {"name": "spec33", "version": "0"}
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
        let names: Vec<String> = shim
            .tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect();
        assert!(
            names.iter().any(|n| n == "open_url"),
            "the agent has no open_url tool; it has {names:?}"
        );
        shim.screenshot();
        shim
    }

    /// The tools `tools/list` offers.
    pub fn tools(&mut self) -> Vec<Value> {
        let list = self.request("tools/list", json!({}));
        list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list failed: {list}"))
            .clone()
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

    /// Opens `url` with `open_url`, without a profile; it must succeed.
    pub fn open(&mut self, url: &str) -> Reply {
        self.ok("open_url", json!({"url": url}))
    }

    /// `open_url` naming `profile`, whatever comes back.
    pub fn open_in(&mut self, url: &str, profile: &str) -> Reply {
        self.call("open_url", json!({"url": url, "profile": profile}))
    }

    /// `open_url` naming `profile`, which must succeed.
    pub fn open_in_ok(&mut self, url: &str, profile: &str) -> Reply {
        self.ok("open_url", json!({"url": url, "profile": profile}))
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

/// An RGB colour.
pub type Rgb = [u8; 3];

pub const RED: Rgb = [255, 0, 0];
pub const GREEN: Rgb = [0, 170, 0];
pub const BLUE: Rgb = [0, 0, 255];
pub const ORANGE: Rgb = [255, 128, 0];
pub const MAGENTA: Rgb = [255, 0, 255];
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
            "Image {}x{} (centre {:?})",
            self.width,
            self.height,
            self.pixel(self.width / 2, self.height / 2),
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

    /// The share of the screen that is colour `c`.
    pub fn share(&self, c: Rgb) -> f64 {
        let mut n = 0usize;
        for y in 0..self.height {
            for x in 0..self.width {
                if close(self.pixel(x, y), c) {
                    n += 1;
                }
            }
        }
        n as f64 / (self.width * self.height) as f64
    }

    /// Whether colour `c` covers most of the screen: a page of that colour
    /// is what the agent sees.
    pub fn shows(&self, c: Rgb) -> bool {
        self.share(c) > 0.5
    }
}

/// The cookie a site sets for a signed-in user, and the pages that set and
/// check it. `/login` sets it (for a day); `/check` and `/again` show which
/// cookie came with them.
pub const COOKIE: &str = "hc33=signed-in";

/// A daemon whose browser is this machine's Chromium (through the wrapper),
/// and the test's web server, serving the login pages. No agent yet: tests
/// start them as they need. (Fields drop in this order: the daemon first,
/// the env last.)
pub struct World {
    pub daemon: Option<Daemon>,
    pub web: web::Web,
    pub browser: browser::Wrapper,
    found: bool,
    pub env: Env,
}

impl World {
    pub fn start() -> Self {
        Self::start_in(Env::new())
    }

    pub fn start_in(env: Env) -> Self {
        let real = browser::find();
        let found = real.is_some();
        let real = real.unwrap_or_else(|| PathBuf::from("/nonexistent/chromium"));
        let wrapper = browser::Wrapper::new(&env, &real, &[]);
        let daemon = Daemon::with_browser(&env, wrapper.path());
        let web = web::Web::start();
        web.page(
            "/login",
            web::Page::solid("Signed in", ORANGE, "")
                .header(&format!("Set-Cookie: {COOKIE}; Max-Age=86400; Path=/")),
        );
        web.page("/check", web::Page::solid("Check", BLUE, ""));
        web.page("/again", web::Page::solid("Again", GREEN, ""));
        web.page("/other", web::Page::solid("Other", YELLOW, ""));
        World {
            daemon: Some(daemon),
            web,
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

    /// A new agent of the daemon, its screen ready.
    pub fn agent(&self) -> Shim {
        Shim::ready(&self.env)
    }

    /// The browser's processes running now.
    pub fn browser_procs(&self) -> std::collections::BTreeSet<procs::Proc> {
        procs::browser_procs(self.browser.mark(), &self.browser.launches())
    }

    /// Waits until no browser of this test runs; fails the test if one
    /// still does.
    pub fn wait_browsers_gone(&self, why: &str) {
        let gone = wait_until(TIMEOUT, || self.browser_procs().is_empty());
        assert!(
            gone,
            "{why}: the browser is still running:\n{}",
            procs::describe(&self.browser_procs())
        );
    }

    /// The Cookie header of the last request for `path`.
    pub fn last_cookie(&self, path: &str) -> String {
        let got = self.web.requests_for(path);
        let last = got
            .last()
            .unwrap_or_else(|| panic!("the browser never asked for {path}"));
        last.cookie.clone()
    }

    /// Signs in on the site in profile `name`: `/login` sets the cookie, and
    /// `/check` must then get it.
    pub fn sign_in(&self, agent: &mut Shim, name: &str) {
        agent.open_in_ok(&self.web.url("/login"), name);
        assert!(
            agent.screenshot().shows(ORANGE),
            "the login page isn't on the screen"
        );
        agent.open_in_ok(&self.web.url("/check"), name);
        let cookie = self.last_cookie("/check");
        assert!(
            cookie.contains(COOKIE),
            "within the session the browser should keep its cookie; /check got {cookie:?}"
        );
    }

    /// The daemon, which must be running.
    pub fn daemon(&mut self) -> &mut Daemon {
        self.daemon.as_mut().expect("the daemon was stopped")
    }

    /// Starts a new daemon in place of the old one, which must be gone.
    pub fn restart_daemon(&mut self) {
        if let Some(mut old) = self.daemon.take() {
            assert!(!old.is_running(), "the old daemon still runs");
        }
        self.daemon = Some(Daemon::with_browser(&self.env, self.browser.path()));
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // The daemon first (it takes its screens and browsers with it), then
        // anything of the browser's still running.
        drop(self.daemon.take());
        procs::reap(self.browser.mark());
    }
}
