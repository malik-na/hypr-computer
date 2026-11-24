//! #25: `key` presses a key or a key combination in the app on the agent's
//! screen. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::json;
use spec25::probe::{
    Event, Mods, all_keys_released, is_modifier, key_presses, keysym, non_modifier_presses,
};
use spec25::{Agent, Daemon, Env, Shim, assert_changed, is_input, launch_foot, wait_for_file};

/// Presses `keys` and returns the events it caused, once every key is up.
fn press(a: &mut Agent, keys: &str) -> Vec<Event> {
    a.probe.wait_keyboard_focus();
    let mark = a.probe.mark();
    a.shim.ok("key", json!({ "keys": keys }));
    a.probe.wait_for(mark, |e| {
        e.iter()
            .any(|e| matches!(e, Event::Key { pressed: false, .. }))
            && all_keys_released(e)
    });
    a.probe.settle(mark)
}

const NONE: Mods = Mods {
    ctrl: false,
    shift: false,
    alt: false,
    logo: false,
};

#[test]
fn a_named_key_is_pressed_and_let_go() {
    let mut a = Agent::with_probe();
    for name in [
        "Return",
        "Escape",
        "Tab",
        "BackSpace",
        "Delete",
        "Up",
        "Page_Down",
        "F5",
    ] {
        let events = press(&mut a, name);
        let presses = key_presses(&events);
        assert!(
            matches!(presses.as_slice(), [(sym, _, mods)] if *sym == keysym(name) && *mods == NONE),
            "{name} gave {presses:?}"
        );
        assert!(all_keys_released(&events), "{name} stayed down: {events:?}");
    }
}

/// Each combination's last key goes down while its modifiers are held.
#[test]
fn key_combinations_reach_the_app_with_their_modifiers() {
    let mut a = Agent::with_probe();
    let cases = [
        ("ctrl+l", &["l"][..], Mods { ctrl: true, ..NONE }),
        (
            "ctrl+shift+t",
            &["t", "T"][..],
            Mods {
                ctrl: true,
                shift: true,
                ..NONE
            },
        ),
        ("alt+Left", &["Left"][..], Mods { alt: true, ..NONE }),
        ("super+Return", &["Return"][..], Mods { logo: true, ..NONE }),
    ];
    for (keys, last, mods) in cases {
        let events = press(&mut a, keys);
        let presses = non_modifier_presses(&events);
        let ok = matches!(
            presses.as_slice(),
            [(sym, _, m)] if last.iter().any(|l| keysym(l) == *sym) && *m == mods
        );
        assert!(ok, "{keys} gave {presses:?} (all events: {events:?})");
        assert!(
            all_keys_released(&events),
            "{keys} left a key down: {events:?}"
        );
    }
}

/// "Held down in order and let go of together": every key goes down, in
/// the order given, before any comes up.
#[test]
fn a_combinations_keys_go_down_in_order_before_any_comes_up() {
    let mut a = Agent::with_probe();
    let events = press(&mut a, "ctrl+shift+t");
    let keys: Vec<(bool, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Key { name, pressed, .. } => Some((*pressed, name.clone())),
            _ => None,
        })
        .collect();
    let first_release = keys.iter().position(|k| !k.0).unwrap_or(keys.len());
    let downs: Vec<&str> = keys[..first_release].iter().map(|k| k.1.as_str()).collect();
    assert!(
        downs.len() == 3
            && downs[0].starts_with("Control")
            && downs[1].starts_with("Shift")
            && !is_modifier(downs[2])
            && downs[2].eq_ignore_ascii_case("t"),
        "expected Control, Shift, t down before any release: {keys:?}"
    );
    assert_eq!(keys.len(), 6, "each key down once and up once: {keys:?}");
}

#[test]
fn nothing_stays_held_after_a_combination() {
    let mut a = Agent::with_probe();
    press(&mut a, "ctrl+shift+t");
    press(&mut a, "alt+Left");
    let mark = a.probe.mark();
    a.shim.ok("type", json!({"text": "ab"}));
    assert_eq!(a.probe.wait_typed(mark, "ab"), "ab");
    for (_, name, mods) in key_presses(&a.probe.since(mark)) {
        assert_eq!(mods, NONE, "{name} went down with a modifier still held");
    }
}

#[test]
fn an_unknown_key_is_a_clear_error_and_presses_nothing() {
    let mut a = Agent::with_probe();
    a.probe.wait_keyboard_focus();
    let mark = a.probe.mark();
    let reply = a.shim.call("key", json!({"keys": "ctrl+nosuchkey"}));
    assert!(reply.is_error(), "{}", reply.raw);
    assert!(
        spec25::mentions(&reply.text(), "nosuchkey"),
        "the error should name the key: {}",
        reply.raw
    );
    let events = a.probe.settle(mark);
    assert!(
        !events.iter().any(is_input),
        "nothing should be pressed, not even ctrl: {events:?}"
    );
}

#[test]
fn a_screenshot_after_each_key_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    a.probe.wait_keyboard_focus();
    let mut before = a.shim.screenshot();
    for keys in ["Return", "ctrl+l", "F5"] {
        a.shim.ok("key", json!({ "keys": keys }));
        before = assert_changed(&mut a.shim, &before, &format!("key {keys}"));
    }
}

/// A real app: ctrl+d in foot ends `cat`'s input.
#[test]
fn a_key_combination_reaches_a_terminal() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let (out, done) = (env.path("out.txt"), env.path("done.txt"));
    let script = format!(
        "cat > '{}'; echo done > '{}'",
        out.display(),
        done.display()
    );
    if launch_foot(&mut shim, &script).is_none() {
        return;
    }
    shim.ok("type", json!({"text": "abc\n"}));
    assert_eq!(wait_for_file(&out, "abc\n"), "abc\n");
    shim.ok("key", json!({"keys": "ctrl+d"}));
    assert_eq!(
        wait_for_file(&done, "done\n"),
        "done\n",
        "ctrl+d didn't end cat's input"
    );
}
