//! #30: `hypr-computer take <agent>` takes over as a click does; one screen
//! is taken over at a time, the other agents keep working, and an agent
//! that disconnects during a take-over ends its session as any other. The
//! interface these tests assume is at the top of tests/spec30/mod.rs.
#![forbid(unsafe_code)]

mod spec30;
mod support;
mod view_support;

use std::process::Output;

use serde_json::json;

use spec30::probe::presses;
use spec30::{Agent, TIMEOUT, WAITS, World, one_press_at, wait_until};

fn said(out: &Output) -> String {
    format!(
        "exit {:?}: {}{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Fails the test unless `agent`'s click at (`x`, `y`) is served at once and
/// lands there.
#[track_caller]
fn assert_served(agent: &mut Agent, x: i64, y: i64) {
    let mark = agent.probe().mark();
    let reply = agent.call("click", json!({"x": x, "y": y}));
    assert!(
        !reply.is_error() && !reply.says_taken_over(),
        "{:?} must be served as usual: {}",
        agent.name,
        reply.raw
    );
    agent.probe().wait_for(mark, |e| !presses(e).is_empty());
    assert!(
        one_press_at(&agent.probe().presses_since(mark), x as f64, y as f64),
        "{:?}'s click must land at ({x}, {y}): {:?}",
        agent.name,
        agent.probe().since(mark)
    );
}

#[test]
fn hypr_computer_take_takes_over_an_agent_by_name_or_session_number() {
    let mut w = World::new();
    let mut alpha = w.agent("alpha");
    let mut beta = w.agent("beta");

    let out = w.hypr_computer(&["take", "beta"]);
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("beta"),
        "`hypr-computer take beta` must take over beta's screen and say so: {}",
        said(&out)
    );
    let id = beta.begin("click", json!({"x": 100, "y": 100}));
    beta.assert_waits(id, WAITS, "beta's click");
    assert_served(&mut alpha, 300, 300);

    // The user's click in beta's view now reaches beta's app.
    let mark = beta.probe().mark();
    w.user_clicks(2, 500.0, 500.0);
    beta.probe().wait_for(mark, |e| !presses(e).is_empty());
    assert!(
        one_press_at(&beta.probe().presses_since(mark), 500.0, 500.0),
        "after `hypr-computer take`, the user's click must reach the agent's app: {:?}",
        beta.probe().since(mark)
    );

    let given = w.user_gives();
    assert!(given.contains("beta"), "`give` must name beta: {given:?}");
    let reply = beta.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "{}",
        reply.raw
    );

    let out = w.hypr_computer(&["take", "1"]);
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("alpha"),
        "`hypr-computer take 1` must take over session 1, alpha: {}",
        said(&out)
    );
    let id = alpha.begin("screenshot", json!({}));
    alpha.assert_waits(id, WAITS, "alpha's screenshot");
    assert_served(&mut beta, 400, 400);
    w.user_gives();
    let reply = alpha.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "{}",
        reply.raw
    );
}

#[test]
fn take_must_name_exactly_one_agent() {
    let mut w = World::new();
    let mut first = w.agent("twin");
    let mut second = w.agent("twin");

    for who in ["twin", "nobody", "9"] {
        let out = w.hypr_computer(&["take", who]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "`hypr-computer take {who}` names no one agent, so it must fail: {}",
            said(&out)
        );
        assert!(
            !out.stderr.is_empty(),
            "`hypr-computer take {who}` must say why it failed"
        );
    }
    let out = w.hypr_computer(&["take"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "`hypr-computer take` with no agent is a usage error: {}",
        said(&out)
    );
    assert_eq!(
        w.hypr_computer(&["give"]).status.code(),
        Some(1),
        "a failed take took something over"
    );
    assert_served(&mut first, 300, 300);
    assert_served(&mut second, 300, 300);
}

#[test]
fn clicking_into_another_agents_view_hands_the_first_back() {
    let mut w = World::new();
    let mut alpha = w.agent("alpha");
    let mut beta = w.agent("beta");
    w.take_over(&alpha, 640.0, 400.0);
    let id = alpha.begin("click", json!({"x": 100, "y": 100}));
    alpha.assert_waits(id, WAITS, "alpha's click");

    let mark = alpha.probe().mark();
    w.take_over(&beta, 640.0, 400.0);
    let reply = alpha.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "taking over beta must hand alpha back, and alpha's waiting click run: {}",
        reply.raw
    );
    alpha.probe().wait_for(mark, |e| !presses(e).is_empty());
    assert!(
        one_press_at(&alpha.probe().presses_since(mark), 100.0, 100.0),
        "alpha's waiting click must land once: {:?}",
        alpha.probe().since(mark)
    );

    let id = beta.begin("click", json!({"x": 200, "y": 200}));
    beta.assert_waits(id, WAITS, "beta's click");
    assert_served(&mut alpha, 300, 300);
    let given = w.user_gives();
    assert!(
        given.contains("beta"),
        "`give` must hand back beta: {given:?}"
    );
    let reply = beta.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "{}",
        reply.raw
    );
}

#[test]
fn an_agent_that_disconnects_during_a_take_over_ends_as_usual() {
    let mut w = World::new();
    let mut alpha = w.agent("alpha");
    let mut beta = w.agent("beta");
    let screen = w.daemon.screen_pid(1);
    w.take_over(&alpha, 640.0, 400.0);
    let _waiting = alpha.begin("click", json!({"x": 100, "y": 100}));

    alpha.kill();
    assert!(
        wait_until(TIMEOUT, || w.daemon.log().contains("session 1 ended")),
        "the session didn't end:\n{}",
        w.report()
    );
    assert!(
        wait_until(TIMEOUT, || !w.has_view(1) && !support::alive(screen)),
        "the ended session's view or screen stayed\n{}",
        w.report()
    );
    let out = w.hypr_computer(&["give"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the take-over must end with the session: {}",
        said(&out)
    );

    // The user can take over another agent, which works as before.
    assert_served(&mut beta, 300, 300);
    w.take_over(&beta, 640.0, 400.0);
    let id = beta.begin("click", json!({"x": 200, "y": 200}));
    beta.assert_waits(id, WAITS, "beta's click");
    w.user_gives();
    let reply = beta.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "{}",
        reply.raw
    );
    assert!(w.daemon.is_running(), "the daemon stopped");
}
