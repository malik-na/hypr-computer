//! The agent's pointer and keyboard: what an app on the agent's screen
//! receives when the agent clicks, moves, scrolls, drags, types and presses
//! keys, and what the next screenshot shows.
#![forbid(unsafe_code)]

mod support;

use serde_json::{Value, json};
use support::app::{self, Log};
use support::{Daemon, Env, Shim, agent};

const RED: [u8; 3] = [0xd0, 0x20, 0x20];
const BLUE: [u8; 3] = [0x20, 0x40, 0xd0];

/// An agent with the test app on its screen, launched as an agent would.
fn agent_with_app(env: &Env) -> (Shim, Log) {
    let mut agent = agent(env);
    let log = Log::new(env);
    let result = agent.call(
        "launch_app",
        json!({"app": app::program(), "args": app::args(RED, BLUE, &log)}),
    );
    assert_eq!(result["isError"], false, "{result}");
    log.wait_for("ready");
    (agent, log)
}

/// Calls `tool` and checks that it worked.
fn act(agent: &mut Shim, tool: &str, args: Value) {
    let result = agent.call(tool, args.clone());
    assert_eq!(result["isError"], false, "{tool} {args}: {result}");
}

/// The app's button events, as (button, down or up, x, y).
fn buttons(log: &Log) -> Vec<(String, String, f64, f64)> {
    log.events()
        .into_iter()
        .filter(|e| e["event"] == "button")
        .map(|e| {
            (
                e["button"].as_str().unwrap().to_owned(),
                e["state"].as_str().unwrap().to_owned(),
                e["x"].as_f64().unwrap(),
                e["y"].as_f64().unwrap(),
            )
        })
        .collect()
}

fn press(button: &str, x: f64, y: f64) -> Vec<(String, String, f64, f64)> {
    ["down", "up"]
        .map(|state| (button.to_owned(), state.to_owned(), x, y))
        .to_vec()
}

#[test]
fn a_click_lands_where_the_agent_clicked_and_the_next_screenshot_shows_it() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let (mut agent, log) = agent_with_app(&env);
    let before = agent.screenshot();

    act(&mut agent, "click", json!({"x": 100, "y": 200}));
    assert_eq!(buttons(&log), press("left", 100.0, 200.0));
    assert!(agent.screenshot() != before, "the screen didn't change");

    let before = agent.screenshot();
    act(
        &mut agent,
        "click",
        json!({"x": 1279, "y": 799, "button": "right", "count": 2}),
    );
    let mut expected = press("left", 100.0, 200.0);
    expected.extend(press("right", 1279.0, 799.0));
    expected.extend(press("right", 1279.0, 799.0));
    assert_eq!(buttons(&log), expected);
    assert!(agent.screenshot() != before, "the screen didn't change");
}

#[test]
fn a_click_off_the_screen_is_refused_and_nothing_is_clicked() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let (mut agent, log) = agent_with_app(&env);

    let result = agent.call("click", json!({"x": 1280, "y": 10}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(support::text(&result).contains("1280x800"), "{result}");
    // A pixel is a whole number: 1.5 isn't rounded to some pixel.
    let refused = agent.request(
        "tools/call",
        json!({"name": "click", "arguments": {"x": 1.5, "y": 5}}),
    );
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    assert_eq!(buttons(&log), []);
}

/// Where the app last saw the pointer.
fn pointer_at(log: &Log) -> (f64, f64) {
    let last = log
        .events()
        .into_iter()
        .rfind(|e| e["event"] == "motion" || e["event"] == "enter")
        .expect("the pointer never came to the app");
    (last["x"].as_f64().unwrap(), last["y"].as_f64().unwrap())
}

#[test]
fn move_takes_the_pointer_there_without_clicking() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let (mut agent, log) = agent_with_app(&env);
    let before = agent.screenshot();

    act(&mut agent, "move", json!({"x": 300, "y": 400}));
    assert_eq!(pointer_at(&log), (300.0, 400.0));
    assert_eq!(buttons(&log), []);
    assert!(agent.screenshot() != before, "the screen didn't change");
}

/// The text the app received, as it read it from the keys.
fn typed(log: &Log) -> String {
    log.events()
        .into_iter()
        .filter(|e| e["event"] == "key")
        .map(|e| e["text"].as_str().unwrap().to_owned())
        .collect()
}

/// Types `text` and checks that the app got exactly that.
fn type_text(agent: &mut Shim, log: &Log, text: &str) {
    let before = typed(log).chars().count();
    act(agent, "type", json!({"text": text}));
    let got: String = typed(log).chars().skip(before).collect();
    if got != text {
        let same = got
            .chars()
            .zip(text.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let around = |s: &str| {
            s.chars()
                .skip(same.saturating_sub(5))
                .take(15)
                .collect::<String>()
        };
        panic!(
            "the app got other text: the first {same} characters match, then it got {:?} for {:?} \
             ({} characters of {})",
            around(&got),
            around(text),
            got.chars().count(),
            text.chars().count()
        );
    }
}

#[test]
fn typed_text_arrives_in_order_and_the_next_screenshot_shows_it() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let (mut agent, log) = agent_with_app(&env);
    let before = agent.screenshot();

    type_text(
        &mut agent,
        &log,
        "Hello, World! ~`@#$%^&*()_+{}|:\"<>? 1234567890-=[]\\;',./\n\tend",
    );
    assert!(agent.screenshot() != before, "the screen didn't change");

    // Far more than an app reads in one go, each letter with Shift.
    let long: String = (0..3000)
        .map(|i| char::from(b'A' + (i % 26) as u8))
        .collect();
    type_text(&mut agent, &log, &long);
}

#[test]
fn any_text_arrives_in_order_non_ascii_included() {
    let env = Env::new();
    let _daemon = Daemon::start(&env);
    let (mut agent, log) = agent_with_app(&env);

    type_text(&mut agent, &log, "Grüße, naïve café: 世界 🙂 Ωμέγα ½ €5\n");
    // More different characters than one keymap has room for, between
    // characters a US keyboard has.
    let many: String = (0x4e00..0x4e00 + 300)
        .filter_map(char::from_u32)
        .flat_map(|c| [c, 'a'])
        .collect();
    type_text(&mut agent, &log, &many);
}
