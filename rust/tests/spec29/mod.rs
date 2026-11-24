//! Helpers for the black-box tests of #29 ("You're told when an agent is
//! working"), written from the spec (specs/Notice.tla and the issue) by an
//! independent test agent.
//!
//! They start the real daemon and shim as agents do (MCP through the shim),
//! each test in a throwaway runtime dir with a cleared environment
//! (`support`). The notification service is [`Notifications`], a fake
//! `org.freedesktop.Notifications` on a private bus of the test's own
//! ([`Bus`]): the tests never reach the notification service, the session
//! bus or the desktop of the machine running them. The host desktop, for the
//! fallback, is `support::Desktop` with its fake Hyprland.
//!
//! The interface these tests assume:
//! - **The bus.** The daemon reaches the notification service on the D-Bus
//!   session bus: `$DBUS_SESSION_BUS_ADDRESS`, or `unix:path=$XDG_RUNTIME_DIR/bus`
//!   when that isn't set, as systemd puts it. The service is
//!   `org.freedesktop.Notifications` at `/org/freedesktop/Notifications`,
//!   interface `org.freedesktop.Notifications` (the Desktop Notifications
//!   Specification, 1.2).
//! - **The notice.** Once an agent has said its name (its first MCP request),
//!   the daemon calls `Notify` once for its session: app name `hypr-computer`,
//!   `replaces_id` 0, `expire_timeout` 0 (it never expires), and the actions
//!   `ok` (labelled "OK") and `stop` (a label with "Stop"). Its summary and
//!   body name the agent as its live view does, `<name> (session <n>)`, and
//!   say where to watch it: "Super+0" or `hypr-computer watch`. It isn't sent
//!   again while the session lasts.
//! - **"Done".** When the session ends (the agent exits, is killed, or the
//!   daemon is stopped with SIGTERM), a notice still on screen is replaced:
//!   `Notify` with `replaces_id` its id, no actions, `expire_timeout` from 1 to
//!   10 000 ms, naming the agent as above and saying "done". This also
//!   happens when the service shows the notice only after the session ended.
//!   The daemon may close nothing instead: Omarchy's shell keeps a closed
//!   notification on screen, and the fake can too.
//! - **The buttons.** `ActionInvoked(id, "stop")` on a session's notice stops
//!   that session, and no other, as `hypr-computer stop <session>` does
//!   (#28): its tool calls fail with "Stopped by the user", and
//!   `hypr-computer resume` lets it go on. `ActionInvoked(id, "ok")`, or
//!   `NotificationClosed(id, _)`, lets the agent go on. After any of them the
//!   notice is gone, and the session's end shows no "done". Signals about
//!   other notifications change nothing.
//! - **The fallback.** When `Notify` fails (no session bus, no service on it,
//!   or an error), or isn't answered within a few seconds, the host desktop
//!   shows its own notice: on Hyprland, one IPC request
//!   `/notify <icon> <time_ms> <color> <message>` per session, with
//!   `time_ms` > 0, its message naming the agent as above, saying where to
//!   watch it, and how to stop it (`hypr-computer stop`). It appears within
//!   [`TOLD`] of the agent saying its name. A service that answers in time
//!   gets no host notice. A late answer's notice is kept, and replaced by
//!   "done" when the session ends. Whatever the service does, the agents'
//!   calls, the `hypr-computer` commands and the daemon's exit don't wait
//!   for it.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

pub mod bus;
pub mod notifications;

use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use bus::Bus;
pub use notifications::{Mode, Notifications, Notify};

use crate::support::{self, Daemon, Desktop, Env, Ran, Shim, TIMEOUT};

/// How soon the user must be told, by the service or the host desktop, once
/// an agent has said its name, whatever the service does.
pub const TOLD: Duration = Duration::from_secs(10);

/// What a test's session bus and notification service are like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// A service that shows notices and answers at once.
    Up,
    /// A service that fails every call.
    Failing,
    /// A service that takes calls and answers none until released.
    Hung,
    /// A session bus with no notification service on it.
    Absent,
    /// No session bus at all.
    NoBus,
    /// A service that answers at once, on a bus at `$XDG_RUNTIME_DIR/bus`,
    /// and no `$DBUS_SESSION_BUS_ADDRESS`.
    InRuntimeDir,
}

/// A daemon, the session bus and notification service it is given, and
/// maybe a desktop. Fields drop in order: the daemon goes first.
pub struct World {
    pub daemon: Daemon,
    service: Option<Notifications>,
    bus: Option<Bus>,
    pub desktop: Option<Desktop>,
    pub env: Env,
    sessions: Cell<u64>,
}

impl World {
    /// A daemon with no desktop.
    pub fn start(service: Service) -> World {
        Self::build(service, false)
    }

