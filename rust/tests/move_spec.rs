//! #25: `move` puts the agent's pointer at a point without clicking, e.g. to
//! hover. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::json;
use spec25::probe::{BTN_LEFT, Event};
use spec25::{Agent, assert_changed};

/// The last pointer position the app was told of, if any.
fn last_position(events: &[Event]) -> Option<(i64, i64)> {
    events.iter().rev().find_map(|e| match e {
        Event::Motion { x, y } | Event::PointerEnter { x, y } => {
            Some((x.floor() as i64, y.floor() as i64))
        }
        _ => None,
    })
}

#[test]
fn move_puts_the_pointer_there_without_clicking() {
    let mut a = Agent::with_probe();
    for (x, y) in [(700, 300), (0, 0), (1279, 799), (64, 512)] {
        let mark = a.probe.mark();
        a.shim.ok("move", json!({"x": x, "y": y}));
        let arrived = a.probe.wait_for(mark, |e| last_position(e) == Some((x, y)));
        let events = a.probe.settle(mark);
        assert!(arrived, "the pointer never got to {x},{y}: {events:?}");
        assert!(
            !events.iter().any(|e| matches!(e, Event::Button { .. })),
            "move must not click: {events:?}"
        );
    }
}

#[test]
fn a_click_after_a_hover_lands_at_the_click_not_the_hover() {
    let mut a = Agent::with_probe();
    a.shim.ok("move", json!({"x": 50, "y": 60}));
    let mark = a.probe.mark();
    a.shim.ok("click", json!({"x": 800, "y": 500}));
    a.probe.wait_for(mark, |e| {
        e.iter().any(|e| matches!(e, Event::Button { .. }))
    });
    let presses = a.probe.presses_since(mark);
    assert!(
        matches!(presses.as_slice(), [(BTN_LEFT, x, y)] if x.floor() == 800.0 && y.floor() == 500.0),
        "{presses:?}"
    );
}

#[test]
fn a_screenshot_after_each_move_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    let mut before = a.shim.screenshot();
    for (x, y) in [(10, 10), (1000, 700), (640, 400)] {
        a.shim.ok("move", json!({"x": x, "y": y}));
        before = assert_changed(&mut a.shim, &before, &format!("move to {x},{y}"));
    }
}
