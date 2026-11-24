//! #34: the daemon logs every action of every agent, and
//! `hypr-computer log <agent>` shows it. Written from the spec alone;
//! spec34/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec34;

use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use spec34::probe::Event;
use spec34::{Record, World, assert_stopped, has_clock_time, lines_with};

/// What a record must say: tool, arguments, result.
fn expect(tool: &str, arguments: Value, result: &str) -> (String, Value, String) {
    (tool.to_owned(), arguments, result.to_owned())
}

#[track_caller]
fn assert_records(got: &[Record], expected: &[(String, Value, String)]) {
    let got_short: Vec<(String, Value, String)> = got
        .iter()
        .map(|r| {
            (
                r.tool().to_owned(),
                r.arguments().clone(),
                r.result().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        got_short, expected,
        "the log must hold each call, in order, with its tool, arguments and result"
    );
}

#[test]
fn every_call_is_logged_with_its_tool_arguments_result_and_time() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);
    let begun = SystemTime::now() - Duration::from_secs(1);

    a.shim.ok("click", json!({"x": 640, "y": 400}));
    a.shim.ok("move", json!({"x": 100, "y": 200}));
    a.shim.ok("key", json!({"keys": "ctrl+l"}));
    a.shim.ok("screenshot", json!({}));
    let bad = a.shim.call("click", json!({"x": 640}));
    assert!(bad.is_error(), "a click without y must fail: {}", bad.raw);
    let ended = SystemTime::now() + Duration::from_secs(1);

    let all = world.wait_records(1, start + 5);
    let got = &all[start..];
    assert_records(
        got,
        &[
            expect("click", json!({"x": 640, "y": 400}), "ok"),
            expect("move", json!({"x": 100, "y": 200}), "ok"),
            expect("key", json!({"keys": "ctrl+l"}), "ok"),
            expect("screenshot", json!({}), "ok"),
            expect("click", json!({"x": 640}), "invalid"),
        ],
    );
    let mut last = begun;
    for r in got {
        assert_eq!((r.session(), r.agent()), (1, "alpha"), "{:?}", r.value);
        let t = r.time();
        assert!(
            t >= last && t <= ended,
            "each record's time must be when its call came in, in order: {:?}",
            r.value
        );
        last = t;
    }
}

#[test]
fn calls_refused_because_the_user_stopped_the_agent_are_logged() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);

    world.user_runs(&["stop", "alpha"]);
    a.assert_stopped(); // a screenshot, then a click at (640, 400)
    let key = a.shim.call("key", json!({"keys": "ctrl+l"}));
    assert_stopped(&key, "the stopped agent's key press");
    world.user_runs(&["resume", "alpha"]);
    a.shim.ok("click", json!({"x": 10, "y": 20}));

    let all = world.wait_records(1, start + 4);
    assert_records(
        &all[start..],
        &[
            expect("screenshot", json!({}), "stopped"),
            expect("click", json!({"x": 640, "y": 400}), "stopped"),
            expect("key", json!({"keys": "ctrl+l"}), "stopped"),
            expect("click", json!({"x": 10, "y": 20}), "ok"),
        ],
    );
}

#[test]
fn a_call_ended_by_a_stop_is_logged_as_stopped() {
    let mut world = World::new();
    let mut a = world.slow_agent("alpha");
    let start = world.after_setup(1);
    let args = json!({"x1": 100, "y1": 400, "x2": 1100, "y2": 400});

    let mark = a.probe.mark();
    let drag = a.shim.begin("drag", args.clone());
    let pressed = a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Button { pressed: true, .. }))
    });
    assert!(pressed, "the drag never pressed: {:?}", a.probe.since(mark));
    world.user_runs(&["stop"]);
    let reply = a.shim.finish(drag, Duration::from_secs(30));
    assert_stopped(&reply, "the drag in progress when the user stopped");

    let all = world.wait_records(1, start + 1);
    assert_records(&all[start..], &[expect("drag", args, "stopped")]);
}

