//! #28: `hypr-computer stop <agent>` stops one agent and leaves the others
//! running; `hypr-computer resume <agent>` lets it go on. Written from the
//! spec (specs/Stop.tla) alone; spec28/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec28;

use std::time::Duration;

use serde_json::json;
use spec28::probe::{BTN_LEFT, Event};
use spec28::{World, assert_stopped, held_motions, is_input};

#[test]
fn stopping_one_agent_by_name_leaves_the_others_running() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");

    world.user_runs(&["stop", "alpha"]);

    alpha.assert_stopped();
    beta.assert_serves(300, 200);
    beta.assert_serves(900, 650);
    alpha.assert_stopped();
}

#[test]
fn resuming_one_agent_by_name_leaves_the_others_stopped() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    world.user_runs(&["stop", "alpha"]);
    world.user_runs(&["stop", "beta"]);
    alpha.assert_stopped();
    beta.assert_stopped();

    world.user_runs(&["resume", "alpha"]);

    alpha.assert_serves(400, 300);
    beta.assert_stopped();

    world.user_runs(&["resume", "beta"]);
    beta.assert_serves(500, 500);
    alpha.assert_serves(600, 300);
}

/// Two agents of the same kind have the same name; the session number on
/// each one's live view tells them apart.
#[test]
fn an_agent_is_also_picked_by_its_session_number() {
    let mut world = World::new();
    let mut first = world.agent("claude-code");
    let mut second = world.agent("claude-code");
    assert_eq!((first.session, second.session), (1, 2));

    world.user_runs(&["stop", "2"]);

    second.assert_stopped();
    first.assert_serves(320, 240);

    world.user_runs(&["resume", "2"]);
    second.assert_serves(700, 500);
    first.assert_serves(100, 100);
}

/// Stopping is never less than the user asked for: a name stops every agent
/// that has it.
#[test]
fn stopping_by_name_stops_every_agent_of_that_name() {
    let mut world = World::new();
    let mut first = world.agent("claude-code");
    let mut other = world.agent("codex");
    let mut second = world.agent("claude-code");

    world.user_runs(&["stop", "claude-code"]);

    first.assert_stopped();
    second.assert_stopped();
    other.assert_serves(640, 400);
}

#[test]
fn stopping_an_agent_that_isnt_there_fails_and_stops_nothing() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");

    for args in [["stop", "gamma"], ["stop", "7"], ["resume", "gamma"]] {
        let ran = world.hypr_computer(&args);
        assert!(
            !ran.success,
            "`hypr-computer {}` must fail: there is no such agent; it said {ran:?}",
            args.join(" ")
        );
        // It says which agent it couldn't find, not how to use the command.
        assert!(
            ran.stderr.contains(args[1]) && !ran.stderr.contains("usage"),
            "`hypr-computer {}` must say there is no agent {:?}; it said {ran:?}",
            args.join(" "),
            args[1]
        );
    }

    alpha.assert_serves(200, 200);
    beta.assert_serves(800, 600);
}

/// `hypr-computer resume alpha` while every agent is stopped resumes alpha
/// alone (Stop.tla, Resume).
#[test]
fn resuming_one_agent_after_stopping_all_resumes_only_that_one() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    world.user_runs(&["stop"]);

    world.user_runs(&["resume", "alpha"]);

    alpha.assert_serves(250, 250);
    beta.assert_stopped();

    world.user_runs(&["resume"]);
    beta.assert_serves(750, 550);
}

#[test]
fn stopping_one_agent_mid_drag_releases_its_button_and_the_others_drag_goes_on() {
    let mut world = World::new();
    let mut alpha = world.slow_agent("alpha");
    let mut beta = world.slow_agent("beta");
    let (x1, x2, y) = (100, 1100, 400);
    let drag = json!({"x1": x1, "y1": y, "x2": x2, "y2": y});

    let (a_mark, b_mark) = (alpha.probe.mark(), beta.probe.mark());
    let a_drag = alpha.shim.begin("drag", drag.clone());
    let b_drag = beta.shim.begin("drag", drag);
    let pressed = |e: &[Event]| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: true, .. }))
    };
    assert!(
        alpha.probe.wait_for(a_mark, pressed),
        "alpha's drag never pressed"
    );
    assert!(
        beta.probe.wait_for(b_mark, pressed),
        "beta's drag never pressed"
    );

    world.user_runs(&["stop", "alpha"]);

    // alpha's drag stops where it was, its button released.
    let reply = alpha.shim.finish(a_drag, Duration::from_secs(30));
    assert_stopped(
        &reply,
        "alpha's drag in progress when the user stopped alpha",
    );
    let released = alpha.probe.wait_for(a_mark, |e| !held_motions(e).1);
    let events = alpha.probe.settle(a_mark);
    assert!(released, "alpha's button was left held: {events:?}");
    let (held, _) = held_motions(&events);
    let halfway = f64::from(x1 + x2) / 2.0;
    assert!(
        held.iter().all(|&(x, _)| x < halfway),
        "alpha's drag went on after the stop: {events:?}"
    );

    // beta's drag goes on to its end and lets go there.
    let reply = beta.shim.finish(b_drag, Duration::from_secs(60));
    assert!(
        !reply.is_error(),
        "beta wasn't stopped, but its drag failed: {}",
        reply.raw
    );
    let events = beta.probe.settle(b_mark);
    let buttons: Vec<(bool, i64, i64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Button {
                button: BTN_LEFT,
                pressed,
                x,
                y,
                ..
            } => Some((*pressed, x.floor() as i64, y.floor() as i64)),
            _ => None,
        })
        .collect();
    assert_eq!(
        buttons,
        vec![
            (true, i64::from(x1), i64::from(y)),
            (false, i64::from(x2), i64::from(y))
        ],
        "beta's drag must press at its start and release at its end: {events:?}"
    );

    let more: Vec<Event> = alpha
        .probe
        .settle(alpha.probe.mark())
        .into_iter()
        .filter(is_input)
        .collect();
    assert!(more.is_empty(), "the stopped agent got input: {more:?}");
}
