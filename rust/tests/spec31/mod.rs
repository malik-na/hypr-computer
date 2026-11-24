//! Helpers for the black-box tests of #31 ("Limits: allowed apps, read-only,
//! password managers") and #32 ("Limit: allowed websites"), written from the
//! spec (the issues and specs/Limits.tla) by an independent test agent.
//!
//! They drive the real daemon, shim and screen as agents do (MCP through the
//! shim), and the `hypr-computer` command as the user runs it, each test in a
//! throwaway runtime dir and home with a cleared environment, so nothing can
//! reach the desktop of the machine running them. The apps the agents use are
//! copies of `hc-test-app` under other names (`notes`, `paint`, `keepassxc`),
//! started with `launch_app`; each paints its own colours and logs every input
//! it gets.
//!
//! # The interface these tests assume
//!
//! ## `hypr-computer allow`
//!
//! ```text
//! hypr-computer allow APP[,APP...]... [--sites SITE[,SITE...]] [--for DURATION]
//! hypr-computer allow --sites SITE[,SITE...] [--for DURATION]
//! hypr-computer allow --read-only [--for DURATION]
//! hypr-computer allow --list          (or no arguments)
//! hypr-computer allow --off
//! ```
//!
//! - Out of the box there is no limit: agents may use every app and site on
//!   their own screens, but never a password manager (below).
//! - `allow APP...` limits every agent, those working now and those that
//!   connect later, to the apps named. An app is named by its program's file
//!   name (`foot`, not `/usr/bin/foot`), case-insensitively; `browser` names
//!   the agent's browser, the one `open_url` starts. `launch_app` of any other
//!   app is refused, and so is input (click, move, type, key, scroll, drag)
//!   while a window of another app is in front: an app's windows are those of
//!   the process `launch_app` started, and of its children. `open_url` needs
//!   `browser`.
//! - `--sites` limits the agent's browser to the sites listed, by host: a
//!   site includes its subdomains, an IP address only itself; any port, http
//!   or https. Given as URLs, they are reduced to their hosts. See spec32.
//! - `--read-only` lets agents only look: every tool that isn't marked
//!   `readOnlyHint: true` in `tools/list` (of today's, all but `screenshot`)
//!   is refused. It can't be given with apps or sites.
//! - `--for DURATION` (`30m`, `2h`, `45s`, `1h30m`; or `--for=30m`): the
//!   limits stop applying once that time is up, as if `--off` had been run.
//!   Without it they last until `--off`.
//! - Each `allow` replaces the limits before it. It applies to every agent's
//!   next tool call; a call already in progress finishes.
//! - `--list` prints the limits in force: "No limits" when there is none
//!   (also once they have expired), otherwise every app and site, "read-only"
//!   if so, and "until" with the time if they have one.
//! - `--off` removes every limit. Password managers stay blocked.
//! - The limits are kept in the user's runtime dir, so they hold across a
//!   restart of the daemon, and `allow` works when no daemon is running: a
//!   daemon that starts later goes by them. They end with the login session.
//!
//! Exit codes: 0 once done, with a line saying what agents may now do on
//! stdout. 2 for arguments it doesn't take (an unknown option, a bad duration
//! or site, nothing to allow, `--read-only` with apps or sites): it says
//! what is wrong and how to use the command on stderr ("usage"), and changes
//! nothing. 1 for anything else that went wrong, said on stderr.
//!
//! ## What an agent is told
//!
//! Every refusal is a failed tool call (`isError: true`, not a protocol
//! error) whose text tells the agent to ask the user ("Ask the user") and
//! names what was refused: the app (`paint`), the site's host, "read-only",
//! or "password manager". The screen is left alone. `launch_app` of a web
//! browser is refused at all times, saying to use `open_url` (#32).
//!
//! ## Password managers (#31)
//!
//! A password manager or password prompt is never launched and never shown,
//! limits or not: `launch_app` of one is refused, and the screen never shows
//! a window of one, whoever started it; it ends that app, or at least its
//! connection, at once. One is known by its program's file name or its
//! window's Wayland app id (`domain::limits::is_password_manager`: names
//! containing `keepass`, `1password`, `bitwarden`, `pinentry`, `polkit`, ...).
//! A `launch_app` whose window is refused that way fails, saying "password
//! manager" and to ask the user.
//!
//! These helpers are kept apart from `support/` and the other tickets' on
//! purpose: these tests must not depend on helpers that another ticket's work
//! may change.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

/// How long the tests wait for something that should happen. Generous: the
/// machine running them may be loaded.
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// How long a tool call may take; `launch_app` waits up to 10 s for a window.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the tests watch for something that must not happen.
pub const QUIET: Duration = Duration::from_secs(2);
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// An RGB colour.
pub type Rgb = [u8; 3];

