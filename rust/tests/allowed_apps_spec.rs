//! #31: `hypr-computer allow APP...` limits agents to the apps it names,
//! optionally for a set time. Written from the spec (#31, specs/Limits.tla)
//! alone; spec31/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec31;

use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use spec31::{
    BLUE, GREEN, ORANGE, PURPLE, QUIET, TEAL, World, assert_refused, input_calls, wait_until,
};

const NOTES: (spec31::Rgb, spec31::Rgb) = (GREEN, BLUE);
const PAINT: (spec31::Rgb, spec31::Rgb) = (ORANGE, PURPLE);

#[test]
fn with_no_limit_an_agent_uses_any_app() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
    let paint = agent.launch(&world.env, "paint", PAINT);
    agent.click_reaches(&paint);
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.contains("No limits"),
        "with no limit, `allow --list` must say \"No limits\": {listed:?}"
    );
}

#[test]
fn an_app_limit_refuses_launching_any_other_app() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    world.user_runs(&["allow", "notes"]);

    let (reply, paint) = agent.try_launch(&world.env, "paint", PAINT, &[]);
    assert_refused(
        &reply,
        "paint",
        "launch_app of paint, which the user didn't allow",
    );
    thread::sleep(QUIET);
    assert!(
        !paint.log.exists(),
        "paint was refused, yet it started: {:?}",
        paint.log.events()
    );

    // The app it allows works, and so does looking.
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
    agent.shim.screenshot();
}

#[test]
fn the_app_name_is_the_programs_file_name_in_any_case() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    world.user_runs(&["allow", "NOTES,Paint"]);
    // Launched by its full path.
    let notes = agent.launch(&world.env, "notes", NOTES);
    agent.click_reaches(&notes);
    let paint = agent.launch(&world.env, "paint", PAINT);
    agent.click_reaches(&paint);
    let (reply, _) = agent.try_launch(&world.env, "sketch", (TEAL, GREEN), &[]);
    assert_refused(&reply, "sketch", "launch_app of sketch");
}

/// The limit applies to the next call: input to the app in front is refused
/// once the user limits agents to other apps, whichever input it is, and
/// nothing reaches any app on the screen.
#[test]
fn input_to_an_app_the_limit_doesnt_name_is_refused_from_the_next_call() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);
    let paint = agent.launch(&world.env, "paint", PAINT);
    agent.click_reaches(&paint);

    world.user_runs(&["allow", "notes"]);

    let before = agent.shim.screenshot();
    let (notes_before, paint_before) = (notes.log.inputs().len(), paint.log.inputs().len());
    for (tool, args) in input_calls() {
        let reply = agent.shim.call(tool, args.clone());
        assert_refused(
            &reply,
            "paint",
            &format!("{tool} {args} while paint, which the user didn't allow, is in front"),
        );
    }
    thread::sleep(QUIET);
    assert_eq!(
        paint.log.inputs()[paint_before..].to_vec(),
        Vec::<serde_json::Value>::new(),
        "refused input reached paint"
    );
    assert_eq!(
        notes.log.inputs()[notes_before..].to_vec(),
        Vec::<serde_json::Value>::new(),
        "refused input reached notes"
    );
    // Looking is still allowed, and the screen was left alone.
    assert_eq!(
        agent.shim.screenshot(),
        before,
        "refused input changed the screen"
    );

    // Allowing paint too lets the input through again.
    world.user_runs(&["allow", "notes,paint"]);
    agent.click_reaches(&paint);
}

#[test]
fn allow_off_lifts_the_limit() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    world.user_runs(&["allow", "notes"]);
    let (reply, _) = agent.try_launch(&world.env, "paint", PAINT, &[]);
    assert_refused(&reply, "paint", "launch_app of paint under `allow notes`");

    world.user_runs(&["allow", "--off"]);

    let paint = agent.launch(&world.env, "paint", PAINT);
    agent.click_reaches(&paint);
}

/// Each `allow` replaces the limits before it.
#[test]
fn a_new_allow_replaces_the_one_before() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    world.user_runs(&["allow", "notes"]);
    world.user_runs(&["allow", "paint"]);
    let (reply, notes) = agent.try_launch(&world.env, "notes", NOTES, &[]);
    assert_refused(
        &reply,
        "notes",
        "launch_app of notes after `allow paint` replaced `allow notes`",
    );
    thread::sleep(QUIET);
    assert!(!notes.log.exists(), "the refused notes started");
    let paint = agent.launch(&world.env, "paint", PAINT);
    agent.click_reaches(&paint);
}

