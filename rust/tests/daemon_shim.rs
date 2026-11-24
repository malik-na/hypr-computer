//! The daemon and the shim, driven as an agent drives them.
#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use support::{Daemon, Env, Shim};

#[test]
fn handshake_then_the_tool_list() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);

    let init = shim.initialize();
    assert_eq!(init["serverInfo"]["name"], "hypr-computer");
    assert!(init["capabilities"]["tools"].is_object(), "{init}");

    assert_eq!(shim.tool_names(), ["screenshot", "launch_app"]);

    let ping = shim.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");
}

#[test]
fn calling_an_unknown_tool_is_an_error_that_names_it() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();

    let resp = shim.request("tools/call", json!({"name": "nope", "arguments": {}}));
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
    assert_eq!(resp["error"]["message"], "unknown tool: nope", "{resp}");
}

#[test]
fn the_socket_dir_is_private_to_the_user() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mode = fs::metadata(env.daemon_dir()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn two_shims_at_once_get_two_sessions() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut a = Shim::start(&env);
    a.initialize();
    daemon.wait_for_log("session 1 started");
    let mut b = Shim::start(&env);
    b.initialize();
    let log = daemon.wait_for_log("session 2 started; 2 open");

    let tools = a.tool_names();
    assert!(tools.contains(&"screenshot".to_owned()), "{tools:?}");
    assert_eq!(b.tool_names(), tools);
    assert!(!log.contains("ended"), "{log}");
}

#[test]
fn killing_a_shim_ends_exactly_its_session() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut a = Shim::start(&env);
    a.initialize();
    daemon.wait_for_log("session 1 started");
    let mut b = Shim::start(&env);
    b.initialize();
    daemon.wait_for_log("session 2 started");

    a.kill(); // SIGKILL: the shim gets no chance to clean up.
    let log = daemon.wait_for_log("session 1 ended (the agent disconnected)");
    assert!(log.contains("1 open"), "{log}");
    assert!(!log.contains("session 2 ended"), "{log}");

    let ping = b.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");
}

#[test]
fn closing_the_shims_stdin_ends_its_session() {
    let env = Env::new();
    let daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();
    daemon.wait_for_log("session 1 started");

    shim.close_stdin();
    assert!(shim.wait_exit().success());
    daemon.wait_for_log("session 1 ended (the agent disconnected)");
}

#[test]
fn without_a_daemon_the_shim_fails_with_one_line_saying_how_to_start_it() {
    let env = Env::new();
    let mut shim = Shim::start(&env);
    let status = shim.wait_exit();
    let stderr = shim.stderr();

    assert!(!status.success());
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    assert!(
        stderr.contains(&env.socket().display().to_string()),
        "{stderr:?}"
    );
    assert!(stderr.contains("hypr-computer-daemon"), "{stderr:?}");
}

#[test]
fn when_the_daemon_stops_its_shims_exit_and_the_socket_is_removed() {
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();
    daemon.wait_for_log("session 1 started");

    let status = daemon.terminate();
    assert!(status.success(), "{status}: {}", daemon.log());
    assert!(!shim.wait_exit().success());
    let log = daemon.log();
    assert!(
        log.contains("session 1 ended (the daemon is stopping)"),
        "{log}"
    );
    assert!(!env.socket().exists(), "the daemon left its socket behind");
}

#[test]
fn a_second_daemon_refuses_to_start_and_the_first_keeps_working() {
    let env = Env::new();
    let _first = Daemon::start(&env);
    let out = env.command(&support::daemon_bin()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("already running"), "{stderr}");

    let mut shim = Shim::start(&env);
    shim.initialize();
}

#[test]
fn a_stale_socket_from_a_dead_daemon_is_replaced() {
    let env = Env::new();
    let mut first = Daemon::start(&env);
    assert!(first.terminate().success());
    // Leave a socket file nobody listens on, as a daemon killed by SIGKILL would.
    let stale = std::os::unix::net::UnixListener::bind(env.socket()).unwrap();
    drop(stale);

    let _second = Daemon::start(&env);
    let mut shim = Shim::start(&env);
    shim.initialize();
}
