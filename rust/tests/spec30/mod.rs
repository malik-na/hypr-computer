//! Helpers for the black-box tests of #30 ("You take over an agent's screen
//! and hand back"), written from the spec (specs/TakeOver.tla) by an
//! independent test agent.
//!
//! They put the real daemon, shim and screens on the live-view tests' fake
//! desktop (`view_support`: a host compositor and a fake Hyprland, in a
//! throwaway runtime dir), drive agents as MCP clients through the shim, and
//! play the user on the host: clicks and keys in a view, their clipboard,
//! and the `hypr-computer` command. The app on an agent's screen is the
//! probe of #25's tests, which records every input it gets.
//!
//! # The interface these tests assume
//!
//! **Taking over.** Live views are watch-only: pointer motion, scrolling and
//! keys in a view reach nothing. A click (a pointer button pressed) in an
//! agent's view takes over its screen. That click is given to no one: the
//! agent's app gets neither its press nor its release. From then on the
//! user's pointer and keys in that view go to the agent's screen, at the same
//! place in the screen as in the view (the view shows the whole screen,
//! scaled to fit, its shape kept). One screen at a time: a click in another
//! agent's view hands the first back and takes the other over.
//!
//! **The banner.** While a screen is taken over its view shows a banner
//! (anything plainly visible: here, at least 1000 pixels of the view differ
//! from the screen). It is drawn on the view only: the agent's screenshots
//! never show it. It leaves the screen where it is in the view, and covers
//! at most its edges: the tests click at least 100 pixels in from them.
//!
//! **What the agent sees.** Every tool call of the agent's made while its
//! screen is taken over, screenshots included, waits: no answer, and nothing
//! reaches the screen, until the user hands back. Then it runs, once, and its
//! result is the tool's usual result (not an error) with a text block that
//! says the user "took over" its screen, so it knows the screen may have
//! changed. A call already giving input when the user clicks in (a drag, long
//! text) finishes first: the user's input reaches the screen only after it.
//!
//! **Handing back.** Escape pressed twice in a row in the view hands back:
//! the first Escape goes to the agent's app, the second to no one. Any other
//! key, or a click, between the two starts the count again. `hypr-computer
//! give` hands back too: it exits 0 and its output names the agent. Run when
//! no screen is taken over, it exits 1 and says why on stderr. After a hand
//! back the view is watch-only again.
//!
//! **`hypr-computer take <agent>`** takes over as a click does. `<agent>`
//! is the name the agent's MCP client gave (as on its view) or its session
//! number (`2`, as in "name (session 2)"). It must pick exactly one agent:
//! an unknown name, or a name that several agents have, exits 1 and takes
//! nothing over. With no `<agent>` it exits 2, as any usage error.
//!
//! **The clipboard.** Each agent's screen has a clipboard of its own. During
//! a take-over it is joined with the user's: when the take-over starts the
//! screen gets the user's clipboard, what the user copies on their desktop
//! then can be pasted on the agent's screen, and what is copied on the
//! agent's screen can be pasted on the desktop. Outside a take-over they are
//! apart: an app on the agent's screen never gets what the user copied
//! (before the take-over, or after it, including what they copied during
//! it), and nothing the agent's apps copy reaches the user's clipboard, not
//! even when a take-over starts. Handing back, or the session ending, leaves
//! the user's own clipboard as it was.
//!
//! **Ending.** An agent that disconnects during a take-over ends its session
//! as any other: its view and screen go, and the take-over with them, so
//! `hypr-computer give` has nothing to hand back and the user can take over
//! another agent.
//!
//! **The hook the tests use.** The user's input comes from the host
//! compositor (`view_support::host`): `Host::click`, `Host::keys`,
//! `Host::hover`, and `Host::copy`/`Host::paste` for their clipboard. To
//! receive it, a view must take the host's seat: its pointer, its keyboard
//! (given to the view when the user clicks it, as Hyprland does) and its
//! data device (which the host gives the window with the keyboard). Nothing
//! else is needed from the implementation. The user's click and the agent's
//! next call race, as on a real desktop; where a test needs the click to
//! win, it waits until the user's pointer motion reaches the screen
//! ([`World::take_over`]).

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

#[path = "../spec25/probe.rs"]
pub mod probe;

