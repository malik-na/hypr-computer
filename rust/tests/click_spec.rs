//! #25: `click` lands where the agent aims it, on its own screen, and the
//! next screenshot shows the app's response. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::json;
use spec25::probe::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Event, MARK};
use spec25::{Agent, assert_changed};

/// Whether a pointer position the app got is pixel (x, y).
fn at(px: f64, py: f64, x: i64, y: i64) -> bool {
    px.floor() as i64 == x && py.floor() as i64 == y
}

#[test]
fn a_click_lands_where_it_was_aimed() {
    let mut a = Agent::with_probe();
    let mark = a.probe.mark();
    a.shim.ok("click", json!({"x": 321, "y": 123}));

    assert!(a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: false, .. }))
    }));
    let events = a.probe.settle(mark);
    let buttons: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Button {
                button,
                pressed,
                x,
                y,
                ..
            } => Some((*button, *pressed, *x, *y)),
            _ => None,
        })
        .collect();
    assert_eq!(buttons.len(), 2, "one press and one release: {events:?}");
    for (button, _, x, y) in &buttons {
        assert_eq!(
            *button, BTN_LEFT,
            "the default is the left button: {events:?}"
        );
        assert!(
            at(*x, *y, 321, 123),
            "landed at {x},{y}, not 321,123: {events:?}"
        );
    }
    assert!(
        buttons[0].1 && !buttons[1].1,
        "press, then release: {events:?}"
    );
}

/// 10 of 10 clicks land, edges and corners included, and each one shows on
/// the screenshot taken right after it.
#[test]
fn ten_of_ten_clicks_land_and_each_shows_on_the_next_screenshot() {
    let mut a = Agent::with_probe();
    let targets: [(i64, i64); 10] = [
        (0, 0),
        (1279, 0),
        (0, 799),
        (1279, 799),
        (640, 400),
        (17, 433),
        (1003, 61),
        (512, 777),
        (1270, 395),
        (333, 9),
    ];
    let mut missed = Vec::new();
    for (x, y) in targets {
        let mark = a.probe.mark();
        let reply = a.shim.call("click", json!({"x": x, "y": y}));
        let shot = a.shim.screenshot();
        let on_screen = shot.pixel(x as u32, y as u32) == MARK;
        a.probe.wait_for(mark, |e| {
            e.iter()
                .any(|e| matches!(e, Event::Button { pressed: true, .. }))
        });
        let presses = a.probe.presses_since(mark);
        let landed = matches!(presses.as_slice(), [(BTN_LEFT, px, py)] if at(*px, *py, x, y));
        if reply.is_error() || !landed || !on_screen {
            missed.push(format!(
                "({x},{y}): reply error {}, app got {presses:?}, screenshot there {:?}",
                reply.is_error(),
                shot.pixel(x as u32, y as u32)
            ));
        }
    }
    assert!(
        missed.is_empty(),
        "{} of 10 clicks missed:\n{}",
        missed.len(),
        missed.join("\n")
    );
}

#[test]
fn right_and_middle_clicks_press_those_buttons() {
    let mut a = Agent::with_probe();
    for (name, code) in [
        ("right", BTN_RIGHT),
        ("middle", BTN_MIDDLE),
        ("left", BTN_LEFT),
    ] {
        let mark = a.probe.mark();
        a.shim
            .ok("click", json!({"x": 200, "y": 300, "button": name}));
        a.probe.wait_for(mark, |e| {
            e.iter()
                .any(|e| matches!(e, Event::Button { pressed: false, .. }))
        });
        let presses = a.probe.presses_since(mark);
        assert!(
            matches!(presses.as_slice(), [(b, x, y)] if *b == code && at(*x, *y, 200, 300)),
            "a {name} click gave {presses:?}"
        );
    }
}

#[test]
fn a_double_and_a_triple_click_are_quick_presses_in_one_place() {
    let mut a = Agent::with_probe();
    for count in [2usize, 3] {
        let mark = a.probe.mark();
        a.shim
            .ok("click", json!({"x": 700, "y": 222, "count": count}));
        a.probe.wait_for(mark, |e| {
            e.iter()
                .filter(|e| matches!(e, Event::Button { pressed: false, .. }))
                .count()
                >= count
        });
        let events = a.probe.settle(mark);
        let presses: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Button {
                    button,
                    pressed: true,
                    x,
                    y,
                    time,
                } => Some((*button, *x, *y, *time)),
                _ => None,
            })
            .collect();
        assert_eq!(presses.len(), count, "count {count}: {events:?}");
        for (b, x, y, _) in &presses {
            assert!(
                *b == BTN_LEFT && at(*x, *y, 700, 222),
                "count {count}: {events:?}"
            );
        }
        // Apps take presses this close together as one double (or triple)
        // click; 400 ms is GTK's default limit.
        let span = presses[count - 1].3.wrapping_sub(presses[0].3);
        assert!(
            span < 400 * (count as u32 - 1),
            "count {count}: presses {span} ms apart"
        );
        let releases = events
            .iter()
            .filter(|e| matches!(e, Event::Button { pressed: false, .. }))
            .count();
        assert_eq!(
            releases, count,
            "count {count}: every press is released: {events:?}"
        );
    }
}

#[test]
fn a_screenshot_after_each_click_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    let mut before = a.shim.screenshot();
    for (i, (x, y)) in [(100, 100), (100, 100), (900, 650), (5, 795), (640, 400)]
        .into_iter()
        .enumerate()
    {
        a.shim.ok("click", json!({"x": x, "y": y}));
        before = assert_changed(&mut a.shim, &before, &format!("click {i} at {x},{y}"));
    }
}
