//! #28: `hypr-computer stop` stops every agent, and `hypr-computer resume`
//! lets them go on. Written from the spec (specs/Stop.tla) alone; spec28/
//! says what interface it assumes.
#![forbid(unsafe_code)]

mod spec28;

use std::thread;
use std::time::Duration;

use serde_json::json;
use spec28::probe::{BTN_LEFT, Event, all_keys_released};
use spec28::{World, arguments, assert_stopped, held_motions, is_input, running};

#[test]
fn stop_refuses_every_tool_screenshots_included_and_tells_the_agent_to_ask() {
    let mut world = World::new();
    let mut a = world.agent("claude-code");
    let tools = a.shim.tools();
    assert!(
        tools.iter().any(|(name, _)| name == "screenshot"),
        "no screenshot tool: {tools:?}"
    );

    world.user_runs(&["stop"]);

    let mark = a.probe.mark();
    for (name, schema) in &tools {
        let args = arguments(&world.env, name, schema);
        let reply = a.shim.call(name, args.clone());
        assert_stopped(&reply, &format!("{name} {args}"));
    }
    let got: Vec<Event> = a.probe.settle(mark).into_iter().filter(is_input).collect();
    assert!(
        got.is_empty(),
        "refused calls still gave the screen input: {got:?}"
    );
    assert!(
        !world.env.path("launched.log").exists(),
        "a refused launch_app still started the app"
    );
}

#[test]
fn stop_stops_every_agent() {
    let mut world = World::new();
    let mut a = world.agent("claude-code");
    let mut b = world.agent("codex");
    let mut c = world.agent("claude-code");

    world.user_runs(&["stop"]);

    a.assert_stopped();
    b.assert_stopped();
    c.assert_stopped();
}

#[test]
fn resume_lets_every_agent_go_on_with_every_tool() {
    let mut world = World::new();
    let mut a = world.agent("claude-code");
    let mut b = world.agent("codex");
    world.user_runs(&["stop"]);
    a.assert_stopped();
    b.assert_stopped();

    world.user_runs(&["resume"]);

    a.assert_serves(200, 300);
    b.assert_serves(900, 600);
    // Every tool works again; launch_app goes last, as its app covers the
    // probe.
    let mut tools = a.shim.tools();
    tools.sort_by_key(|(name, _)| name == "launch_app");
    for (name, schema) in &tools {
        let args = arguments(&world.env, name, schema);
        let reply = a.shim.call(name, args.clone());
        assert!(
            !reply.text().contains("Stopped by the user"),
            "{name} {args} is still refused after resume: {}",
            reply.raw
        );
        if spec28_knows(name) {
            assert!(
                !reply.is_error(),
                "{name} {args} failed after resume: {}",
                reply.raw
            );
        }
    }
}

/// Whether `arguments` has real arguments for `tool`, so it must work.
fn spec28_knows(tool: &str) -> bool {
    [
        "screenshot",
        "click",
        "move",
        "scroll",
        "type",
        "key",
        "drag",
        "launch_app",
    ]
    .contains(&tool)
}

#[test]
fn a_drag_in_progress_ends_with_the_button_released() {
    let mut world = World::new();
    let mut a = world.slow_agent("claude-code");
    let (x1, x2, y) = (100, 1100, 400);

    let mark = a.probe.mark();
    let drag = a
        .shim
        .begin("drag", json!({"x1": x1, "y1": y, "x2": x2, "y2": y}));
    let pressed = a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: true, .. }))
    });
    assert!(pressed, "the drag never pressed: {:?}", a.probe.since(mark));

    world.user_runs(&["stop"]);

    let reply = a.shim.finish(drag, Duration::from_secs(30));
    assert_stopped(&reply, "the drag in progress when the user stopped");
    let released = a.probe.wait_for(mark, |e| !held_motions(e).1);
    let events = a.probe.settle(mark);
    let what = format!("events since the drag began: {events:?}");
    assert!(released, "the button was left held; {what}");

    let buttons: Vec<(usize, bool)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::Button {
                button: BTN_LEFT,
                pressed,
                ..
            } => Some((i, *pressed)),
            _ => None,
        })
        .collect();
    assert!(
        matches!(buttons.as_slice(), [(_, true), (_, false)]),
        "one press, then its release; {what}"
    );
    let (held, _) = held_motions(&events);
    let halfway = f64::from(x1 + x2) / 2.0;
    assert!(
        held.iter().all(|&(x, _)| x < halfway),
        "the drag went on after the stop; {what}"
    );
    let after: Vec<&Event> = events[buttons[1].0 + 1..]
        .iter()
        .filter(|e| is_input(e))
        .collect();
    assert!(after.is_empty(), "input after the release: {after:?}");

    // Nothing more comes, and once resumed, a click is a fresh press.
    let later = a.probe.mark();
    thread::sleep(Duration::from_secs(1));
    let more: Vec<Event> = a.probe.since(later).into_iter().filter(is_input).collect();
    assert!(more.is_empty(), "the stopped drag went on: {more:?}");
    world.user_runs(&["resume"]);
    a.assert_serves(640, 700);
}

