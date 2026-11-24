//! Every agent session gets a screen of its own: the real compositor, in
//! software mode, driven as an agent drives it.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::json;
use support::app::App;
use support::{Daemon, Env, Shim, agent, alive, files_in, kill_9};

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

    let _app = App::paint(&env, 1, RED, BLUE);
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

    let _on_a = App::paint(&env, 1, RED, RED);
    let _on_b = App::paint(&env, 2, BLUE, BLUE);
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

/// A screen that passes its readiness check, then breaks the protocol and
/// hangs, deaf even to its stdin closing.
#[test]
fn a_screen_that_stops_making_sense_ends_its_session_instead_of_failing_every_call() {
    let env = Env::new();
    let shot = env.path("shot.png");
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, 1280, 800);
    encoder.set_color(png::ColorType::Rgb);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![0; 1280 * 800 * 3])
        .unwrap();
    fs::write(&shot, &png).unwrap();
    let daemon = daemon_with_screen(
        &env,
        &format!(
            "echo ready
             read request; printf 'png %s\\n' {len}; cat '{shot}'
             read request; echo nonsense
             exec sleep 15",
            len = png.len(),
            shot = shot.display()
        ),
    );
    let mut shim = Shim::start(&env);
    shim.initialize();
    daemon.wait_for_log("session 1: screen ready");

    let failed = shim.call("screenshot", json!({}));
    assert_eq!(failed["isError"], true, "{failed}");
    daemon.wait_for_log("session 1 ended");
    assert!(
        !shim.wait_exit().success(),
        "the agent's connection should close"
    );
}

#[test]
fn the_daemon_refuses_to_start_without_its_screen_program() {
    let env = Env::new();
    let missing = env.path("no-such-screen");
    let mut daemon = env
        .command(&support::daemon_bin())
        .env("HYPR_COMPUTER_SCREEN", &missing)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = support::wait_exit(&mut daemon, support::TIMEOUT);
    let _ = daemon.kill();
    let mut stderr = String::new();
    daemon
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!status.expect("the daemon started anyway").success());
    assert!(stderr.contains(&missing.display().to_string()), "{stderr}");
    assert!(
        !env.socket().exists(),
        "it shouldn't listen without a screen to give"
    );
}

#[test]
fn apps_on_the_screen_get_a_pointer_and_a_keyboard() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let _agent = agent(&env);

    let app = App::paint(&env, 1, RED, BLUE);
    assert_eq!(
        app.log.wait_for("ready")["seat"],
        json!(["pointer", "keyboard"])
    );
}

/// Before it has drawn, a window isn't set up for it: foot 1.16 crashed on
/// a keyboard it got earlier.
#[test]
fn an_app_gets_the_keyboard_only_once_its_window_has_drawn() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let _agent = agent(&env);

    let app = App::paint(&env, 1, RED, BLUE);
    assert_eq!(app.log.wait_for("keyboard")["drawn"], true);
}

/// foot, a terminal: it won't run on a screen without a clipboard.
#[test]
fn a_real_app_runs_on_the_screen_and_stays_up() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut agent = agent(&env);
    let empty = agent.screenshot();

    let mut foot = env
        .command(Path::new("foot"))
        .arg("cat")
        .env("WAYLAND_DISPLAY", env.screen_socket(1))
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run foot; the tests need it installed");
    let end = Instant::now() + support::TIMEOUT;
    while agent.screenshot() == empty {
        if let Some(status) = foot.try_wait().unwrap() {
            let mut why = String::new();
            foot.stderr
                .take()
                .unwrap()
                .read_to_string(&mut why)
                .unwrap();
            panic!("foot exited ({status}):\n{why}");
        }
        assert!(Instant::now() < end, "foot drew nothing");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_secs(1));
    let still = foot.try_wait().unwrap();
    let _ = foot.kill();
    let _ = foot.wait();
    assert_eq!(still, None, "foot didn't stay up");
}
