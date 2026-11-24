//! #34: the action log is the user's alone, and holds no typed text, no
//! secret part of a URL and, unless the user turns that on, no picture.
//! Written from the spec alone; spec34/ says what interface it assumes.
#![forbid(unsafe_code)]

mod spec34;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::{Value, json};
use spec34::{
    Record, Setup, World, assert_nowhere, assert_stopped, files_under, loose_entries,
    pictures_under,
};

/// The daemon runs under umask 000 here: the modes must be its own doing.
#[test]
fn the_log_is_readable_by_the_user_only() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    let mut b = world.agent("beta");
    a.shim.ok("click", json!({"x": 640, "y": 400}));
    b.shim.ok("click", json!({"x": 640, "y": 400}));
    world.wait_records(1, 2);
    world.wait_records(2, 2);

    let loose = loose_entries(&world.state_dir());
    assert!(
        loose.is_empty(),
        "every dir from {} down must be 0700 and every file 0600: {loose:#?}",
        world.state_dir().display()
    );
}

#[test]
fn a_log_dir_left_open_to_others_is_closed_again() {
    let mut world = World::new();
    {
        let mut a = world.agent("alpha");
        a.shim.ok("click", json!({"x": 640, "y": 400}));
        world.wait_records(1, 2);
    }
    let mut opened = Vec::new();
    spec34::walk(&world.state_dir(), &mut |p, meta| {
        let mode = if meta.is_dir() { 0o755 } else { 0o644 };
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
        opened.push(p.to_path_buf());
    });
    assert!(opened.len() >= 3, "{opened:?}");

    world.restart_daemon();
    let mut a = world.agent("alpha");
    a.shim.ok("click", json!({"x": 10, "y": 10}));
    world.wait_records(1, 2);

    let loose = loose_entries(&world.state_dir());
    assert!(
        loose.is_empty(),
        "the daemon must close up the log again (dirs 0700, files 0600): {loose:#?}"
    );
}

#[test]
fn the_log_is_in_xdg_state_home_when_it_is_set() {
    let mut world = World::with(Setup {
        xdg_dirs: true,
        ..Setup::default()
    });
    let mut a = world.agent("alpha");
    a.shim.ok("click", json!({"x": 640, "y": 400}));
    world.wait_records(1, 2);

    assert!(
        world
            .log_dir()
            .starts_with(world.everything().join("xdg-state")),
        "{}",
        world.log_dir().display()
    );
    let elsewhere = files_under(&world.home().join(".local"));
    assert!(
        elsewhere.is_empty(),
        "with $XDG_STATE_HOME set, nothing goes in ~/.local: {elsewhere:?}"
    );
    let loose = loose_entries(&world.state_dir());
    assert!(loose.is_empty(), "{loose:#?}");
    let shown = world.user_runs(&["log", "alpha"]).stdout;
    assert!(shown.contains("click"), "{shown}");
}

/// Whatever path the text takes (typed, refused, cut short by a stop, or
/// sent to a tool that doesn't exist), it never reaches the disk.
#[test]
fn typed_text_never_reaches_the_disk() {
    let typed = "hcSecretQ4zK9wX";
    let refused = "hcSecretR8mV2pL";
    let cut = "hcSecretC3nT6yB";
    let unknown = "hcSecretU5hJ1dF";

    let mut world = World::new();
    let mut a = world.slow_agent("alpha");
    a.probe.wait_keyboard_focus();
    let start = world.after_setup(1);

    let mark = a.probe.mark();
    a.shim.ok("type", json!({"text": typed}));
    assert_eq!(
        a.probe.wait_typed(mark, typed),
        typed,
        "the text wasn't typed"
    );

    world.user_runs(&["stop", "alpha"]);
    let reply = a.shim.call("type", json!({"text": refused}));
    assert_stopped(&reply, "the stopped agent's typing");
    world.user_runs(&["resume", "alpha"]);

    let long = format!("{cut}{}", "x".repeat(2000));
    let mark = a.probe.mark();
    let typing = a.shim.begin("type", json!({"text": long}));
    let started = a
        .probe
        .wait_for(mark, |e| !spec34::probe::typed(e).is_empty());
    assert!(started, "typing never started: {:?}", a.probe.since(mark));
    world.user_runs(&["stop"]);
    let reply = a.shim.finish(typing, Duration::from_secs(60));
    assert_stopped(&reply, "the typing cut short by the stop");
    world.user_runs(&["resume"]);

    let reply = a.shim.call("nope", json!({"text": unknown}));
    assert!(reply.is_error(), "an unknown tool must fail: {}", reply.raw);

    let all = world.wait_records(1, start + 4);
    let got: Vec<(&str, &Value, &str)> = all[start..]
        .iter()
        .map(|r| (r.tool(), r.arguments(), r.result()))
        .collect();
    let chars = |s: &str| json!({"text": {"characters": s.chars().count()}});
    assert_eq!(
        got[..3],
        [
            ("type", &chars(typed), "ok"),
            ("type", &chars(refused), "stopped"),
            ("type", &chars(&long), "stopped"),
        ],
        "typed text is recorded as its length in characters"
    );
    assert_eq!(
        all[start + 3].result(),
        "invalid",
        "a call to an unknown tool is logged as invalid: {:?}",
        all[start + 3].value
    );

    for secret in [typed, refused, cut, unknown] {
        assert_nowhere(
            &world.everything(),
            secret,
            &world.daemon_output(),
            "text the agent typed",
        );
    }
}

