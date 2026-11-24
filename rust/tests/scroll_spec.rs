//! #25: `scroll` turns the agent's mouse wheel over a point, in the
//! direction asked, by the notches asked. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::{Value, json};
use spec25::probe::Event;
use spec25::{Agent, assert_changed};

/// What one scroll did: (vertical total, horizontal total, wheel total in
/// 1/120ths of a notch (vertical, horizontal), where each axis event was).
struct Scrolled {
    vertical: f64,
    horizontal: f64,
    wheel: (i32, i32),
    at: Vec<(i64, i64)>,
    events: Vec<Event>,
}

fn scroll(a: &mut Agent, args: Value) -> Scrolled {
    let mark = a.probe.mark();
    a.shim.ok("scroll", args.clone());
    a.probe
        .wait_for(mark, |e| e.iter().any(|e| matches!(e, Event::Axis { .. })));
    let events = a.probe.settle(mark);
    let mut s = Scrolled {
        vertical: 0.0,
        horizontal: 0.0,
        wheel: (0, 0),
        at: Vec::new(),
        events: events.clone(),
    };
    for e in &events {
        match e {
            Event::Axis {
                horizontal,
                value,
                x,
                y,
            } => {
                if *horizontal {
                    s.horizontal += value;
                } else {
                    s.vertical += value;
                }
                s.at.push((x.floor() as i64, y.floor() as i64));
            }
            Event::Wheel {
                horizontal,
                value120,
            } => {
                if *horizontal {
                    s.wheel.1 += value120;
                } else {
                    s.wheel.0 += value120;
                }
            }
            _ => {}
        }
    }
    s
}

/// Wayland's axes: positive vertical is down, positive horizontal is right.
#[test]
fn each_direction_scrolls_that_way_over_the_point() {
    let mut a = Agent::with_probe();
    let directions: [(&str, f64, f64); 4] = [
        ("down", 1.0, 0.0),
        ("up", -1.0, 0.0),
        ("right", 0.0, 1.0),
        ("left", 0.0, -1.0),
    ];
    for (direction, sign_v, sign_h) in directions {
        let s = scroll(&mut a, json!({"x": 400, "y": 300, "direction": direction}));
        let ok = s.vertical.signum() * sign_v.abs() == sign_v
            && s.horizontal.signum() * sign_h.abs() == sign_h
            && (sign_v == 0.0 || s.horizontal == 0.0)
            && (sign_h == 0.0 || s.vertical == 0.0);
        assert!(
            ok,
            "{direction}: vertical {}, horizontal {}: {:?}",
            s.vertical, s.horizontal, s.events
        );
        assert!(
            !s.at.is_empty() && s.at.iter().all(|p| *p == (400, 300)),
            "{direction}: scrolled at {:?}",
            s.at
        );
        assert!(
            !s.events.iter().any(|e| matches!(e, Event::Button { .. })),
            "scrolling must not click: {:?}",
            s.events
        );
    }
}

#[test]
fn the_default_is_down() {
    let mut a = Agent::with_probe();
    let s = scroll(&mut a, json!({"x": 640, "y": 400}));
    assert!(s.vertical > 0.0 && s.horizontal == 0.0, "{:?}", s.events);
}

/// Twice the notches scroll twice as far.
#[test]
fn the_amount_scales_the_scroll() {
    let mut a = Agent::with_probe();
    let three = scroll(
        &mut a,
        json!({"x": 640, "y": 400, "direction": "down", "amount": 3}),
    );
    let six = scroll(
        &mut a,
        json!({"x": 640, "y": 400, "direction": "down", "amount": 6}),
    );
    let one = scroll(
        &mut a,
        json!({"x": 640, "y": 400, "direction": "up", "amount": 1}),
    );
    let ratio = six.vertical / three.vertical;
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "6 notches vs 3: {} vs {}",
        six.vertical,
        three.vertical
    );
    let ratio = -three.vertical / one.vertical;
    assert!(
        (ratio - 3.0).abs() < 0.15,
        "3 notches down vs 1 up: {} vs {}",
        three.vertical,
        one.vertical
    );
}

/// Apps scroll by lines per wheel notch only when told it is a wheel, in
/// whole notches (axis_discrete, or axis_value120 in 120ths of one).
#[test]
fn the_wheel_turns_by_whole_notches() {
    let mut a = Agent::with_probe();
    for (args, expected) in [
        (
            json!({"x": 640, "y": 400, "direction": "down", "amount": 3}),
            (360, 0),
        ),
        (
            json!({"x": 640, "y": 400, "direction": "up", "amount": 2}),
            (-240, 0),
        ),
        (
            json!({"x": 640, "y": 400, "direction": "right", "amount": 1}),
            (0, 120),
        ),
        (
            json!({"x": 640, "y": 400, "direction": "left", "amount": 5}),
            (0, -600),
        ),
    ] {
        let s = scroll(&mut a, args.clone());
        assert_eq!(s.wheel, expected, "{args}: {:?}", s.events);
    }
}

#[test]
fn a_screenshot_after_each_scroll_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    let mut before = a.shim.screenshot();
    for direction in ["down", "up", "left", "right"] {
        a.shim.ok(
            "scroll",
            json!({"x": 640, "y": 400, "direction": direction, "amount": 1}),
        );
        before = assert_changed(&mut a.shim, &before, &format!("scrolling {direction}"));
    }
}
