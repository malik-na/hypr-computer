//! #30: Escape pressed twice in a row in the view hands the screen back. The
//! first Escape goes to the agent's app, so Escape still works there; the
//! second goes to no one. Any other key or a click between them starts the
//! count again. The interface these tests assume is at the top of
//! tests/spec30/mod.rs.
#![forbid(unsafe_code)]

mod spec30;
mod support;
mod view_support;

use serde_json::json;

use spec30::probe::presses;
use spec30::{TIMEOUT, WAITS, World, inputs, keys_pressed, one_press_at};

#[test]
fn escape_twice_hands_back_and_only_the_first_escape_reaches_the_app() {
    let mut w = World::new();
    let mut agent = w.agent("escape-agent");
    w.take_over(&agent, 640.0, 400.0);
    let id = agent.begin("click", json!({"x": 100, "y": 100}));
    agent.assert_waits(id, WAITS, "a click");

    let mark = agent.probe().mark();
    w.user_presses(1, &["Escape", "Escape"]);
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "after Escape twice the waiting click must run, and say the user took over: {}",
        reply.raw
    );
    agent.probe().wait_for(mark, |e| !presses(e).is_empty());
    let got = agent.probe().settle(mark);
    assert!(
        one_press_at(&presses(&got), 100.0, 100.0),
        "the waiting click must land once, at (100, 100): {got:?}"
    );
    assert_eq!(
        keys_pressed(&got),
        ["Escape"],
        "the first Escape must reach the agent's app, and the second must not"
    );
    let out = w.hypr_computer(&["give"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "after Escape twice there is nothing left to give back"
    );
}

#[test]
fn a_key_or_click_between_the_escapes_starts_the_count_again() {
    let mut w = World::new();
    let mut agent = w.agent("escape-agent");
    w.take_over(&agent, 640.0, 400.0);
    let id = agent.begin("screenshot", json!({}));

    let mark = agent.probe().mark();
    w.user_presses(1, &["Escape", "a", "Escape"]);
    agent.assert_waits(id, WAITS, "after Escape, a, Escape, a screenshot");
    w.user_clicks(1, 500.0, 500.0);
    w.user_presses(1, &["Escape"]);
    agent.assert_waits(id, WAITS, "after Escape, a click, Escape, a screenshot");
    let got = agent.probe().since(mark);
    assert_eq!(
        keys_pressed(&got),
        ["Escape", "a", "Escape", "Escape"],
        "while the user has the screen, their keys must all reach the agent's app"
    );
    assert!(
        one_press_at(&presses(&got), 500.0, 500.0),
        "the user's click must reach the agent's app: {got:?}"
    );

    // Now two in a row.
    let mark = agent.probe().mark();
    w.user_presses(1, &["Escape"]);
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "Escape twice in a row must hand back: {}",
        reply.raw
    );
    assert!(
        keys_pressed(&agent.probe().settle(mark)).is_empty(),
        "the Escape that hands back reached the agent's app"
    );
}

#[test]
fn after_escape_twice_the_view_is_watch_only_again_until_the_next_click() {
    let mut w = World::new();
    let mut agent = w.agent("escape-agent");
    w.take_over(&agent, 640.0, 400.0);
    let id = agent.begin("screenshot", json!({}));
    w.user_presses(1, &["Escape", "Escape"]);
    let reply = agent.finish(id, TIMEOUT);
    assert!(!reply.is_error(), "{}", reply.raw);

    let mark = agent.probe().mark();
    w.user_hovers(1, 300.0, 300.0);
    w.user_presses(1, &["x", "Escape"]);
    let got = agent.probe().settle(mark);
    assert!(
        inputs(&got).is_empty(),
        "after the hand-back the user's pointer and keys reached the agent's app: {got:?}"
    );

    // A click takes over again.
    let mark = agent.probe().mark();
    w.user_clicks(1, 640.0, 400.0);
    w.wait_taken_over(&agent, 640.0, 400.0);
    let id = agent.begin("click", json!({"x": 200, "y": 200}));
    agent.assert_waits(id, WAITS, "a click after the second take-over");
    w.user_gives();
    let reply = agent.finish(id, TIMEOUT);
    assert!(
        !reply.is_error() && reply.says_taken_over(),
        "{}",
        reply.raw
    );
    assert!(
        agent
            .probe()
            .since(mark)
            .iter()
            .all(|e| !matches!(e, spec30::probe::Event::Key { .. })),
        "the user's keys after the hand-back reached the agent's app"
    );
}
