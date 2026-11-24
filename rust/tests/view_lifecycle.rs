//! A live view closes when its session ends, however the agent ended, and
//! the other agents' views stay (#27).
#![forbid(unsafe_code)]

mod support;
mod view_support;

use std::thread;
use std::time::{Duration, Instant};

use support::Daemon;
use view_support::{Desktop, is_daemon, running};

/// How soon a view must close after its session ends.
const PROMPTLY: Duration = Duration::from_secs(2);

/// Two agents with their views open; returns the views' addresses and pids.
fn two_agents(
    desk: &Desktop,
    daemon: &Daemon,
) -> (support::Shim, support::Shim, [(String, i32); 2]) {
    let a = desk.agent("first-agent");
    let first = desk.wait_for_views(1, daemon).remove(0);
    let b = desk.agent("second-agent");
    let second = desk
        .wait_for_views(2, daemon)
        .into_iter()
        .find(|v| v.address != first.address)
        .unwrap();
    let view = |v: view_support::host::HostWindow| (v.address.unwrap(), v.pid.unwrap_or(-1));
    (a, b, [view(first), view(second)])
}

fn wait_until_closed(desk: &Desktop, daemon: &Daemon, address: &str, pid: i32) {
    let start = Instant::now();
    loop {
        let on_host = desk
            .views()
            .iter()
            .any(|v| v.address.as_deref() == Some(address));
        let listed = desk.hypr.world().find(address).is_some();
        if !on_host && !listed {
            break;
        }
        assert!(
            start.elapsed() < PROMPTLY,
            "the view {address} is still open {PROMPTLY:?} after its session ended\n{}",
            desk.report(daemon)
        );
        thread::sleep(Duration::from_millis(10));
    }
    // A view drawn by a process of its own takes that process with it.
    if pid > 0 && !is_daemon(pid) {
        let end = Instant::now() + PROMPTLY;
        while running(pid) && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !running(pid),
            "the view's process {pid} outlived its session"
        );
    }
}

fn assert_still_open(desk: &Desktop, address: &str) {
    assert!(
        desk.views()
            .iter()
            .any(|v| v.address.as_deref() == Some(address)),
        "another agent's view closed too: {:?}",
        desk.views()
    );
}

#[test]
fn a_view_closes_when_its_agent_disconnects_and_the_other_view_stays() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let (mut a, _b, [(first, pid), (second, _)]) = two_agents(&desk, &daemon);

    a.close_stdin();
    daemon.wait_for_log("session 1 ended");
    wait_until_closed(&desk, &daemon, &first, pid);
    thread::sleep(Duration::from_millis(300));
    assert_still_open(&desk, &second);
}

#[test]
fn a_view_closes_when_its_agent_is_killed_with_sigkill_and_the_other_view_stays() {
    let desk = Desktop::new();
    let daemon = desk.start_daemon();
    let (mut a, _b, [(first, pid), (second, _)]) = two_agents(&desk, &daemon);

    a.kill(); // SIGKILL
    wait_until_closed(&desk, &daemon, &first, pid);
    thread::sleep(Duration::from_millis(300));
    assert_still_open(&desk, &second);
}

#[test]
fn every_view_closes_when_the_daemon_stops() {
    let desk = Desktop::new();
    let mut daemon = desk.start_daemon();
    let (_a, _b, views) = two_agents(&desk, &daemon);

    assert!(daemon.terminate().success(), "{}", daemon.log());
    for (address, pid) in views {
        wait_until_closed(&desk, &daemon, &address, pid);
    }
}
