//! Every agent session gets a screen of its own: the real compositor, in
//! software mode, driven as an agent drives it.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use support::app::App;
use support::{Daemon, Env, Shim, alive, files_in, kill_9};

const STILL_STARTING: &str = "Your screen is still starting. Call the tool again in a moment.";

/// A daemon whose screens run `script` (sh) instead of the real screen.
fn daemon_with_screen(env: &Env, script: &str) -> Daemon {
    let program = env.path("screen.sh");
    fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = env.command(&support::daemon_bin());
    cmd.env("HYPR_COMPUTER_SCREEN", &program);
    Daemon::spawn(env, cmd, "daemon.log")
}

fn real_screen() -> PathBuf {
    support::bin("hypr-computer-screen")
}

/// An agent whose screen is ready.
fn agent(env: &Env) -> Shim {
    let mut shim = Shim::start(env);
    shim.initialize();
    shim.screenshot();
    shim
}

#[test]
fn screenshot_returns_the_agents_own_1280x800_screen() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();

    assert!(shim.tool_names().contains(&"screenshot".to_owned()));

    let shot = shim.screenshot();
    assert_eq!((shot.width, shot.height), (1280, 800));
}

const RED: [u8; 3] = [0xd0, 0x20, 0x20];
const BLUE: [u8; 3] = [0x20, 0x40, 0xd0];

#[test]
fn screenshot_shows_the_app_on_the_screen_full_size_and_right_way_up() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();
    shim.screenshot();

    let _app = App::paint(&env.screen_socket(1), RED, BLUE);
    let shot = shim.screenshot();
    for (x, y) in [(0, 0), (1279, 0), (640, 399)] {
        assert_eq!(shot.pixel(x, y), RED, "at {x},{y}");
    }
    for (x, y) in [(640, 400), (0, 799), (1279, 799)] {
        assert_eq!(shot.pixel(x, y), BLUE, "at {x},{y}");
    }
}

#[test]
fn two_agents_get_two_screens_and_neither_sees_the_others() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut a = agent(&env);
    let mut b = agent(&env);

    let _on_a = App::paint(&env.screen_socket(1), RED, RED);
    let _on_b = App::paint(&env.screen_socket(2), BLUE, BLUE);
    assert_eq!(a.screenshot().pixel(640, 400), RED);
    assert_eq!(b.screenshot().pixel(640, 400), BLUE);
    assert!(daemon.log().contains("session 2 started; 2 open"));
}

#[test]
fn killing_an_agent_removes_its_screen_at_once_and_nothing_else() {
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let mut a = agent(&env);
    let mut b = agent(&env);
    let (screen_a, screen_b) = (daemon.screen_pid(1), daemon.screen_pid(2));
    assert!(alive(screen_a) && env.screen_dir(1).exists());

    let killed = Instant::now();
    a.kill(); // SIGKILL: the agent's side gets no chance to clean up.
    daemon.wait_for_log("session 1 ended (the agent disconnected)");
    assert!(
        killed.elapsed() < Duration::from_secs(1),
        "teardown took {:?}",
        killed.elapsed()
    );
    assert!(
        !alive(screen_a),
        "session 1's screen process is still there"
    );
    assert_eq!(
        files_in(&env.screen_dir(1)),
        None,
        "session 1's screen left files"
    );

    assert!(daemon.is_running());
    assert!(alive(screen_b));
    assert_eq!(b.screenshot().width, 1280);
    assert!(
        !daemon.log().contains("session 2 ended"),
        "{}",
        daemon.log()
    );
}

#[test]
fn a_screen_killed_with_sigkill_leaves_no_files_and_ends_only_its_session() {
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let mut a = agent(&env);
    let mut b = agent(&env);
    let screen_a = daemon.screen_pid(1);
    assert!(env.screen_socket(1).exists());

    kill_9(screen_a);
    daemon.wait_for_log("session 1 ended (its screen exited)");
    assert_eq!(
        files_in(&env.screen_dir(1)),
        None,
        "the dead screen's files are still there"
    );
    // Its agent's connection is closed: the agent reconnects to get a new screen.
    assert!(!a.wait_exit().success());

    assert!(daemon.is_running());
    assert_eq!(b.screenshot().width, 1280);
}

#[test]
fn a_tool_call_while_the_screen_is_starting_is_refused_then_served_once_it_is_ready() {
    let env = Env::new();
    let go = env.path("go");
    // The real screen, held back until the test says go (for at most 10s).
    let _daemon = daemon_with_screen(
        &env,
        &format!(
            "for i in $(seq 1000); do [ -e '{}' ] && exec '{}' \"$@\"; sleep 0.01; done; exit 1",
            go.display(),
            real_screen().display()
        ),
    );
    let mut shim = Shim::start(&env);
    shim.initialize();

    let refused = shim.call("screenshot", json!({}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(
        refused["content"],
        json!([{"type": "text", "text": STILL_STARTING}])
    );

    fs::write(&go, "").unwrap();
    assert_eq!(shim.screenshot().width, 1280);
}

/// The spec's HandOffEarly bug: the screen's process answering is not enough.
#[test]
fn a_screen_that_answers_but_fails_its_readiness_check_never_serves_a_call() {
    let env = Env::new();
    // Says it is up, then never answers a request (and ends with its stdin).
    let daemon = daemon_with_screen(&env, "echo ready; cat > /dev/null");
    let mut shim = Shim::start(&env);
    shim.initialize();
    daemon.wait_for_log("session 1: starting its screen");

    for _ in 0..5 {
        let refused = shim.call("screenshot", json!({}));
        assert_eq!(
            refused["content"],
            json!([{"type": "text", "text": STILL_STARTING}]),
            "{refused}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!daemon.log().contains("screen ready"), "{}", daemon.log());
}

#[test]
fn when_the_daemon_stops_every_screen_goes_with_it() {
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let _a = agent(&env);
    let _b = agent(&env);
    let screens = [daemon.screen_pid(1), daemon.screen_pid(2)];

    assert!(daemon.terminate().success(), "{}", daemon.log());
    for (n, pid) in [1, 2].into_iter().zip(screens) {
        assert!(!alive(pid), "session {n}'s screen outlived the daemon");
        assert_eq!(
            files_in(&env.screen_dir(n)),
            None,
            "session {n}'s screen left files"
        );
    }
    assert!(
        daemon
            .log()
            .contains("session 2 ended (the daemon is stopping)")
    );
}