#[test]
fn each_agent_has_a_log_of_its_own() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    let mut beta = world.agent("beta");
    let (a0, b0) = (world.after_setup(1), world.after_setup(2));

    alpha.shim.ok("click", json!({"x": 111, "y": 222}));
    beta.shim.ok("click", json!({"x": 777, "y": 555}));
    alpha.shim.ok("move", json!({"x": 121, "y": 232}));
    beta.shim.ok("move", json!({"x": 787, "y": 565}));

    let a = world.wait_records(1, a0 + 2);
    let b = world.wait_records(2, b0 + 2);
    assert_records(
        &a[a0..],
        &[
            expect("click", json!({"x": 111, "y": 222}), "ok"),
            expect("move", json!({"x": 121, "y": 232}), "ok"),
        ],
    );
    assert_records(
        &b[b0..],
        &[
            expect("click", json!({"x": 777, "y": 555}), "ok"),
            expect("move", json!({"x": 787, "y": 565}), "ok"),
        ],
    );
    let (fa, fb) = (world.session_file(1), world.session_file(2));
    assert_ne!(fa, fb, "each session's records go to a file of their own");
    for r in world.records() {
        let whose = (r.session(), r.agent().to_owned());
        if r.file == fa {
            assert_eq!(
                whose,
                (1, "alpha".to_owned()),
                "in alpha's file: {:?}",
                r.value
            );
        }
        if r.file == fb {
            assert_eq!(
                whose,
                (2, "beta".to_owned()),
                "in beta's file: {:?}",
                r.value
            );
        }
    }

    let alphas = [("111", "222"), ("121", "232")];
    let betas = [("777", "555"), ("787", "565")];
    let shown = world.user_runs(&["log", "alpha"]).stdout;
    for (x, y) in alphas {
        assert!(
            shows(&shown, x, y),
            "alpha's log must show ({x}, {y}):\n{shown}"
        );
    }
    for (x, y) in betas {
        assert!(
            !shows(&shown, x, y),
            "alpha's log shows beta's ({x}, {y}):\n{shown}"
        );
    }
    assert!(!shown.contains("beta"), "alpha's log shows beta:\n{shown}");
    let shown = world.user_runs(&["log", "beta"]).stdout;
    for (x, y) in betas {
        assert!(
            shows(&shown, x, y),
            "beta's log must show ({x}, {y}):\n{shown}"
        );
    }
    for (x, y) in alphas {
        assert!(
            !shows(&shown, x, y),
            "beta's log shows alpha's ({x}, {y}):\n{shown}"
        );
    }
    assert!(!shown.contains("alpha"), "beta's log shows alpha:\n{shown}");
}

/// Whether a line of `shown` holds both `x` and `y` (a call at that point).
fn shows(shown: &str, x: &str, y: &str) -> bool {
    shown.lines().any(|l| l.contains(x) && l.contains(y))
}

/// The first line of `shown` that holds both `x` and `y`.
fn line_of(shown: &str, x: &str, y: &str) -> Option<usize> {
    shown.lines().position(|l| l.contains(x) && l.contains(y))
}

/// Finds, in order, a line for each expected call: it names the tool, holds
/// each of `parts`, the result word, and a time of day.
#[track_caller]
fn assert_lines_in_order(shown: &str, expected: &[(&str, &[&str], &str)]) {
    let lines: Vec<&str> = shown.lines().collect();
    let mut at = 0;
    for (tool, parts, result) in expected {
        let found = lines[at..].iter().position(|l| {
            !lines_with(l, tool).is_empty()
                && !lines_with(l, result).is_empty()
                && parts.iter().all(|p| l.contains(p))
                && has_clock_time(l)
        });
        let Some(i) = found else {
            panic!(
                "after line {at}, `hypr-computer log` must show a line with the time, {tool}, \
                 {parts:?} and {result}:\n{shown}"
            );
        };
        at += i + 1;
    }
}

