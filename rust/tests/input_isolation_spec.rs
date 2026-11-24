//! #25: an agent's input goes to its own screen and nowhere else: not to
//! another agent's screen, and not to the user's desktop. Written from the
//! spec alone.
#![forbid(unsafe_code)]

mod spec25;

use std::os::unix::net::UnixListener;

use serde_json::json;
use spec25::probe::Probe;
use spec25::{Daemon, Env, Shim, is_input};

/// One of every input tool, all aimed at the middle of the screen.
fn every_input(shim: &mut Shim) {
    shim.ok("move", json!({"x": 640, "y": 400}));
    shim.ok("click", json!({"x": 640, "y": 400}));
    shim.ok("type", json!({"text": "secret ✓"}));
    shim.ok("key", json!({"keys": "ctrl+a"}));
    shim.ok("scroll", json!({"x": 640, "y": 400}));
    shim.ok("drag", json!({"x1": 100, "y1": 100, "x2": 200, "y2": 200}));
}

#[test]
fn one_agents_input_never_reaches_another_agents_screen() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut a = Shim::ready(&env);
    let _b = Shim::ready(&env);
    let on_a = Probe::start(&env.screen_socket(1));
    let on_b = Probe::start(&env.screen_socket(2));
    on_a.wait_keyboard_focus();
    on_b.wait_keyboard_focus();
    let (mark_a, mark_b) = (on_a.mark(), on_b.mark());

    every_input(&mut a);
    assert!(on_a.wait_for(mark_a, |e| spec25::probe::typed(e).contains("secret ✓")));
    let leaked = on_b.settle(mark_b);
    assert!(
        !leaked.iter().any(is_input),
        "agent 1's input reached agent 2's screen: {leaked:?}"
    );
}

/// A daemon started from inside the user's desktop knows its Wayland
/// socket; the agent's input must never go there.
#[test]
fn input_never_goes_to_the_users_desktop() {
    let env = Env::new();
    let desktop = env.runtime_dir().join("wayland-user-desktop");
    let listener = UnixListener::bind(&desktop).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut cmd = env.command(&spec25::bin("hypr-computer-daemon"));
    cmd.env("WAYLAND_DISPLAY", "wayland-user-desktop");
    let _daemon = Daemon::spawn(&env, cmd);
    let mut shim = Shim::ready(&env);
    let probe = Probe::start(&env.screen_socket(1));
    probe.wait_keyboard_focus();
    // Whatever connected while the session started (a live view, one day)
    // isn't input; from here on nothing may.
    while listener.accept().is_ok() {}

    let mark = probe.mark();
    every_input(&mut shim);
    assert!(probe.wait_for(mark, |e| spec25::probe::typed(e).contains("secret ✓")));
    probe.settle(mark);
    assert!(
        listener.accept().is_err(),
        "something connected to the user's desktop while the agent was giving input"
    );
}
