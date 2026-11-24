//! #34: logs are kept a few days (3 by default, `days` in the settings),
//! then removed; a running agent's log never is. Logs are made old by
//! setting their modification time back, never by waiting. Written from the
//! spec alone; spec34/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec34;

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::json;
use spec34::{DAY, TIMEOUT, World, backdate, wait_until};

/// An agent called `name` makes a call and leaves; returns its log file.
fn session_that_ended(world: &mut World, name: &str, n: u64) -> PathBuf {
    let mut a = world.agent(name);
    a.shim.ok("click", json!({"x": 640, "y": 400}));
    world.wait_records(n, 2);
    a.shim.close_stdin();
    assert!(a.shim.wait_exit(), "{name}'s shim didn't exit");
    drop(a);
    world.session_file(n)
}

/// Waits until `file` is gone; fails the test if it stays.
#[track_caller]
fn assert_removed(world: &World, file: &Path, what: &str) {
    assert!(
        wait_until(TIMEOUT, || !file.exists()),
        "{what} must be removed: {} is still there; the log dir holds {:?}",
        file.display(),
        spec34::files_under(&world.log_dir())
    );
}

/// Fails the test unless `file` is still there, a moment after the sweep
/// that removed another log.
#[track_caller]
fn assert_kept(file: &Path, what: &str) {
    thread::sleep(Duration::from_millis(300));
    assert!(
        file.exists(),
        "{what} must be kept: {} is gone",
        file.display()
    );
}

#[test]
fn a_log_older_than_three_days_is_removed_when_an_agent_connects() {
    let mut world = World::new();
    let old = session_that_ended(&mut world, "alpha", 1);
    let recent = session_that_ended(&mut world, "beta", 2);
    backdate(&old, 3 * DAY + Duration::from_secs(3600));
    backdate(&recent, 2 * DAY + Duration::from_secs(20 * 3600));

    let (_gamma, _) = world.connect("gamma");

    assert_removed(
        &world,
        &old,
        "alpha's log, last written 3 days and an hour ago",
    );
    assert_kept(&recent, "beta's log, last written 2 days and 20 hours ago");
    let ran = world.hypr_computer(&["log", "alpha"]);
    assert!(
        !ran.success,
        "`log alpha` must fail once alpha's log is removed: {ran:?}"
    );
    world.user_runs(&["log", "beta"]);
}

#[test]
fn old_logs_are_removed_when_the_daemon_starts() {
    let mut world = World::new();
    let old = session_that_ended(&mut world, "alpha", 1);
    backdate(&old, 4 * DAY);

    world.restart_daemon();

    assert_removed(&world, &old, "a 4-day-old log, once the daemon starts");
}

#[test]
fn how_long_logs_are_kept_is_a_setting() {
    let mut world = World::with_config("[log]\ndays = 10\n");
    let ancient = session_that_ended(&mut world, "alpha", 1);
    let old = session_that_ended(&mut world, "beta", 2);
    backdate(&ancient, 11 * DAY);
    backdate(&old, 9 * DAY);

    let (_gamma, _) = world.connect("gamma");

    assert_removed(&world, &ancient, "with days = 10, an 11-day-old log");
    assert_kept(&old, "with days = 10, a 9-day-old log");
}

#[test]
fn a_shorter_setting_removes_logs_sooner() {
    let mut world = World::with_config("[log]\ndays = 1\n");
    let old = session_that_ended(&mut world, "alpha", 1);
    let today = session_that_ended(&mut world, "beta", 2);
    backdate(&old, DAY + Duration::from_secs(3600));
    backdate(&today, Duration::from_secs(20 * 3600));

    let (_gamma, _) = world.connect("gamma");

    assert_removed(&world, &old, "with days = 1, a log written 25 hours ago");
    assert_kept(&today, "with days = 1, a log written 20 hours ago");
}

/// An agent that has been working, or idle, for longer than the retention
/// time still has its whole log, and goes on adding to the same file.
#[test]
fn a_running_agents_log_is_never_removed() {
    let mut world = World::new();
    let ended = session_that_ended(&mut world, "beta", 1);
    let mut alpha = world.agent("alpha");
    alpha.shim.ok("click", json!({"x": 111, "y": 222}));
    world.wait_records(2, 2);
    let running = world.session_file(2);
    backdate(&ended, 4 * DAY);
    backdate(&running, 4 * DAY);

    let (_gamma, _) = world.connect("gamma");
    assert_removed(&world, &ended, "the ended session's 4-day-old log");
    assert_kept(&running, "the running agent's log");

    alpha.shim.ok("click", json!({"x": 333, "y": 444}));
    let records = world.wait_records(2, 3);
    assert!(
        records.iter().all(|r| r.file == running),
        "the running agent goes on in the same file: {records:#?}"
    );
    let shown = world.user_runs(&["log", "alpha"]).stdout;
    for (x, y) in [("111", "222"), ("333", "444")] {
        assert!(
            shown.lines().any(|l| l.contains(x) && l.contains(y)),
            "the running agent's whole log must be shown, its click at ({x}, {y}) too:\n{shown}"
        );
    }
}
