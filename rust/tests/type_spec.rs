//! #25: `type` types text into the app on the agent's screen, in order,
//! non-ASCII included. Written from the spec alone.
#![forbid(unsafe_code)]

mod spec25;

use serde_json::json;
use spec25::probe::{all_keys_released, key_presses, keysym};
use spec25::{Agent, Daemon, Env, Shim, assert_changed, launch_foot, wait_for_file};

/// Types `text` and returns what the app received.
fn type_and_read(a: &mut Agent, text: &str) -> String {
    a.probe.wait_keyboard_focus();
    let mark = a.probe.mark();
    a.shim.ok("type", json!({ "text": text }));
    a.probe.wait_typed(mark, text)
}

#[test]
fn ascii_text_arrives_in_order() {
    let mut a = Agent::with_probe();
    let text = "hello world 0123456789";
    assert_eq!(type_and_read(&mut a, text), text);
}

#[test]
fn capitals_and_symbols_arrive_as_typed() {
    let mut a = Agent::with_probe();
    let text = "Hello, World! ~`!@#$%^&*()-_=+[]{}\\|;:'\",.<>/? QWERTY";
    assert_eq!(type_and_read(&mut a, text), text);
}

#[test]
fn non_ascii_text_arrives_in_order() {
    let mut a = Agent::with_probe();
    let text = "héllo wörld ✓";
    assert_eq!(type_and_read(&mut a, text), text);
}

#[test]
fn text_in_any_script_arrives_in_order() {
    let mut a = Agent::with_probe();
    let text = "Grüße, señor — naïve café ☕ Ωμέγα Привет 日本語 🙂 ✓";
    assert_eq!(type_and_read(&mut a, text), text);
}

#[test]
fn newline_presses_enter_and_tab_presses_tab() {
    let mut a = Agent::with_probe();
    a.probe.wait_keyboard_focus();
    let mark = a.probe.mark();
    a.shim.ok("type", json!({"text": "one\ttwo\nthree"}));
    assert_eq!(
        a.probe.wait_typed(mark, "one\ttwo\nthree"),
        "one\ttwo\nthree"
    );
    let syms: Vec<u32> = key_presses(&a.probe.since(mark))
        .iter()
        .map(|k| k.0)
        .collect();
    let tab = syms.iter().position(|s| *s == keysym("Tab"));
    let enter = syms.iter().position(|s| *s == keysym("Return"));
    assert!(
        matches!((tab, enter), (Some(t), Some(e)) if t < e),
        "Tab then Return were pressed: {syms:?}"
    );
}

#[test]
fn a_long_mixed_text_arrives_whole_and_in_order() {
    let mut a = Agent::with_probe();
    let pieces = [
        "abc", "XYZ", " ", "é", "ü", "✓", "日", "123", "!?", "ß", "€", "ñ",
    ];
    let text: String = (0..160)
        .map(|i| pieces[(i * 7 + i / 3) % pieces.len()])
        .collect();
    assert_eq!(type_and_read(&mut a, &text), text);
}

#[test]
fn no_key_stays_down_after_typing() {
    let mut a = Agent::with_probe();
    assert_eq!(type_and_read(&mut a, "ABC ✓ É"), "ABC ✓ É");
    // Were Shift (or anything) left held, these would come out wrong.
    assert_eq!(type_and_read(&mut a, "abc"), "abc");
    assert!(
        all_keys_released(&a.probe.events()),
        "{:?}",
        a.probe.events()
    );
}

#[test]
fn a_screenshot_after_typing_differs_from_the_one_before() {
    let mut a = Agent::with_probe();
    a.probe.wait_keyboard_focus();
    let mut before = a.shim.screenshot();
    for text in ["a", "Z", "ö", "hello"] {
        a.shim.ok("type", json!({ "text": text }));
        before = assert_changed(&mut a.shim, &before, &format!("typing {text:?}"));
    }
}

/// A real app: what the agent types into foot reaches the program in it.
#[test]
fn typed_text_non_ascii_included_reaches_a_terminal() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    let out = env.path("typed.txt");
    if launch_foot(&mut shim, &format!("cat > '{}'", out.display())).is_none() {
        return;
    }
    let line = "héllo wörld ✓ and ASCII too\n";
    shim.ok("type", json!({ "text": line }));
    assert_eq!(wait_for_file(&out, line), line);
}

/// A real app redraws as it is typed into, and the agent sees it.
#[test]
fn a_screenshot_after_typing_into_a_terminal_differs() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let mut shim = Shim::ready(&env);
    if launch_foot(&mut shim, "cat > /dev/null").is_none() {
        return;
    }
    let mut before = shim.screenshot();
    for text in ["x", "y", "ü"] {
        shim.ok("type", json!({ "text": text }));
        before = assert_changed(&mut shim, &before, &format!("typing {text:?} into foot"));
    }
}