#[test]
fn the_log_command_shows_one_line_per_call_in_order() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    a.probe.wait_keyboard_focus();
    let text = "the quick brown fox ran";
    assert_eq!(text.chars().count(), 23);

    a.shim.ok("click", json!({"x": 640, "y": 400}));
    let mark = a.probe.mark();
    a.shim.ok("type", json!({"text": text}));
    assert_eq!(a.probe.wait_typed(mark, text), text);
    a.shim.ok("key", json!({"keys": "ctrl+l"}));
    world.user_runs(&["stop", "alpha"]);
    let refused = a.shim.call("click", json!({"x": 320, "y": 240}));
    assert_stopped(&refused, "the stopped agent's click");
    world.wait_records(1, 1);

    let shown = world.user_runs(&["log", "alpha"]).stdout;
    let header = shown.lines().find(|l| l.contains("alpha"));
    assert!(
        header.is_some_and(|l| l.contains("session 1")),
        "the log starts the session with a line naming it, \"alpha (session 1)\":\n{shown}"
    );
    assert_lines_in_order(
        &shown,
        &[
            ("click", &["640", "400"], "ok"),
            ("type", &["23 characters"], "ok"),
            ("key", &["ctrl+l"], "ok"),
            ("click", &["320", "240"], "stopped"),
        ],
    );
    assert!(
        !shown.contains("quick brown"),
        "the log shows what the agent typed:\n{shown}"
    );
}

/// Two agents of the same kind have the same name; the session number on
/// each one's live view tells them apart.
#[test]
fn the_log_command_picks_an_agent_by_session_number_too() {
    let mut world = World::new();
    let mut first = world.agent("claude-code");
    let mut second = world.agent("claude-code");
    first.shim.ok("click", json!({"x": 111, "y": 222}));
    second.shim.ok("click", json!({"x": 777, "y": 555}));
    world.wait_records(1, 2);
    world.wait_records(2, 2);

    let shown = world.user_runs(&["log", "2"]).stdout;
    assert!(
        shows(&shown, "777", "555") && shown.contains("session 2"),
        "`log 2` must show session 2:\n{shown}"
    );
    assert!(
        !shows(&shown, "111", "222") && !shown.contains("session 1"),
        "`log 2` must show session 2 only:\n{shown}"
    );

    let shown = world.user_runs(&["log", "claude-code"]).stdout;
    let (one, two) = (line_of(&shown, "111", "222"), line_of(&shown, "777", "555"));
    assert!(
        matches!((one, two), (Some(a), Some(b)) if a < b),
        "`log claude-code` must show both sessions of that name, oldest first:\n{shown}"
    );
}

#[test]
fn asking_for_an_agent_with_no_log_is_an_error() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    a.shim.ok("click", json!({"x": 640, "y": 400}));
    world.wait_records(1, 2);
    world.user_runs(&["log", "alpha"]);

    let ran = world.hypr_computer(&["log", "nobody"]);
    assert!(
        !ran.success && ran.stderr.contains("nobody"),
        "`log nobody` must fail, saying there is no log of \"nobody\": {ran:?}"
    );
    assert!(
        !shows(&ran.stdout, "640", "400"),
        "`log nobody` showed another agent's log: {ran:?}"
    );
}

#[test]
fn the_log_outlives_its_session_and_the_daemon() {
    let mut world = World::new();
    {
        let mut a = world.agent("alpha");
        a.shim.ok("click", json!({"x": 333, "y": 444}));
        world.wait_records(1, 2);
        a.shim.close_stdin();
        assert!(a.shim.wait_exit(), "the shim didn't exit");
    }
    let shown = world.user_runs(&["log", "alpha"]).stdout;
    assert!(
        shows(&shown, "333", "444"),
        "an ended session's log must still be shown:\n{shown}"
    );

    world.restart_daemon();
    let shown = world.user_runs(&["log", "alpha"]).stdout;
    assert!(
        shows(&shown, "333", "444"),
        "the log must outlive the daemon:\n{shown}"
    );

    // The new daemon numbers sessions from 1 again.
    let mut again = world.agent("alpha");
    again.shim.ok("click", json!({"x": 555, "y": 666}));
    let mut shown = String::new();
    spec34::wait_until(spec34::TIMEOUT, || {
        shown = world.user_runs(&["log", "alpha"]).stdout;
        shows(&shown, "555", "666")
    });
    let (old, new) = (line_of(&shown, "333", "444"), line_of(&shown, "555", "666"));
    assert!(
        matches!((old, new), (Some(a), Some(b)) if a < b),
        "`log alpha` must show both of alpha's sessions, oldest first:\n{shown}"
    );
    let shown = world.user_runs(&["log", "1"]).stdout;
    assert!(
        shows(&shown, "555", "666") && !shows(&shown, "333", "444"),
        "`log 1` is session 1 of the running daemon only:\n{shown}"
    );
}
