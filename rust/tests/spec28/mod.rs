//! Helpers for the black-box tests of #28 ("You stop one agent or all"),
//! written from the spec (specs/Stop.tla) by an independent test agent.
//!
//! They drive the real daemon, shim and screen as agents do (MCP through the
//! shim), and the `hypr-computer` command as the user runs it, each test in a
//! throwaway runtime dir with a cleared environment, so nothing can reach
//! the desktop of the machine running them. The app on each agent's screen is
//! the probe of #25's tests, which records every input it gets.
//!
//! The interface these tests assume:
//! - `hypr-computer stop` stops every agent, and every agent that connects
//!   until `hypr-computer resume`; `hypr-computer resume` lets them all go on.
//! - `hypr-computer stop <agent>` and `hypr-computer resume <agent>` pick
//!   agents by name (the name its MCP client gives, as on its live view;
//!   every agent of that name) or by session number (`2`, as on its live
//!   view, "name (session 2)"). An agent that isn't there is an error.
//! - Each command exits 0 once it is done, and non-zero, saying why, if it
//!   couldn't be done. A stop is done once every input in progress has ended.
//! - A stopped agent's tool calls, screenshots included, fail (a tool error,
//!   not a protocol error) with a text that says "Stopped by the user" and
//!   tells it to ask the user. A call in progress when the stop comes ends
//!   the same way, its button or key released.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

#[path = "../spec25/probe.rs"]
pub mod probe;

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

use probe::{BTN_LEFT, Event, Probe};

pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// How late a slow app answers the screen's pings. The screen gives an app
/// a drag, or long text, in parts, each once the app has read the one
/// before (#25), so on a slow app a drag stays in progress for a dozen of
/// these: long enough for the user to stop it halfway.
pub const SLOW: Duration = Duration::from_millis(400);

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

/// A throwaway home and `$XDG_RUNTIME_DIR` under /tmp.
pub struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("hc28-")
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
}

/// The real daemon, its stderr going to a log file.
pub struct Daemon {
    child: Child,
    log: PathBuf,
}