pub mod clip;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::support::{self, Daemon, Image};
use crate::view_support::host::{BTN_LEFT, Picture};
use crate::view_support::{Desktop, VIEWS_WORKSPACE};
use probe::{Event, Probe};

pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// How long a call must go unanswered to count as waiting.
pub const WAITS: Duration = Duration::from_millis(1500);

/// How many pixels of a view must differ from the screen to be a banner.
pub const BANNER_PIXELS: usize = 1000;

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

    /// Whether the agent is told the user took over its screen.
    pub fn says_taken_over(&self) -> bool {
        self.text().to_lowercase().contains("took over")
    }

    pub fn is_still_starting(&self) -> bool {
        self.is_error() && self.text().contains("still starting")
    }

    /// The screenshot in it (a result may also have text).
    pub fn image(&self) -> Image {
        let content = self.raw["result"]["content"].as_array();
        let block = content
            .and_then(|c| c.iter().find(|b| b["type"] == "image"))
            .unwrap_or_else(|| panic!("no image in {}", self.raw));
        assert_eq!(block["mimeType"], "image/png", "{block}");
        let png = STANDARD
            .decode(block["data"].as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("the image isn't base64: {e}"));
        Image::from_png(&png)
    }
}

/// An agent: the real shim, driven as an MCP client would, and the probe on
/// its screen once it has one.
pub struct Agent {
    // Dropped first: the probe, then the shim.
    pub probe: Option<Probe>,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
    early: HashMap<u64, Value>,
    pub session: u64,
    pub name: String,
}