    /// A daemon of a Hyprland session, on `support::Desktop`.
    pub fn on_desktop(service: Service) -> World {
        Self::build(service, true)
    }

    fn build(kind: Service, on_desktop: bool) -> World {
        let env = Env::new();
        let desktop = on_desktop.then(|| Desktop::start(&env));
        let bus = match kind {
            Service::NoBus => None,
            Service::InRuntimeDir => Some(Bus::start(&env.runtime_dir().join("bus"))),
            _ => Some(Bus::start(&env.path("bus"))),
        };
        let mode = match kind {
            Service::Up | Service::InRuntimeDir => Some(Mode::Up),
            Service::Failing => Some(Mode::Failing),
            Service::Hung => Some(Mode::Hung),
            Service::Absent | Service::NoBus => None,
        };
        let service = match (&bus, mode) {
            (Some(bus), Some(mode)) => Some(Notifications::start(&bus.address, mode)),
            _ => None,
        };
        let mut cmd = env.command(&support::daemon_bin());
        if let Some(desktop) = &desktop {
            desktop.apply(&mut cmd);
        }
        if let Some(bus) = &bus
            && kind != Service::InRuntimeDir
        {
            cmd.env("DBUS_SESSION_BUS_ADDRESS", &bus.address);
        }
        let daemon = Daemon::spawn(&env, cmd, "daemon.log");
        World {
            daemon,
            service,
            bus,
            desktop,
            env,
            sessions: Cell::new(0),
        }
    }

    /// The fake notification service.
    pub fn notifications(&self) -> &Notifications {
        self.service
            .as_ref()
            .expect("this world has no notification service")
    }

    /// An agent called `name`, connected and introduced: its session is the
    /// next one (agents are started one at a time).
    pub fn agent(&self, name: &str) -> Agent {
        let mut agent = self.connect();
        agent.introduce(name);
        agent
    }

    /// An agent that has connected but said nothing yet.
    pub fn connect(&self) -> Agent {
        self.sessions.set(self.sessions.get() + 1);
        Agent {
            shim: Shim::start(&self.env),
            name: String::new(),
            session: self.sessions.get(),
        }
    }

    /// Runs `hypr-computer <args>` as the user does.
    pub fn hypr_computer(&self, args: &[&str]) -> Ran {
        support::hypr_computer(&self.env, args)
    }

    /// Runs `hypr-computer <args>`, which must succeed.
    #[track_caller]
    pub fn user_runs(&self, args: &[&str]) -> Ran {
        let ran = self.hypr_computer(args);
        assert!(
            ran.success,
            "`hypr-computer {}` failed: {}{}",
            args.join(" "),
            ran.stdout,
            ran.stderr
        );
        ran
    }

    /// The `Notify` calls that are `agent`'s notice (with a Stop button, not
    /// replacing anything), shown or not.
    pub fn notices_of(&self, agent: &Agent) -> Vec<Notify> {
        let label = agent.label();
        self.notifications()
            .notifies()
            .into_iter()
            .filter(|n| is_notice(n) && n.text().contains(&label))
            .collect()
    }

    /// Waits until the service has shown `agent`'s notice, and returns its id
    /// and the call.
    pub fn notice_of(&self, agent: &Agent) -> (u32, Notify) {
        let label = agent.label();
        let shown = |n: &Notify| is_notice(n) && n.text().contains(&label) && n.id.is_some();
        let notifies = self.notifications().wait_for(
            &format!("a notice naming {label:?}, and showed it"),
            |all| all.iter().any(shown),
        );
        let notice = notifies.into_iter().find(shown).unwrap();
        (notice.id.unwrap(), notice)
    }

    /// Waits until the service has been asked for `agent`'s notice, shown or
    /// not (a hung service holds it).
    pub fn asked_for_notice_of(&self, agent: &Agent) -> Notify {
        let label = agent.label();
        let asked = |n: &Notify| is_notice(n) && n.text().contains(&label);
        let notifies = self
            .notifications()
            .wait_for(&format!("a notice naming {label:?}"), |all| {
                all.iter().any(asked)
            });
        notifies.into_iter().find(asked).unwrap()
    }

    /// The `Notify` calls that replace notification `id`.
    pub fn replacements_of(&self, id: u32) -> Vec<Notify> {
        self.notifications()
            .notifies()
            .into_iter()
            .filter(|n| n.replaces_id == id)
            .collect()
    }

    /// Waits until notification `id` has been replaced, and returns the
    /// replacement.
    pub fn wait_for_replacement_of(&self, id: u32) -> Notify {
        let notifies = self
            .notifications()
            .wait_for(&format!("a Notify replacing notice {id}"), |all| {
                all.iter().any(|n| n.replaces_id == id)
            });
        notifies.into_iter().find(|n| n.replaces_id == id).unwrap()
    }

