//! #31: read-only mode, `hypr-computer allow --read-only`, lets agents look
//! but not click or type. Written from the spec (#31, specs/Limits.tla)
//! alone; spec31/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec31;

use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use spec31::{BLUE, GREEN, ORANGE, PURPLE, QUIET, World, arguments, assert_refused, input_calls};

const NOTES: (spec31::Rgb, spec31::Rgb) = (GREEN, BLUE);

/// Whether `tools/list` marks a tool as only reading.
fn only_reads(tool: &Value) -> bool {
    tool["annotations"]["readOnlyHint"] == json!(true)
}

#[test]
fn the_tools_say_whether_they_only_read() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let tools = agent.shim.tools();
    let find = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no tool {name} in {tools:?}"))
            .clone()
    };
    assert!(
        only_reads(&find("screenshot")),
        "screenshot only reads the screen: tools/list must say readOnlyHint: true; it says {}",
        find("screenshot")
    );
    for name in [
        "click",
        "move",
        "type",
        "key",
        "scroll",
        "drag",
        "launch_app",
        "open_url",
    ] {
        assert!(
            !only_reads(&find(name)),
            "{name} changes the screen: it must not be marked readOnlyHint: true"
        );
    }
}

/// Read-only set mid-session applies to the next call: every tool that does
/// more than read is refused, saying read-only and to ask the user, and
/// nothing reaches the screen. Looking still works.
#[test]
fn read_only_lets_agents_only_look() {
    let mut fake = None;
    let mut world = World::with_browser(|env| {
        let browser = env.fake_browser();
        let path = browser.path.clone();
        fake = Some(browser);
        path
    });
    let fake = fake.unwrap();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
    let before_shot = agent.shim.screenshot();

    world.user_runs(&["allow", "--read-only"]);

    let shot = agent.shim.screenshot();
    assert_eq!(shot, before_shot, "read-only: looking must still work");
    let before = notes.log.inputs().len();
    let tools = agent.shim.tools();
    let mut refused = Vec::new();
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let args = arguments(&world.env, name, &tool["inputSchema"]);
        let reply = agent.shim.call(name, args.clone());
        if only_reads(tool) {
            assert!(
                !reply.is_error(),
                "{name} only reads, so read-only allows it: {}",
                reply.raw
            );
        } else {
            assert_refused(
                &reply,
                "read-only",
                &format!("{name} {args} in read-only mode"),
            );
            refused.push(name.to_owned());
        }
    }
    for (tool, _) in input_calls() {
        assert!(refused.iter().any(|t| t == tool), "{tool} wasn't refused");
    }
    thread::sleep(QUIET);
    assert_eq!(
        notes.log.inputs()[before..].to_vec(),
        Vec::<Value>::new(),
        "input reached the app in read-only mode"
    );
    assert!(
        !world.env.path("launched-anyway.log").exists(),
        "launch_app started an app in read-only mode"
    );
    assert!(
        fake.launches().is_empty(),
        "open_url started the browser in read-only mode"
    );
    assert_eq!(agent.shim.screenshot(), before_shot, "the screen changed");

    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.to_lowercase().contains("read-only"),
        "{listed:?}"
    );

    world.user_runs(&["allow", "--off"]);
    agent.click_reaches(&notes);
}

#[test]
fn read_only_applies_to_agents_that_connect_later() {
    let mut world = World::new();
    world.user_runs(&["allow", "--read-only"]);
    let mut agent = world.agent("beta");
    agent.shim.screenshot();
    let (reply, app) = agent.try_launch(&world.env, "notes", NOTES, &[]);
    assert_refused(&reply, "read-only", "launch_app in read-only mode");
    thread::sleep(QUIET);
    assert!(
        !app.log.exists(),
        "launch_app started an app in read-only mode"
    );
    for (tool, args) in input_calls() {
        let reply = agent.shim.call(tool, args.clone());
        assert_refused(&reply, "read-only", &format!("{tool} in read-only mode"));
    }
}

#[test]
fn read_only_for_a_set_time_stops_applying_when_the_time_is_up() {
    const FOR: Duration = Duration::from_secs(12);
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let paint = agent.launch(&world.env, "paint", (ORANGE, PURPLE));

    let set = Instant::now();
    world.user_runs(&["allow", "--read-only", "--for", "12s"]);
    let reply = agent.shim.call("click", json!({"x": 640, "y": 400}));
    assert!(
        set.elapsed() < FOR - Duration::from_secs(2),
        "the machine is too slow for this test: {:?} went by before the check",
        set.elapsed()
    );
    assert_refused(&reply, "read-only", "a click in read-only mode");

    thread::sleep(FOR.saturating_sub(set.elapsed()) + Duration::from_secs(2));

    // An expired limit stops applying: not read-only, and no other limit.
    agent.click_reaches(&paint);
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
}
