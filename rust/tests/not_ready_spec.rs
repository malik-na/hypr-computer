//! #25: input tools called before the agent's screen is ready are refused,
//! and none of that input turns up later. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use std::fs;

use serde_json::json;
use spec25::probe::{BTN_LEFT, Probe};
use spec25::{Daemon, Env, Shim, is_input};

fn calls() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("click", json!({"x": 100, "y": 100})),
        ("move", json!({"x": 200, "y": 200})),
        ("type", json!({"text": "early"})),
        ("key", json!({"keys": "Return"})),
        ("scroll", json!({"x": 300, "y": 300})),
        ("drag", json!({"x1": 10, "y1": 10, "x2": 50, "y2": 50})),
        (
            "launch_app",
            json!({"app": "sh", "args": ["-c", "sleep 30"]}),
        ),
    ]
}

#[test]
fn calls_while_the_screen_is_starting_are_refused_and_do_nothing_later() {
    let env = Env::new();
    let go = env.path("go");
    let mut daemon = Daemon::with_screen_held_back(&env, &go);
    let mut shim = Shim::start(&env);

    for (tool, args) in calls() {
        let reply = shim.call(tool, args.clone());
        assert!(
            reply.is_error(),
            "{tool} {args} wasn't refused: {}",
            reply.raw
        );
        assert!(
            reply.text().contains("still starting"),
            "{tool}: {}",
            reply.raw
        );
    }
    assert!(daemon.is_running());

    fs::write(&go, "").unwrap();
    shim.screenshot();
    let probe = Probe::start(&env.screen_socket(1));
    let events = probe.settle(0);
    assert!(
        !events.iter().any(is_input),
        "refused input arrived later: {events:?}"
    );

    // And now the screen is ready, input works.
    shim.ok("click", json!({"x": 100, "y": 100}));
    assert!(probe.wait_for(0, |e| !spec25::probe::presses(e).is_empty()));
    assert!(matches!(
        probe.presses_since(0).as_slice(),
        [(BTN_LEFT, _, _)]
    ));
}

/// With no app on the screen there's nothing to take input; whatever the
/// tools answer, the session carries on.
#[test]
fn input_on_an_empty_screen_is_harmless() {
    let env = Env::new();
    let mut daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    for (tool, args) in calls().into_iter().filter(|(t, _)| *t != "launch_app") {
        let reply = shim.call(tool, args.clone());
        assert!(
            !reply.is_error() || !reply.text().trim().is_empty(),
            "{tool}: an error with no message: {}",
            reply.raw
        );
        assert!(
            daemon.is_running(),
            "{tool} crashed the daemon:\n{}",
            daemon.log()
        );
    }
    assert_eq!(shim.screenshot().width, 1280);
    let probe = Probe::start(&env.screen_socket(1));
    let mark = probe.mark();
    shim.ok("click", json!({"x": 640, "y": 400}));
    assert!(probe.wait_for(mark, |e| !spec25::probe::presses(e).is_empty()));
}
