//! #25: `drag` presses the left button at one point, moves to another and
//! lets go there. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::json;
use spec25::probe::{BTN_LEFT, Event};
use spec25::{Agent, assert_changed};

fn px(v: f64) -> i64 {
    v.floor() as i64
}

/// Drags and checks what the app saw: a left press at the start, motion
/// with the button held that heads from the start to the end without
/// leaving the box between them, and a release at the end.
fn drag_and_check(a: &mut Agent, (x1, y1): (i64, i64), (x2, y2): (i64, i64)) {
    let mark = a.probe.mark();
    a.shim
        .ok("drag", json!({"x1": x1, "y1": y1, "x2": x2, "y2": y2}));
    a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: false, .. }))
    });
    let events = a.probe.settle(mark);
    let what = format!("drag {x1},{y1} -> {x2},{y2}: {events:?}");

    let buttons: Vec<(usize, u32, bool, i64, i64)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Event::Button {
                button,
                pressed,
                x,
                y,
                ..
            } => Some((i, *button, *pressed, px(*x), px(*y))),
            _ => None,
        })
        .collect();
    assert_eq!(buttons.len(), 2, "one press and one release; {what}");
    let (press, release) = (buttons[0], buttons[1]);
    assert_eq!((press.1, press.2), (BTN_LEFT, true), "{what}");
    assert_eq!((release.1, release.2), (BTN_LEFT, false), "{what}");
    assert_eq!(
        (press.3, press.4),
        (x1, y1),
        "pressed at the wrong place; {what}"
    );
    assert_eq!(
        (release.3, release.4),
        (x2, y2),
        "released at the wrong place; {what}"
    );

    let held: Vec<(i64, i64)> = events[press.0..release.0]
        .iter()
        .filter_map(|e| match e {
            Event::Motion { x, y } => Some((px(*x), px(*y))),
            _ => None,
        })
        .collect();
    assert!(
        !held.is_empty(),
        "the pointer must move while the button is held; {what}"
    );
    assert_eq!(held.last(), Some(&(x2, y2)), "{what}");
    let (lo_x, hi_x) = (x1.min(x2), x1.max(x2));
    let (lo_y, hi_y) = (y1.min(y2), y1.max(y2));
    let mut last = (x1, y1);
    for &(x, y) in &held {
        assert!(
            (lo_x..=hi_x).contains(&x) && (lo_y..=hi_y).contains(&y),
            "left the path at {x},{y}; {what}"
        );
        // Never back the way it came.
        assert!(
            (x - last.0) * (x2 - x1).signum() >= 0 && (y - last.1) * (y2 - y1).signum() >= 0,
            "went back at {x},{y}; {what}"
        );
        last = (x, y);
    }
}

#[test]
fn a_drag_presses_at_the_start_moves_and_releases_at_the_end() {
    let mut a = Agent::with_probe();
    drag_and_check(&mut a, (100, 100), (600, 400));
}

#[test]
fn a_drag_works_in_every_direction_and_to_the_edges() {
    let mut a = Agent::with_probe();
    drag_and_check(&mut a, (900, 700), (200, 50));
    drag_and_check(&mut a, (640, 400), (640, 0));
    drag_and_check(&mut a, (10, 400), (1279, 400));
    drag_and_check(&mut a, (1279, 799), (0, 0));
}

#[test]
fn the_button_is_up_after_a_drag() {
    let mut a = Agent::with_probe();
    a.shim
        .ok("drag", json!({"x1": 100, "y1": 100, "x2": 300, "y2": 300}));
    let mark = a.probe.mark();
    a.shim.ok("click", json!({"x": 500, "y": 500}));
    a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: false, .. }))
    });
    let presses = a.probe.presses_since(mark);
    assert!(
        matches!(presses.as_slice(), [(BTN_LEFT, x, y)] if px(*x) == 500 && px(*y) == 500),
        "a click after a drag is a fresh press: {presses:?}"
    );
}

#[test]
fn a_screenshot_after_each_drag_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    let mut before = a.shim.screenshot();
    for (x1, y1, x2, y2) in [
        (100, 100, 400, 300),
        (400, 300, 100, 100),
        (50, 700, 1200, 700),
    ] {
        a.shim
            .ok("drag", json!({"x1": x1, "y1": y1, "x2": x2, "y2": y2}));
        before = assert_changed(
            &mut a.shim,
            &before,
            &format!("drag {x1},{y1} -> {x2},{y2}"),
        );
    }
}