#[test]
fn a_key_press_that_types_a_character_is_logged_as_one_character() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    a.probe.wait_keyboard_focus();
    let start = world.after_setup(1);

    let keys = ["z", "shift+q", "7", "ctrl+l", "Return", "alt+Left"];
    for keys in keys {
        a.shim.ok("key", json!({"keys": keys}));
    }

    let all = world.wait_records(1, start + keys.len());
    let got: Vec<&Value> = all[start..].iter().map(Record::arguments).collect();
    let one = json!({"keys": {"characters": 1}});
    let full = |k: &str| json!({"keys": k});
    assert_eq!(
        got,
        [
            &one,
            &one,
            &one,
            &full("ctrl+l"),
            &full("Return"),
            &full("alt+Left")
        ],
        "a key that only types a character is recorded as one character; shortcuts in full"
    );
}

/// open_url keeps only the URL's origin: user info, path, query and
/// fragment can hold secrets (reset links, tokens).
#[test]
fn a_url_is_logged_by_its_origin_only() {
    let secret = "hcSecretW7eN3rT";
    // A browser that can't start: open_url gets as far as trying, and fails.
    let mut world = World::with(Setup {
        daemon_env: vec![(
            "HYPR_COMPUTER_BROWSER".into(),
            "/nonexistent/hc-browser".into(),
        )],
        ..Setup::default()
    });
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);

    world.user_runs(&["stop", "alpha"]);
    let reply = a.shim.call(
        "open_url",
        json!({"url": format!("https://me:{secret}@example.com:8443/reset/{secret}?token={secret}#{secret}")}),
    );
    assert_stopped(&reply, "the stopped agent's open_url");
    world.user_runs(&["resume", "alpha"]);
    let failed = a.shim.call(
        "open_url",
        json!({"url": format!("http://127.0.0.1:9/{secret}?k={secret}")}),
    );
    assert!(
        failed.is_error(),
        "open_url without a browser must fail: {}",
        failed.raw
    );
    let refused = a.shim.call(
        "open_url",
        json!({"url": format!("ftp://example.com/{secret}")}),
    );
    assert!(
        refused.is_error(),
        "an ftp URL must be refused: {}",
        refused.raw
    );
    let junk = format!("not a url {secret}");
    let refused = a.shim.call("open_url", json!({"url": junk}));
    assert!(
        refused.is_error(),
        "a non-URL must be refused: {}",
        refused.raw
    );

    let all = world.wait_records(1, start + 4);
    let got: Vec<(&Value, &str)> = all[start..]
        .iter()
        .map(|r| (r.arguments(), r.result()))
        .collect();
    assert_eq!(
        got,
        [
            (&json!({"url": "https://example.com:8443"}), "stopped"),
            (&json!({"url": "http://127.0.0.1:9"}), "error"),
            (&json!({"url": "ftp://example.com"}), "invalid"),
            (
                &json!({"url": {"characters": junk.chars().count()}}),
                "invalid"
            ),
        ],
        "a URL is recorded as its origin, scheme://host[:port]"
    );
    assert_nowhere(
        &world.everything(),
        secret,
        &world.daemon_output(),
        "the secret part of a URL",
    );
}

#[test]
fn screenshots_are_not_kept_by_default() {
    let mut world = World::new();
    let mut a = world.agent("alpha");
    let start = world.after_setup(1);
    for (x, y) in [(100, 100), (600, 300), (1000, 700)] {
        a.shim.ok("click", json!({"x": x, "y": y}));
        a.shim.screenshot();
    }

    let all = world.wait_records(1, start + 6);
    let shots: Vec<&Record> = all[start..]
        .iter()
        .filter(|r| r.tool() == "screenshot")
        .collect();
    assert_eq!(shots.len(), 3, "each screenshot call is logged: {shots:#?}");
    for r in &all {
        assert!(
            r.value.get("screenshot").is_none(),
            "with screenshots off, no record names a picture: {:?}",
            r.value
        );
    }
    let pictures = pictures_under(&world.state_dir());
    assert!(
        pictures.is_empty(),
        "with screenshots off (the default), no picture reaches the log: {pictures:?}"
    );
}
