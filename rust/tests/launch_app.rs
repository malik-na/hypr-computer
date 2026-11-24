//! `launch_app`: an agent starts an app, and it opens on the agent's own
//! screen and nowhere else.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use serde_json::json;
use support::app::{self, Log};
use support::{Daemon, Env, agent, alive, text};

const RED: [u8; 3] = [0xd0, 0x20, 0x20];
const BLUE: [u8; 3] = [0x20, 0x40, 0xd0];

/// The user's desktop, as the daemon's environment points to it: a Wayland
/// display, and a session bus in the user's runtime dir, each a socket that
/// notes whether anything connected.
struct Desktop {
    display: PathBuf,
    bus: PathBuf,
    listeners: Vec<UnixListener>,
}

impl Desktop {
    fn new(env: &Env) -> Desktop {
        let display = env.path("users-desktop");
        let bus = env.runtime_dir().join("bus");
        let listeners = [&display, &bus]
            .map(|socket| {
                let listener = UnixListener::bind(socket).unwrap();
                listener.set_nonblocking(true).unwrap();
                listener
            })
            .into();
        Desktop {
            display,
            bus,
            listeners,
        }
    }

    fn was_reached(&self) -> bool {
        self.listeners.iter().any(|l| l.accept().is_ok())
    }
}

/// A daemon started from the user's session: `WAYLAND_DISPLAY`, `DISPLAY` and
/// the session bus are the user's.
fn daemon_in_the_users_session(env: &Env, desktop: &Desktop) -> Daemon {
    let mut cmd = env.command(&support::daemon_bin());
    cmd.env("WAYLAND_DISPLAY", &desktop.display)
        .env("DISPLAY", ":42")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", desktop.bus.display()),
        );
    Daemon::spawn(env, cmd, "daemon.log")
}

#[test]
fn launch_app_opens_the_app_on_the_agents_screen_and_not_the_users() {
    let env = Env::new();
    let desktop = Desktop::new(&env);
    let _daemon = daemon_in_the_users_session(&env, &desktop);
    let mut agent = agent(&env);
    let log = Log::new(&env);

    let result = agent.call(
        "launch_app",
        json!({"app": app::program(), "args": app::args(RED, BLUE, &log)}),
    );
    assert_eq!(result["isError"], false, "{result}");
    assert!(
        text(&result).contains("window is on your screen"),
        "{result}"
    );

    // Already there: launch_app returns once the app's window is drawn.
    let shot = agent.screenshot();
    assert_eq!(shot.pixel(640, 200), RED);
    assert_eq!(shot.pixel(640, 600), BLUE);
    assert!(
        !desktop.was_reached(),
        "the app connected to the user's desktop"
    );
    let ready = log.wait_for("ready");
    assert_eq!(ready["display"], json!(null), "it got the user's X display");
    assert_eq!(ready["bus"], json!(null), "it got the user's session bus");
    let runtime = ready["runtime_dir"].as_str().unwrap();
    assert_ne!(
        Path::new(runtime),
        env.runtime_dir(),
        "it got the user's runtime dir, and the sockets in it"
    );
}

#[test]
fn an_app_and_everything_it_started_end_with_the_session() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut agent = agent(&env);
    let log = Log::new(&env);
    let started = env.path("started.pid");
    // The app starts a helper that would outlive the screen on its own.
    let script = format!(
        "sleep 60 & echo $! > '{}'; exec '{}' {}",
        started.display(),
        app::program().display(),
        app::args(RED, BLUE, &log).join(" ")
    );
    let result = agent.call("launch_app", json!({"app": "sh", "args": ["-c", script]}));
    assert_eq!(result["isError"], false, "{result}");
    let app = log.wait_for("ready")["pid"].as_u64().unwrap() as u32;
    let helper: u32 = fs::read_to_string(&started)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(app) && alive(helper));

    agent.kill();
    daemon.wait_for_log("session 1 ended");
    assert!(!alive(app), "the app outlived its session");
    assert!(!alive(helper), "what the app started outlived its session");
}

#[test]
fn an_app_that_ends_at_once_is_reported_with_what_it_printed() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut agent = agent(&env);

    let result = agent.call(
        "launch_app",
        json!({"app": "sh", "args": ["-c", "echo 'no display for me' >&2; exit 3"]}),
    );
    assert_eq!(result["isError"], true, "{result}");
    let said = text(&result);
    assert!(said.contains("exit status: 3"), "{said}");
    assert!(said.contains("no display for me"), "{said}");

    let result = agent.call("launch_app", json!({"app": "no-such-app-anywhere"}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(text(&result).contains("No such file"), "{result}");
}

#[test]
fn an_app_started_by_a_launcher_that_exits_at_once_is_found() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut agent = agent(&env);
    let log = Log::new(&env);
    // A launcher: starts the app in the background, and is done.
    let script = format!(
        "'{}' {} & exit 0",
        app::program().display(),
        app::args(RED, BLUE, &log).join(" ")
    );

    let result = agent.call("launch_app", json!({"app": "sh", "args": ["-c", script]}));
    assert_eq!(result["isError"], false, "{result}");
    assert!(
        text(&result).contains("window is on your screen"),
        "{result}"
    );
    assert_eq!(agent.screenshot().pixel(640, 200), RED);
}