impl Daemon {
    pub fn start(env: &Env) -> Self {
        let log = env.path("daemon.log");
        let child = env
            .command(&bin("hypr-computer-daemon"))
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

    /// The pid of session `n`'s screen process, as the daemon logged it.
    pub fn screen_pid(&self, n: u64) -> u32 {
        let needle = format!("session {n}: starting its screen (pid ");
        let mut log = String::new();
        let ok = wait_until(TIMEOUT, || {
            log = self.log();
            log.contains(&needle)
        });
        assert!(ok, "the daemon never logged {needle:?}:\n{log}");
        let rest = &log[log.find(&needle).unwrap() + needle.len()..];
        rest[..rest.find(')').unwrap()].parse().unwrap()
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

/// What `hypr-computer <args>` printed, and whether it succeeded.
#[derive(Debug)]
pub struct Ran {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
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
        success: status.success(),
        stdout,
        stderr,
    }
}

/// Runs `hypr-computer <args>`, which must succeed.
pub fn user_runs(env: &Env, args: &[&str]) -> Ran {
    let ran = hypr_computer(env, args);
    assert!(
        ran.success,
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

    /// Whether this is the refusal a stopped agent gets: a failed tool call
    /// (not a protocol error) that says it was stopped by the user and tells
    /// the agent to ask the user.
    pub fn is_stopped(&self) -> bool {
        let text = self.text();
        self.raw.get("error").is_none()
            && self.raw["result"]["isError"] == json!(true)
            && text.contains("Stopped by the user")
            && text.to_lowercase().contains("ask the user")
    }

    pub fn is_still_starting(&self) -> bool {
        self.is_error() && self.text().contains("still starting")
    }
}

/// Fails the test unless `reply` is a stopped agent's refusal.
#[track_caller]
pub fn assert_stopped(reply: &Reply, what: &str) {
    assert!(
        reply.is_stopped(),
        "{what}: a stopped agent's call must fail with \"Stopped by the user\" and tell it to \
         ask the user; it got {}",
        reply.raw
    );
}

/// The real shim, driven over its stdin and stdout as an agent would.
pub struct Shim {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
    /// Answers that came while waiting for another one.
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
        self.answer(id, TIMEOUT)
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

    /// The tools `tools/list` gives, with their input schemas.
    pub fn tools(&mut self) -> Vec<(String, Value)> {
        let list = self.request("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array();
        let tools = tools.unwrap_or_else(|| panic!("tools/list failed: {list}"));
        tools
            .iter()
            .map(|t| {
                (
                    t["name"].as_str().unwrap().to_owned(),
                    t["inputSchema"].clone(),
                )
            })
            .collect()
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

    /// The agent goes away cleanly: its end of the pipe closes.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Waits for the shim to exit; whether it did.
    pub fn wait_exit(&mut self) -> bool {
        wait_until(TIMEOUT, || self.child.try_wait().unwrap().is_some())
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
            self.pixel(WIDTH / 2, HEIGHT / 2)
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

/// One agent: its connection, its session number, and the probe on its
/// screen. (Fields drop in this order: the probe first.)
pub struct Agent {
    pub probe: Probe,
    pub shim: Shim,
    pub session: u64,
    pub name: String,
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

    /// An agent called `name` connects. Returns its shim and session number,
    /// without waiting for its screen.
    pub fn connect(&mut self, name: &str) -> (Shim, u64) {
        let shim = Shim::start(&self.env, name);
        self.connected += 1;
        (shim, self.connected)
    }

    /// An agent called `name`, its screen ready, with the probe on it.
    pub fn agent(&mut self, name: &str) -> Agent {
        self.agent_with(name, Duration::ZERO)
    }

    /// Like [`World::agent`], with a slow app on its screen (see [`SLOW`]).
    pub fn slow_agent(&mut self, name: &str) -> Agent {
        self.agent_with(name, SLOW)
    }

    fn agent_with(&mut self, name: &str, pong_delay: Duration) -> Agent {
        let (mut shim, session) = self.connect(name);
        shim.screenshot();
        let probe = Probe::start_slow(&self.env.screen_socket(session), pong_delay);
        assert_eq!((probe.width, probe.height), (WIDTH as i32, HEIGHT as i32));
        Agent {
            probe,
            shim,
            session,
            name: name.to_owned(),
        }
    }

    /// `hypr-computer <args>`, which must succeed.
    pub fn user_runs(&self, args: &[&str]) -> Ran {
        user_runs(&self.env, args)
    }

    /// `hypr-computer <args>`, whatever happens.
    pub fn hypr_computer(&self, args: &[&str]) -> Ran {
        hypr_computer(&self.env, args)
    }
}

impl Agent {
    /// Fails the test unless this agent is stopped: a screenshot and a
    /// click are refused as a stopped agent's, and the click reaches nothing.
    #[track_caller]
    pub fn assert_stopped(&mut self) {
        let mark = self.probe.mark();
        let shot = self.shim.call("screenshot", json!({}));
        assert_stopped(&shot, &format!("{}'s screenshot", self.who()));
        let click = self.shim.call("click", json!({"x": 640, "y": 400}));
        assert_stopped(&click, &format!("{}'s click", self.who()));
        let got: Vec<Event> = self
            .probe
            .settle(mark)
            .into_iter()
            .filter(is_input)
            .collect();
        assert!(
            got.is_empty(),
            "{}'s refused click still reached its screen: {got:?}",
            self.who()
        );
    }

    /// Fails the test unless this agent is served: a screenshot works, and
    /// a click at (`x`, `y`) reaches its app there.
    #[track_caller]
    pub fn assert_serves(&mut self, x: i64, y: i64) {
        let shot = self.shim.call("screenshot", json!({}));
        assert!(
            !shot.is_error(),
            "{} must be served, but its screenshot failed: {}",
            self.who(),
            shot.raw
        );
        Image::from_result(&shot.raw["result"]);
        let mark = self.probe.mark();
        let click = self.shim.call("click", json!({"x": x, "y": y}));
        assert!(
            !click.is_error(),
            "{} must be served, but its click failed: {}",
            self.who(),
            click.raw
        );
        let ok = self.probe.wait_for(mark, |e| {
            e.iter()
                .any(|e| matches!(e, Event::Button { pressed: false, .. }))
        });
        let presses = self.probe.presses_since(mark);
        assert!(
            ok && matches!(presses.as_slice(),
                [(BTN_LEFT, px, py)] if px.floor() as i64 == x && py.floor() as i64 == y),
            "{}'s click at ({x}, {y}) must reach its app as one press there: {:?}",
            self.who(),
            self.probe.since(mark)
        );
    }

    pub fn who(&self) -> String {
        format!("agent {:?} (session {})", self.name, self.session)
    }
}

/// Whether `e` is input an app gets (not focus coming or going).
pub fn is_input(e: &Event) -> bool {
    matches!(
        e,
        Event::Motion { .. }
            | Event::Button { .. }
            | Event::Axis { .. }
            | Event::Wheel { .. }
            | Event::Key { .. }
            | Event::Text(_)
    )
}

/// Valid arguments for `tool`, whose input schema is `schema`: for the
/// tools of today, ones that do something visible; for a tool added later,
/// a value of the right type for each argument it requires.
pub fn arguments(env: &Env, tool: &str, schema: &Value) -> Value {
    match tool {
        "screenshot" => json!({}),
        "click" | "move" | "scroll" => json!({"x": 640, "y": 400}),
        "type" => json!({"text": "hello"}),
        "key" => json!({"keys": "a"}),
        "drag" => json!({"x1": 100, "y1": 100, "x2": 300, "y2": 300}),
        "launch_app" => json!({
            "app": bin("hc-test-app").display().to_string(),
            "args": ["--log", env.path("launched.log").display().to_string()],
        }),
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

/// The motions an app got while its left button was down, and whether the
/// button was down at the end of `events`.
pub fn held_motions(events: &[Event]) -> (Vec<(f64, f64)>, bool) {
    let mut down = false;
    let mut held = Vec::new();
    for e in events {
        match e {
            Event::Button {
                button: BTN_LEFT,
                pressed,
                ..
            } => down = *pressed,
            Event::Motion { x, y } if down => held.push((*x, *y)),
            _ => {}
        }
    }
    (held, down)
}