pub const GREEN: Rgb = [0x00, 0xaa, 0x00];
pub const BLUE: Rgb = [0x00, 0x00, 0xff];
pub const ORANGE: Rgb = [0xff, 0x80, 0x00];
pub const PURPLE: Rgb = [0x80, 0x00, 0x80];
pub const TEAL: Rgb = [0x00, 0x80, 0x80];
/// The colour every password manager in these tests paints: it must never be
/// seen. White: hc-test-app turns a new shade at each input it gets, and none
/// of its shades comes near white.
pub const SECRET: Rgb = [0xff, 0xff, 0xff];

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
                "cli",
                "-p",
                "workspace-tests",
            ])
            .arg("--manifest-path")
            .arg(workspace.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "building the daemon, shim, screen, CLI and test app failed"
        );
        target.join("debug")
    });
    dir.join(name)
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

/// `s` quoted for sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `RRGGBB`, as hc-test-app takes colours.
pub fn hex([r, g, b]: Rgb) -> String {
    format!("{r:02x}{g:02x}{b:02x}")
}

/// A throwaway home and `$XDG_RUNTIME_DIR` under /tmp.
pub struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc31-")
            .tempdir_in("/tmp")
            .unwrap();
        for sub in ["run", "home", "bin"] {
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
    /// this env's home, runtime dir and a UTF-8 locale. No WAYLAND_DISPLAY:
    /// nothing here can reach a real desktop.
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

    /// Writes an executable sh script at `bin/<name>` and returns its path.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A copy of hc-test-app at `bin/<name>`, an app of that name (its file
    /// name: `name` may have a directory in front).
    pub fn app_program(&self, name: &str) -> PathBuf {
        let path = self.path("bin").join(name);
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(bin("hc-test-app"), &path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// A browser that only notes that it was started (its pid, arguments and
    /// environment) and then waits; for the tests that must see that no
    /// browser starts, or how one is started, without a real one.
    pub fn fake_browser(&self) -> FakeBrowser {
        let dir = self.path("fake-browser");
        fs::create_dir_all(&dir).unwrap();
        let path = self.script(
            "chromium",
            &format!(
                "echo $$ >> {pids}\n\
                 {{ echo '--- launch'; printf '%s\\n' \"$@\"; }} >> {args}\n\
                 env > {dir}/env-$$\n\
                 exec sleep 600",
                pids = quote(&dir.join("pids").to_string_lossy()),
                args = quote(&dir.join("args").to_string_lossy()),
                dir = quote(&dir.to_string_lossy()),
            ),
        );
        FakeBrowser { path, dir }
    }
}

/// A browser that only records how it was started (see [`Env::fake_browser`]).
pub struct FakeBrowser {
    pub path: PathBuf,
    dir: PathBuf,
}

impl FakeBrowser {
    /// The pid of each time it was started.
    pub fn launches(&self) -> Vec<u32> {
        fs::read_to_string(self.dir.join("pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    /// The arguments of each launch, in order.
    pub fn args(&self) -> Vec<Vec<String>> {
        let text = fs::read_to_string(self.dir.join("args")).unwrap_or_default();
        let mut launches: Vec<Vec<String>> = Vec::new();
        for line in text.lines() {
            if line == "--- launch" {
                launches.push(Vec::new());
            } else if let Some(last) = launches.last_mut() {
                last.push(line.to_owned());
            }
        }
        launches
    }

    /// The environment launch `pid` was started with.
    pub fn env_of(&self, pid: u32) -> HashMap<String, String> {
        fs::read_to_string(self.dir.join(format!("env-{pid}")))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }
}

/// The real daemon, its stderr going to a log file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    pub fn start(env: &Env) -> Self {
        Self::start_with(env, &[])
    }

    /// The daemon, with `vars` set in its environment (the browser it
    /// starts, say).
    pub fn start_with(env: &Env, vars: &[(&str, &Path)]) -> Self {
        let n = fs::read_dir(env.path("")).map_or(0, |d| {
            d.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("daemon"))
                .count()
        });
        let log = env.path(&format!("daemon-{n}.log"));
        let mut cmd = env.command(&bin("hypr-computer-daemon"));
        for (k, v) in vars {
            cmd.env(k, v);
        }
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

    /// SIGKILL: the daemon gets no chance to clean up or save anything.
    pub fn kill_9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// What `hypr-computer <args>` printed, and how it exited.
#[derive(Debug)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn success(&self) -> bool {
        self.code == Some(0)
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
    let end = Instant::now() + CALL_TIMEOUT;
    let status = loop {
        if let Some(status) = cli.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= end {
            let _ = cli.kill();
            let _ = cli.wait();
            panic!("hypr-computer {args:?} didn't finish within {CALL_TIMEOUT:?}");
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

/// Runs `hypr-computer <args>`, which must succeed.
#[track_caller]
pub fn user_runs(env: &Env, args: &[&str]) -> Ran {
    let ran = hypr_computer(env, args);
    assert!(
        ran.success(),
        "`hypr-computer {}` failed: {ran:?}",
        args.join(" ")
    );
    ran
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

    pub fn is_still_starting(&self) -> bool {
        self.is_error() && self.text().contains("still starting")
    }

    /// Whether this is a refusal the agent can act on: a failed tool call
    /// (not a protocol error) that tells it to ask the user and names `what`
    /// (case-insensitively).
    pub fn is_refusal(&self, what: &str) -> bool {
        let text = self.text().to_lowercase();
        self.raw.get("error").is_none()
            && self.raw["result"]["isError"] == json!(true)
            && text.contains("ask the user")
            && text.contains(&what.to_lowercase())
    }
}

/// Fails the test unless `reply` is a refusal that tells the agent to ask the
/// user and names `what`.
#[track_caller]
pub fn assert_refused(reply: &Reply, what: &str, call: &str) {
    assert!(
        reply.is_refusal(what),
        "{call} must fail as a tool error that names {what:?} and tells the agent to ask \
         the user; it got {}",
        reply.raw
    );
}

/// The real shim, driven over its stdin and stdout as an agent would.
pub struct Shim {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
    early: HashMap<u64, Value>,
}

impl Shim {
    /// Starts the shim and makes the MCP handshake as an agent called `name`.
    pub fn start(env: &Env, name: &str) -> Self {
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
            early: HashMap::new(),
        };
        let init = shim.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": name, "version": "0"}
            }),
        );
        assert!(init.get("error").is_none(), "initialize failed: {init}");
        shim.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        shim
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Sends a request and returns its id, without waiting for the answer.
    pub fn begin_request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// Waits up to `timeout` for the answer to request `id`.
    pub fn answer(&mut self, id: u64, timeout: Duration) -> Value {
        if let Some(msg) = self.early.remove(&id) {
            return msg;
        }
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("no answer to request {id} within {timeout:?}: {e}"));
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("not JSON from the shim ({e}): {line}"));
            match msg["id"].as_u64() {
                Some(got) if got == id => return msg,
                Some(other) => {
                    self.early.insert(other, msg);
                }
                None => {}
            }
        }
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.begin_request(method, params);
        self.answer(id, CALL_TIMEOUT)
    }

    /// Calls a tool as an agent does, whatever comes back.
    pub fn call(&mut self, name: &str, arguments: Value) -> Reply {
        Reply {
            raw: self.request("tools/call", json!({"name": name, "arguments": arguments})),
        }
    }

    /// Starts a tool call and returns at once; [`Shim::finish`] waits for it.
    pub fn begin(&mut self, name: &str, arguments: Value) -> u64 {
        self.begin_request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    /// The answer to a call started with [`Shim::begin`].
    pub fn finish(&mut self, id: u64, timeout: Duration) -> Reply {
        Reply {
            raw: self.answer(id, timeout),
        }
    }

    /// Calls a tool that must succeed.
    #[track_caller]
    pub fn ok(&mut self, name: &str, arguments: Value) -> Reply {
        let reply = self.call(name, arguments.clone());
        assert!(
            !reply.is_error(),
            "{name} {arguments} failed: {}",
            reply.raw
        );
        reply
    }

    /// The tools `tools/list` gives, as listed.
    pub fn tools(&mut self) -> Vec<Value> {
        let list = self.request("tools/list", json!({}));
        list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list failed: {list}"))
            .clone()
    }

    /// A screenshot call's reply, once the screen is no longer starting.
    pub fn screenshot_once_started(&mut self) -> Reply {
        let end = Instant::now() + TIMEOUT;
        loop {
            let reply = self.call("screenshot", json!({}));
            if !reply.is_still_starting() {
                return reply;
            }
            assert!(
                Instant::now() < end,
                "the screen was still starting after {TIMEOUT:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// A screenshot, which must work.
    #[track_caller]
    pub fn screenshot(&mut self) -> Image {
        let reply = self.screenshot_once_started();
        assert!(!reply.is_error(), "screenshot failed: {}", reply.raw);
        Image::from_result(&reply.raw["result"])
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
            "Image {}x{} (top {:?}, bottom {:?})",
            self.width,
            self.height,
            self.pixel(WIDTH / 2, HEIGHT / 4),
            self.pixel(WIDTH / 2, HEIGHT * 3 / 4),
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

    /// The share of the screen that is colour `c`, sampled.
    pub fn share(&self, c: Rgb) -> f64 {
        let mut n = 0;
        let mut all = 0;
        for y in (0..self.height).step_by(4) {
            for x in (0..self.width).step_by(4) {
                all += 1;
                if close(self.pixel(x, y), c) {
                    n += 1;
                }
            }
        }
        f64::from(n) / f64::from(all.max(1))
    }

    /// Whether any visible part of the screen is colour `c`.
    pub fn has(&self, c: Rgb) -> bool {
        self.share(c) > 0.001
    }

    /// Whether the app painted `top` over `bottom` fills the screen.
    pub fn shows(&self, (top, bottom): (Rgb, Rgb)) -> bool {
        close(self.pixel(WIDTH / 2, HEIGHT / 4), top)
            && close(self.pixel(WIDTH / 2, HEIGHT * 3 / 4), bottom)
    }
}

/// What an app wrote to its log: one JSON object per line.
#[derive(Clone)]
pub struct Log {
    pub path: PathBuf,
}

impl Log {
    /// Every event logged so far; a line still being written is left out.
    pub fn events(&self) -> Vec<Value> {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        let whole = &text[..text.rfind('\n').map_or(0, |end| end + 1)];
        whole
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Whether the app was ever started: it logs from its first moment.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// The input the app got: pointer and keyboard events, not focus.
    pub fn inputs(&self) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| {
                matches!(
                    e["event"].as_str(),
                    Some("motion" | "button" | "scroll" | "key" | "drop")
                )
            })
            .collect()
    }

    /// The pid the app logged once its window was on the screen.
    pub fn pid(&self) -> Option<u32> {
        self.events()
            .iter()
            .find(|e| e["event"] == "ready")
            .and_then(|e| e["pid"].as_u64())
            .and_then(|p| u32::try_from(p).ok())
    }

    pub fn wait_for(&self, cond: impl Fn(&[Value]) -> bool) -> bool {
        wait_until(TIMEOUT, || cond(&self.events()))
    }
}

/// An app an agent started with `launch_app`. It paints `colours` until its
/// first input; then it turns a new shade at each input it gets.
pub struct App {
    pub name: String,
    pub colours: (Rgb, Rgb),
    pub log: Log,
}

/// An agent: its connection and its session number.
pub struct Agent {
    pub shim: Shim,
    pub session: u64,
    pub name: String,
    apps: u64,
}

impl Agent {
    /// `launch_app` of a copy of hc-test-app called `name`, painted `top`
    /// over `bottom`, with `extra` arguments; whatever comes back.
    pub fn try_launch(
        &mut self,
        env: &Env,
        name: &str,
        (top, bottom): (Rgb, Rgb),
        extra: &[&str],
    ) -> (Reply, App) {
        let program = env.app_program(name);
        self.launch_program(env, &program, name, (top, bottom), extra)
    }

    /// `launch_app` of `program` (hc-test-app, or a script that runs it),
    /// painted `top` over `bottom`; whatever comes back.
    pub fn launch_program(
        &mut self,
        env: &Env,
        program: &Path,
        name: &str,
        (top, bottom): (Rgb, Rgb),
        extra: &[&str],
    ) -> (Reply, App) {
        self.apps += 1;
        let log = Log {
            path: env.path(&format!("app-{}-{}-{name}.log", self.session, self.apps)),
        };
        let mut args = vec![
            "--top".to_owned(),
            hex(top),
            "--bottom".to_owned(),
            hex(bottom),
            "--log".to_owned(),
            log.path.display().to_string(),
        ];
        args.extend(extra.iter().map(|a| (*a).to_owned()));
        let reply = self.shim.call(
            "launch_app",
            json!({"app": program.display().to_string(), "args": args}),
        );
        let app = App {
            name: name.to_owned(),
            colours: (top, bottom),
            log,
        };
        (reply, app)
    }

    /// Launches app `name`, which must work, and waits until it fills the
    /// screen.
    #[track_caller]
    pub fn launch(&mut self, env: &Env, name: &str, colours: (Rgb, Rgb)) -> App {
        let (reply, app) = self.try_launch(env, name, colours, &[]);
        assert!(!reply.is_error(), "launch_app {name} failed: {}", reply.raw);
        assert!(
            app.log
                .wait_for(|e| e.iter().any(|e| e["event"] == "ready")),
            "{name} never showed its window: {:?}",
            app.log.events()
        );
        let mut last = None;
        let shown = wait_until(TIMEOUT, || {
            let shot = self.shim.screenshot();
            let ok = shot.shows(colours);
            last = Some(shot);
            ok
        });
        assert!(shown, "{name} isn't in front after launch_app: {last:?}");
        app
    }

    /// Clicks at the centre of the screen, which must work and reach `app`
    /// there.
    #[track_caller]
    pub fn click_reaches(&mut self, app: &App) {
        let before = app.log.inputs().len();
        let reply = self.shim.call("click", json!({"x": 640, "y": 400}));
        assert!(
            !reply.is_error(),
            "{}'s click failed: {}",
            self.who(),
            reply.raw
        );
        let pressed = |inputs: &[Value]| {
            inputs
                .iter()
                .skip(before)
                .any(|e| e["event"] == "button" && e["state"] == "down")
        };
        let reached = wait_until(TIMEOUT, || pressed(&app.log.inputs()));
        assert!(
            reached,
            "{}'s click never reached {}: {:?}",
            self.who(),
            app.name,
            app.log.events()
        );
    }

    pub fn who(&self) -> String {
        format!("agent {:?} (session {})", self.name, self.session)
    }
}

/// The input tools and arguments that make each of them reach the app in
/// front: the calls an app limit and read-only mode must refuse.
pub fn input_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("click", json!({"x": 640, "y": 400})),
        ("move", json!({"x": 300, "y": 300})),
        ("type", json!({"text": "hello"})),
        ("key", json!({"keys": "a"})),
        ("scroll", json!({"x": 640, "y": 400})),
        ("drag", json!({"x1": 100, "y1": 100, "x2": 300, "y2": 300})),
    ]
}

/// Valid arguments for `tool`, whose input schema is `schema`: for the tools
/// of today, ones that do something; for a tool added later, a value of the
/// right type for each argument it requires.
pub fn arguments(env: &Env, tool: &str, schema: &Value) -> Value {
    if let Some((_, args)) = input_calls().into_iter().find(|(t, _)| *t == tool) {
        return args;
    }
    match tool {
        "screenshot" => json!({}),
        "launch_app" => json!({
            "app": env.app_program("notes").display().to_string(),
            "args": ["--log", env.path("launched-anyway.log").display().to_string()],
        }),
        "open_url" => json!({"url": "http://127.0.0.1:9/"}),
        _ => {
            let mut args = Map::new();
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            for name in required.iter().filter_map(Value::as_str) {
                let value = match schema["properties"][name]["type"].as_str() {
                    Some("integer" | "number") => json!(10),
                    Some("boolean") => json!(false),
                    Some("array") => json!([]),
                    Some("object") => json!({}),
                    _ => json!("a"),
                };
                args.insert(name.to_owned(), value);
            }
            Value::Object(args)
        }
    }
}

/// A daemon, and the agents that connect to it in turn. Sessions are
/// numbered from 1 in the order the agents connect.
pub struct World {
    pub daemon: Daemon,
    pub env: Env,
    connected: u64,
}

impl World {
    pub fn new() -> Self {
        let env = Env::new();
        let daemon = Daemon::start(&env);
        World {
            daemon,
            env,
            connected: 0,
        }
    }

    /// A world whose daemon's browser is `browser`.
    pub fn with_browser(browser: impl FnOnce(&Env) -> PathBuf) -> Self {
        let env = Env::new();
        let path = browser(&env);
        let daemon = Daemon::start_with(&env, &[("HYPR_COMPUTER_BROWSER", &path)]);
        World {
            daemon,
            env,
            connected: 0,
        }
    }

    /// An agent called `name`, its screen ready.
    pub fn agent(&mut self, name: &str) -> Agent {
        let mut shim = Shim::start(&self.env, name);
        self.connected += 1;
        shim.screenshot();
        Agent {
            shim,
            session: self.connected,
            name: name.to_owned(),
            apps: 0,
        }
    }

    /// The daemon is killed and started again, as systemd restarts one that
    /// crashed. Its agents are gone; the next one is session 1 again.
    pub fn restart_daemon(&mut self) {
        self.daemon.kill_9();
        self.daemon = Daemon::start(&self.env);
        self.connected = 0;
    }

    /// `hypr-computer <args>`, which must succeed.
    #[track_caller]
    pub fn user_runs(&self, args: &[&str]) -> Ran {
        user_runs(&self.env, args)
    }

    /// `hypr-computer <args>`, whatever happens.
    pub fn hypr_computer(&self, args: &[&str]) -> Ran {
        hypr_computer(&self.env, args)
    }
}