#[test]
fn the_limit_applies_to_every_agent_including_ones_that_connect_later() {
    let mut world = World::new();
    let mut alpha = world.agent("alpha");
    world.user_runs(&["allow", "notes"]);
    let mut beta = world.agent("beta");

    for agent in [&mut alpha, &mut beta] {
        let (reply, _) = agent.try_launch(&world.env, "paint", PAINT, &[]);
        assert_refused(
            &reply,
            "paint",
            &format!("{}'s launch_app of paint", agent.who()),
        );
        let notes = agent.launch(&world.env, "notes", NOTES);
        agent.click_reaches(&notes);
    }
}

#[test]
fn open_url_needs_the_browser_allowed() {
    let mut fake = None;
    let mut world = World::with_browser(|env| {
        let browser = env.fake_browser();
        let path = browser.path.clone();
        fake = Some(browser);
        path
    });
    let fake = fake.unwrap();
    let mut agent = world.agent("alpha");
    world.user_runs(&["allow", "notes"]);

    let reply = agent
        .shim
        .call("open_url", json!({"url": "http://127.0.0.1:9/page"}));
    assert_refused(&reply, "browser", "open_url under `allow notes`");
    thread::sleep(QUIET);
    assert!(
        fake.launches().is_empty(),
        "open_url was refused, yet the browser started: {:?}",
        fake.args()
    );
}

/// The real past behaviour this undoes: v0.8's grant left agents read-only
/// once its time was up (d7f3efe). Now an expired limit stops applying.
#[test]
fn an_app_limit_for_a_set_time_stops_applying_when_the_time_is_up() {
    const FOR: Duration = Duration::from_secs(12);
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let notes = agent.launch(&world.env, "notes", NOTES);
    let paint = agent.launch(&world.env, "paint", PAINT);

    let set = Instant::now();
    let ran = world.user_runs(&["allow", "notes", "--for", "12s"]);
    assert!(
        ran.stdout.to_lowercase().contains("until"),
        "`allow --for` should say until when: {ran:?}"
    );
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.contains("notes") && listed.stdout.to_lowercase().contains("until"),
        "`allow --list` should show the app and until when: {listed:?}"
    );
    let reply = agent.shim.call("click", json!({"x": 640, "y": 400}));
    let (reply2, _) = agent.try_launch(&world.env, "sketch", (TEAL, GREEN), &[]);
    assert!(
        set.elapsed() < FOR - Duration::from_secs(2),
        "the machine is too slow for this test: {:?} went by before the checks",
        set.elapsed()
    );
    assert_refused(
        &reply,
        "paint",
        "a click on paint while `allow notes --for 12s` holds",
    );
    assert_refused(
        &reply2,
        "sketch",
        "launch_app of sketch while the limit holds",
    );

    let left = FOR.saturating_sub(set.elapsed()) + Duration::from_secs(2);
    thread::sleep(left);

    agent.click_reaches(&paint);
    let sketch = agent.launch(&world.env, "sketch", (TEAL, GREEN));
    agent.click_reaches(&sketch);
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(
        listed.stdout.contains("No limits"),
        "an expired limit is no limit: {listed:?}"
    );
    drop(notes);
}

/// The limits hold across a restart of the daemon, as systemd restarts one
/// that crashed: a crash must not lift them.
#[test]
fn the_limits_survive_a_restart_of_the_daemon() {
    let mut world = World::new();
    world.user_runs(&["allow", "notes"]);
    world.restart_daemon();

    let mut agent = world.agent("alpha");
    let (reply, paint) = agent.try_launch(&world.env, "paint", PAINT, &[]);
    assert_refused(
        &reply,
        "paint",
        "launch_app of paint after the daemon restarted",
    );
    thread::sleep(QUIET);
    assert!(!paint.log.exists(), "the refused paint started");
    let listed = world.user_runs(&["allow", "--list"]);
    assert!(listed.stdout.contains("notes"), "{listed:?}");
}

/// The user can limit agents before any of them starts, when no daemon is
/// running yet: the daemon that starts goes by the limits.
#[test]
fn a_limit_set_while_no_daemon_runs_applies_once_one_starts() {
    let env = spec31::Env::new();
    let ran = spec31::hypr_computer(&env, &["allow", "notes"]);
    assert!(
        ran.success(),
        "`allow` must work with no daemon running: {ran:?}"
    );
    let daemon = spec31::Daemon::start(&env);
    let mut agent = spec31::Shim::start(&env, "alpha");
    agent.screenshot();
    let program = env.app_program("paint");
    let log = env.path("paint.log");
    let reply = agent.call(
        "launch_app",
        json!({"app": program.display().to_string(), "args": ["--log", log.display().to_string()]}),
    );
    assert_refused(
        &reply,
        "paint",
        "launch_app of paint under a limit set before the daemon started",
    );
    assert!(
        !wait_until(QUIET, || log.exists()),
        "the refused paint started"
    );
    drop(daemon);
}