impl Agent {
    fn connect(desk: &Desktop, name: &str, session: u64) -> Agent {
        let mut child = desk
            .command(&support::shim_bin())
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
        let mut agent = Agent {
            probe: None,
            stdin: child.stdin.take(),
            child,
            lines,
            next_id: 0,
            early: HashMap::new(),
            session,
            name: name.to_owned(),
        };
        let id = agent.begin_request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": name, "version": "1.0.0"}
            }),
        );
        let init = agent.answer(id, TIMEOUT);
        assert!(init.get("error").is_none(), "initialize failed: {init}");
        agent.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        agent
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("the agent has gone");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn begin_request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// The answer to request `id`, if it comes within `timeout`.
    fn try_answer(&mut self, id: u64, timeout: Duration) -> Option<Value> {
        if let Some(msg) = self.early.remove(&id) {
            return Some(msg);
        }
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).ok()?;
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("not JSON from the shim ({e}): {line}"));
            match msg["id"].as_u64() {
                Some(got) if got == id => return Some(msg),
                Some(other) => {
                    self.early.insert(other, msg);
                }
                None => {}
            }
        }
    }

    fn answer(&mut self, id: u64, timeout: Duration) -> Value {
        self.try_answer(id, timeout)
            .unwrap_or_else(|| panic!("no answer to request {id} within {timeout:?}"))
    }

    /// Starts a tool call and returns at once; [`Agent::finish`] waits for it.
    pub fn begin(&mut self, tool: &str, arguments: Value) -> u64 {
        self.begin_request("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    /// The answer to a call started with [`Agent::begin`].
    pub fn finish(&mut self, id: u64, timeout: Duration) -> Reply {
        Reply {
            raw: self.answer(id, timeout),
        }
    }

    /// Fails the test if call `id` is answered within `time`: it must wait.
    #[track_caller]
    pub fn assert_waits(&mut self, id: u64, time: Duration, what: &str) {
        if let Some(raw) = self.try_answer(id, time) {
            panic!(
                "{what}: the agent's call must wait while the user has its screen, \
                 but it was answered: {raw}"
            );
        }
    }

    /// Calls a tool, whatever comes back.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Reply {
        let id = self.begin(tool, arguments);
        self.finish(id, TIMEOUT)
    }

    /// Calls a tool that must succeed.
    #[track_caller]
    pub fn ok(&mut self, tool: &str, arguments: Value) -> Reply {
        let reply = self.call(tool, arguments.clone());
        assert!(
            !reply.is_error(),
            "{tool} {arguments} failed: {}",
            reply.raw
        );
        reply
    }

    /// A screenshot, once the screen has started.
    #[track_caller]
    pub fn screenshot(&mut self) -> Image {
        let end = Instant::now() + TIMEOUT;
        loop {
            let reply = self.call("screenshot", json!({}));
            if !reply.is_still_starting() {
                assert!(!reply.is_error(), "screenshot failed: {}", reply.raw);
                return reply.image();
            }
            assert!(Instant::now() < end, "the screen was still starting");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The probe on this agent's screen.
    pub fn probe(&self) -> &Probe {
        self.probe.as_ref().expect("this agent has no probe")
    }

    /// The agent goes away: its shim is killed, so its connection closes.
    pub fn kill(&mut self) {
        self.probe = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.probe = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The user's desktop, the daemon, and the agents that connect to it in
/// turn. Sessions are numbered from 1 in the order the agents connect.
pub struct World {
    // Dropped in this order: the daemon first, then the desktop.
    pub daemon: Daemon,
    pub desk: Desktop,
    connected: u64,
}

impl World {
    /// The user on workspace 10, where the views are, as `hypr-computer
    /// watch` takes them.
    pub fn new() -> World {
        let desk = Desktop::new();
        let daemon = desk.start_daemon();
        desk.hypr.user_shows_workspace(VIEWS_WORKSPACE);
        World {
            daemon,
            desk,
            connected: 0,
        }
    }

    /// An agent called `name`, its screen ready, without an app on it.
    pub fn bare_agent(&mut self, name: &str) -> Agent {
        self.connected += 1;
        let mut agent = Agent::connect(&self.desk, name, self.connected);
        agent.screenshot();
        self.view(agent.session);
        agent
    }

    /// An agent called `name`, its screen ready, with the probe on it and its
    /// view on the desktop.
    pub fn agent(&mut self, name: &str) -> Agent {
        let mut agent = self.bare_agent(name);
        let probe = Probe::start(&self.desk.screen(agent.session));
        assert_eq!((probe.width, probe.height), (WIDTH as i32, HEIGHT as i32));
        probe.wait_keyboard_focus();
        agent.probe = Some(probe);
        agent
    }

    /// The address of session `n`'s view, once it has mapped.
    pub fn view(&self, n: u64) -> String {
        let suffix = format!("(session {n})");
        let mut found = None;
        let ok = wait_until(TIMEOUT, || {
            found = self
                .desk
                .mapped_views()
                .into_iter()
                .find(|w| w.title.ends_with(&suffix))
                .and_then(|w| w.address);
            found.is_some()
        });
        assert!(
            ok,
            "session {n} has no view on the desktop\n{}",
            self.desk.report(&self.daemon)
        );
        found.unwrap()
    }

    /// Whether session `n` still has a view on the desktop.
    pub fn has_view(&self, n: u64) -> bool {
        let suffix = format!("(session {n})");
        self.desk.views().iter().any(|w| w.title.ends_with(&suffix))
    }

    /// Where the point `(x, y)` of an agent's screen is in its view.
    fn in_view(&self, address: &str, x: f64, y: f64) -> (f64, f64) {
        let picture = self.capture(address);
        let scale = (f64::from(picture.width) / f64::from(WIDTH))
            .min(f64::from(picture.height) / f64::from(HEIGHT));
        let left = (f64::from(picture.width) - f64::from(WIDTH) * scale) / 2.0;
        let top = (f64::from(picture.height) - f64::from(HEIGHT) * scale) / 2.0;
        (left + x * scale, top + y * scale)
    }

    /// The user clicks the left button in session `n`'s view, over the point
    /// `(x, y)` of the agent's screen.
    #[track_caller]
    pub fn user_clicks(&self, n: u64, x: f64, y: f64) {
        self.user_clicks_button(n, x, y, BTN_LEFT);
    }

    #[track_caller]
    pub fn user_clicks_button(&self, n: u64, x: f64, y: f64, button: u32) {
        let address = self.view(n);
        let at = self.in_view(&address, x, y);
        self.desk
            .host
            .click(&address, at, button)
            .unwrap_or_else(|e| panic!("the user can't click the view: {e}"));
    }

    /// The user takes over `agent`'s screen by clicking its view over
    /// `(x, y)`, and the test waits until the take-over has taken effect: the
    /// user's pointer reaches the probe on the screen. (The click and the
    /// agent's next call race, as for a real user; this lets the click win.)
    #[track_caller]
    pub fn take_over(&self, agent: &Agent, x: f64, y: f64) {
        self.user_clicks(agent.session, x, y);
        self.wait_taken_over(agent, x, y);
    }

    /// Waits until the user's pointer, moved over `agent`'s view near
    /// `(x, y)`, reaches the probe on its screen: the user has taken it over.
    #[track_caller]
    pub fn wait_taken_over(&self, agent: &Agent, x: f64, y: f64) {
        let probe = agent.probe();
        let mark = probe.mark();
        let end = Instant::now() + TIMEOUT;
        let mut nudge = 1.0;
        loop {
            self.user_hovers(agent.session, x + nudge, y);
            nudge = -nudge;
            let reached = wait_until(Duration::from_millis(100), || {
                probe
                    .since(mark)
                    .iter()
                    .any(|e| matches!(e, Event::Motion { .. }))
            });
            if reached {
                return;
            }
            assert!(
                Instant::now() < end,
                "after a click into its view, the user's pointer never reached the agent's \
                 screen: the user didn't take over\n{}",
                self.report()
            );
        }
    }

    /// The user moves the pointer over session `n`'s view, to the point
    /// `(x, y)` of the agent's screen.
    #[track_caller]
    pub fn user_hovers(&self, n: u64, x: f64, y: f64) {
        let address = self.view(n);
        let at = self.in_view(&address, x, y);
        self.desk
            .host
            .hover(&address, at)
            .unwrap_or_else(|e| panic!("the user can't move over the view: {e}"));
    }

    /// The user presses `keys` (xkb names or combinations like `ctrl+v`) in
    /// session `n`'s view, which they clicked before.
    #[track_caller]
    pub fn user_presses(&self, n: u64, keys: &[&str]) {
        let address = self.view(n);
        self.desk
            .host
            .keys(&address, keys)
            .unwrap_or_else(|e| panic!("the user can't type in the view: {e}"));
    }

    /// What session `n`'s view shows now.
    pub fn view_picture(&self, n: u64) -> Picture {
        self.capture(&self.view(n))
    }

    fn capture(&self, address: &str) -> Picture {
        let mut picture = None;
        let ok = wait_until(TIMEOUT, || {
            picture = self.desk.host.capture(address).ok();
            picture.is_some()
        });
        assert!(ok, "the view at {address} shows nothing");
        picture.unwrap()
    }

    /// Runs `hypr-computer <args>`.
    pub fn hypr_computer(&self, args: &[&str]) -> Output {
        self.desk.hypr_computer(args)
    }

    /// `hypr-computer give`, which must hand back.
    #[track_caller]
    pub fn user_gives(&self) -> String {
        let out = self.hypr_computer(&["give"]);
        assert!(
            out.status.success(),
            "`hypr-computer give` failed ({:?}): {}{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Everything that might explain a failure.
    pub fn report(&self) -> String {
        self.desk.report(&self.daemon)
    }
}

/// How many pixels of `view` differ from `shot` (of the same size) by more
/// than a rounding error.
pub fn pixels_unlike(view: &Picture, shot: &Image) -> usize {
    assert_eq!(
        (view.width, view.height),
        (shot.width, shot.height),
        "the view isn't the screen's size"
    );
    let mut unlike = 0;
    for y in 0..shot.height {
        for x in 0..shot.width {
            let (a, b) = (view.pixel(x, y), shot.pixel(x, y));
            if a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 8) {
                unlike += 1;
            }
        }
    }
    unlike
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

/// The input among `events`.
pub fn inputs(events: &[Event]) -> Vec<Event> {
    events.iter().filter(|e| is_input(e)).cloned().collect()
}

/// The button presses and releases among `events`: (button, pressed, x, y).
pub fn buttons(events: &[Event]) -> Vec<(u32, bool, f64, f64)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Button {
                button,
                pressed,
                x,
                y,
                ..
            } => Some((*button, *pressed, *x, *y)),
            _ => None,
        })
        .collect()
}

/// The names of the keys pressed among `events`, in order.
pub fn keys_pressed(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key {
                name,
                pressed: true,
                ..
            } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `presses` is exactly one press of the left button at `(x, y)`.
pub fn one_press_at(presses: &[(u32, f64, f64)], x: f64, y: f64) -> bool {
    matches!(presses, [(BTN_LEFT, px, py)] if (px - x).abs() <= 1.0 && (py - y).abs() <= 1.0)
}