#[test]
fn typing_in_progress_stops_with_no_key_left_down() {
    let mut world = World::new();
    let mut a = world.slow_agent("claude-code");
    a.probe.wait_keyboard_focus();
    let text = "x".repeat(2000);

    let mark = a.probe.mark();
    let typing = a.shim.begin("type", json!({"text": text}));
    let started = a
        .probe
        .wait_for(mark, |e| !spec28::probe::typed(e).is_empty());
    assert!(started, "typing never started: {:?}", a.probe.since(mark));

    world.user_runs(&["stop"]);

    let reply = a.shim.finish(typing, Duration::from_secs(60));
    assert_stopped(&reply, "the typing in progress when the user stopped");
    thread::sleep(Duration::from_secs(1));
    let events = a.probe.settle(mark);
    let typed = spec28::probe::typed(&events);
    assert!(
        typed.len() < text.len(),
        "all {} characters were typed despite the stop",
        text.len()
    );
    assert!(
        all_keys_released(&events),
        "a key was left down: {:?}",
        events.iter().rev().take(20).collect::<Vec<_>>()
    );
    let later = a.probe.mark();
    thread::sleep(Duration::from_secs(1));
    let more: Vec<Event> = a.probe.since(later).into_iter().filter(is_input).collect();
    assert!(more.is_empty(), "the stopped typing went on: {more:?}");
}

#[test]
fn a_stopped_agents_screen_stays() {
    let mut world = World::new();
    let mut a = world.agent("claude-code");
    let screen = world.daemon.screen_pid(a.session);
    let before = a.shim.screenshot();

    world.user_runs(&["stop"]);
    a.assert_stopped();
    thread::sleep(Duration::from_secs(1));

    assert!(
        running(screen),
        "the stopped agent's screen process is gone"
    );
    assert!(
        world.env.screen_socket(a.session).exists(),
        "the stopped agent's screen no longer takes apps"
    );
    let lost: Vec<Event> = a
        .probe
        .events()
        .into_iter()
        .filter(|e| matches!(e, Event::Disconnected(_)))
        .collect();
    assert!(
        lost.is_empty(),
        "the app on the stopped agent's screen lost it: {lost:?}"
    );

    world.user_runs(&["resume"]);
    let after = a.shim.screenshot();
    assert_eq!(
        after.pixel(640, 400),
        before.pixel(640, 400),
        "the screen after resume doesn't show the app it showed before the stop"
    );
    a.assert_serves(640, 400);
}

/// Stop-all holds for agents that connect while it is in force: an agent
/// can't get out of a stop by reconnecting (Stop.tla,
/// StopAllHoldsForNewAgents).
#[test]
fn an_agent_that_reconnects_while_stopped_is_still_stopped() {
    let mut world = World::new();
    let mut a = world.agent("claude-code");
    world.user_runs(&["stop"]);
    a.assert_stopped();
    a.shim.close_stdin();
    assert!(a.shim.wait_exit(), "the shim didn't exit");
    drop(a);

    let (mut again, _) = world.connect("claude-code");
    let reply = again.screenshot_once_started();
    assert_stopped(&reply, "the reconnected agent's screenshot");

    world.user_runs(&["resume"]);
    again.screenshot();
}
