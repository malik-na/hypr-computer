//! #25: bad arguments to the input tools get a clear error, not a crash, and
//! do nothing. Written from the spec alone.
//!
//! "Clear" here: the agent is told it failed, and the message names the
//! argument at fault or the bad value it gave. Coordinates off the 1280x800
//! screen count as bad arguments.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::{Value, json};
use spec25::{Agent, is_input, mentions};

/// Makes each call and returns a line per case that went wrong.
fn check(a: &mut Agent, tool: &str, cases: &[(Value, &[&str])]) {
    a.probe.wait_keyboard_focus();
    let mut wrong = Vec::new();
    for (args, words) in cases {
        let mark = a.probe.mark();
        let reply = a.shim.call(tool, args.clone());
        let text = reply.text();
        if !reply.is_error() {
            wrong.push(format!("{tool} {args}: no error: {}", reply.raw));
        } else if !words.iter().any(|w| mentions(&text, w)) {
            wrong.push(format!(
                "{tool} {args}: the error names none of {words:?}: {text:?}"
            ));
        }
        let events = a.probe.settle(mark);
        if events.iter().any(is_input) {
            wrong.push(format!(
                "{tool} {args}: the app got input anyway: {events:?}"
            ));
        }
        if !a.daemon.is_running() {
            panic!("{tool} {args} crashed the daemon:\n{}", a.daemon.log());
        }
    }
    assert_eq!(
        a.shim.screenshot().width,
        1280,
        "the session should carry on"
    );
    assert!(
        wrong.is_empty(),
        "{} of {} bad calls:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

#[test]
fn bad_click_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "click",
        &[
            (json!({}), &["x", "y"]),
            (json!({"y": 5}), &["x"]),
            (json!({"x": 5}), &["y"]),
            (json!({"x": "ten", "y": 5}), &["x", "ten"]),
            (json!({"x": 1.5, "y": 5}), &["x", "1.5"]),
            (json!({"x": -1, "y": 5}), &["x", "-1"]),
            (json!({"x": 1280, "y": 5}), &["x", "1280"]),
            (json!({"x": 5, "y": 800}), &["y", "800"]),
            (
                json!({"x": 5, "y": 5, "button": "side"}),
                &["button", "side"],
            ),
            (json!({"x": 5, "y": 5, "count": 0}), &["count", "0"]),
            (json!({"x": 5, "y": 5, "count": 4}), &["count", "4"]),
        ],
    );
}

#[test]
fn bad_move_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "move",
        &[
            (json!({"x": 5}), &["y"]),
            (json!({"x": 99999, "y": 5}), &["x", "99999"]),
            (json!({"x": 5, "y": -3}), &["y", "-3"]),
        ],
    );
}

#[test]
fn bad_type_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "type",
        &[
            (json!({}), &["text"]),
            (json!({"text": 42}), &["text", "42"]),
        ],
    );
}

#[test]
fn bad_key_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "key",
        &[
            (json!({}), &["keys"]),
            (json!({"keys": ""}), &["keys"]),
            (json!({"keys": "ctrl+nosuchkey"}), &["nosuchkey"]),
            (json!({"keys": "ctrl+"}), &["keys", "ctrl+"]),
        ],
    );
}

#[test]
fn bad_scroll_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "scroll",
        &[
            (
                json!({"x": 5, "y": 5, "direction": "diagonal"}),
                &["direction", "diagonal"],
            ),
            (json!({"x": 5, "y": 5, "amount": 0}), &["amount", "0"]),
            (json!({"x": 5, "y": 5, "amount": 51}), &["amount", "51"]),
            (json!({"x": 2000, "y": 5}), &["x", "2000"]),
        ],
    );
}

#[test]
fn bad_drag_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "drag",
        &[
            (json!({"x1": 1, "y1": 1, "x2": 5}), &["y2"]),
            (
                json!({"x1": 1, "y1": 1, "x2": 5000, "y2": 5}),
                &["x2", "5000"],
            ),
            (json!({"x1": -5, "y1": 1, "x2": 5, "y2": 5}), &["x1", "-5"]),
        ],
    );
}

#[test]
fn bad_launch_app_arguments() {
    let mut a = Agent::with_probe();
    check(
        &mut a,
        "launch_app",
        &[
            (json!({}), &["app"]),
            (json!({"app": ""}), &["app"]),
            (json!({"app": "foot", "args": "-e sh"}), &["args", "-e sh"]),
            (json!({"app": "hc25-no-such-app"}), &["hc25-no-such-app"]),
        ],
    );
}