    /// The notices the host desktop (the fake Hyprland) was asked to show:
    /// the messages of its `/notify` requests.
    pub fn host_notices(&self) -> Vec<HostNotice> {
        let desktop = self.desktop.as_ref().expect("this world has no desktop");
        desktop
            .hyprland
            .requests()
            .iter()
            .filter_map(|r| HostNotice::parse(r))
            .collect()
    }

    /// Waits up to [`TOLD`] until the host desktop has been asked to show a
    /// notice naming `agent`, and returns the ones naming it.
    pub fn wait_for_host_notice_of(&self, agent: &Agent) -> Vec<HostNotice> {
        let label = agent.label();
        let end = Instant::now() + TOLD;
        loop {
            let notices: Vec<HostNotice> = self
                .host_notices()
                .into_iter()
                .filter(|n| n.message.contains(&label))
                .collect();
            if !notices.is_empty() {
                return notices;
            }
            assert!(
                Instant::now() < end,
                "the host desktop showed no notice naming {label:?} within {TOLD:?}; Hyprland \
                 got {:?}\ndaemon log:\n{}",
                self.desktop.as_ref().unwrap().hyprland.requests(),
                self.daemon.log()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Whether `n` is an agent's notice: it has a Stop button and replaces
/// nothing.
pub fn is_notice(n: &Notify) -> bool {
    n.replaces_id == 0 && n.action_keys().contains(&"stop")
}

/// Whether `text` says where to watch the agent.
pub fn says_where_to_watch(text: &str) -> bool {
    text.contains("Super+0") || text.contains("hypr-computer watch")
}

/// A notice Hyprland was asked to show: `/notify <icon> <time_ms> <color> <message>`.
#[derive(Clone, Debug)]
pub struct HostNotice {
    pub time_ms: i64,
    pub message: String,
}

impl HostNotice {
    fn parse(request: &str) -> Option<HostNotice> {
        let rest = request.strip_prefix("/notify ")?;
        let mut parts = rest.splitn(4, ' ');
        let _icon = parts.next()?;
        let time_ms = parts.next()?.parse().ok()?;
        let _color = parts.next()?;
        let message = parts.next()?.to_owned();
        Some(HostNotice { time_ms, message })
    }
}

/// An agent: the real shim, driven as an agent drives it.
pub struct Agent {
    pub shim: Shim,
    pub name: String,
    /// Its session's number, as on its live view.
    pub session: u64,
}

impl Agent {
    /// The MCP handshake, as an agent called `name`.
    pub fn introduce(&mut self, name: &str) {
        self.shim.initialize_as(name);
        self.name = name.to_owned();
    }

    /// How its live view, and its notice, name it.
    pub fn label(&self) -> String {
        format!("{} (session {})", self.name, self.session)
    }

    /// A screenshot call's result, once its screen is no longer starting.
    pub fn screenshot_result(&mut self) -> Value {
        let end = Instant::now() + TIMEOUT;
        loop {
            let resp = self
                .shim
                .request("tools/call", json!({"name": "screenshot", "arguments": {}}));
            let text = resp.to_string();
            if !(resp["result"]["isError"] == json!(true) && text.contains("still starting")) {
                return resp;
            }
            assert!(Instant::now() < end, "the screen was still starting");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether its calls are refused as a stopped agent's are: a tool error
    /// (not a protocol error) saying "Stopped by the user" and telling it to
    /// ask the user.
    pub fn is_stopped(&mut self) -> bool {
        let resp = self.screenshot_result();
        let text = resp.to_string();
        resp.get("error").is_none()
            && resp["result"]["isError"] == json!(true)
            && text.contains("Stopped by the user")
            && text.to_lowercase().contains("ask the user")
    }

    /// Fails the test unless a screenshot works.
    #[track_caller]
    pub fn assert_works(&mut self) {
        let resp = self.screenshot_result();
        assert!(
            resp.get("error").is_none() && resp["result"]["isError"] != json!(true),
            "{}'s screenshot failed: {resp}",
            self.label()
        );
    }

    /// Waits until its calls are refused as a stopped agent's.
    #[track_caller]
    pub fn assert_becomes_stopped(&mut self) {
        let end = Instant::now() + TIMEOUT;
        while !self.is_stopped() {
            assert!(
                Instant::now() < end,
                "{} was never stopped: {}",
                self.label(),
                self.screenshot_result()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The agent exits: it closes the shim's stdin, as an agent does.
    pub fn exit(&mut self) {
        self.shim.close_stdin();
        self.shim.wait_exit();
    }

    /// The agent is killed: its shim gets SIGKILL.
    pub fn kill(&mut self) {
        self.shim.kill();
    }
}
