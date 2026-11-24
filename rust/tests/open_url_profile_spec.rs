//! #33, part 2: an agent opens pages in a profile the user made, by naming
//! it: `open_url {url, profile}`. What the site sets (a login's cookie) is
//! kept for the next session that names it; a session without a profile
//! still starts fresh. Written from the spec alone; the interface is at the
//! top of spec33/mod.rs.
//!
//! The tests that need a browser pass with a SKIPPED note where no
//! Chromium-family browser is installed (or `HC33_BROWSER=none`).
#![forbid(unsafe_code)]

mod spec33;

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use spec33::{BLUE, COOKIE, Env, GREEN, World, listed, new_profile, tree_modes};

/// `open_url` offers `profile` to the agent.
#[test]
fn open_url_has_a_profile_argument() {
    let w = World::start();
    let mut a = w.agent();
    let tools = a.tools();
    let open_url = tools
        .iter()
        .find(|t| t["name"] == "open_url")
        .unwrap_or_else(|| panic!("no open_url in {tools:?}"));
    let profile = &open_url["inputSchema"]["properties"]["profile"];
    assert_eq!(
        profile["type"], "string",
        "open_url's input schema should have a string `profile`: {open_url}"
    );
    let required = open_url["inputSchema"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !required.contains(&json!("profile")),
        "`profile` must be optional: {open_url}"
    );
}

/// The acceptance criterion: a session using a named profile keeps its
/// logins.
#[test]
fn a_login_made_in_a_profile_is_there_in_the_next_session_that_names_it() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    assert!(
        w.browser.profiles().contains(&w.env.profile("work")),
        "the browser wasn't started on the profile {}: it had {:?}",
        w.env.profile("work").display(),
        w.browser.profiles()
    );

    // The agent leaves; a new session names the profile.
    first.close_stdin();
    w.wait_browsers_gone("the first session ended");
    let mut second = w.agent();
    second.open_in_ok(&w.web.url("/again"), "work");
    assert!(second.screenshot().shows(GREEN), "the page isn't shown");
    let cookie = w.last_cookie("/again");
    assert!(
        cookie.contains(COOKIE),
        "the next session in profile work didn't send the login's cookie; it sent {cookie:?}"
    );
    // The profile is still there, and still the user's.
    assert_eq!(listed(&w.env), ["work"]);
}

/// The acceptance criterion: a fresh session has no cookies from the last
/// one, even when the last one used a profile.
#[test]
fn a_session_without_a_profile_starts_fresh_even_after_one_with_a_profile() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");

    // While the profile's session still runs, and after it ended.
    let mut fresh = w.agent();
    fresh.open(&w.web.url("/again"));
    assert!(fresh.screenshot().shows(GREEN));
    let cookie = w.last_cookie("/again");
    assert!(
        !cookie.contains("hc33"),
        "a session without a profile sent the profile's cookie: {cookie:?}"
    );
    drop(fresh);
    first.close_stdin();
    w.wait_browsers_gone("both sessions ended");

    let mut after = w.agent();
    after.open(&w.web.url("/check"));
    assert!(after.screenshot().shows(BLUE));
    let cookie = w.last_cookie("/check");
    assert!(
        !cookie.contains("hc33"),
        "a new session without a profile sent the last session's cookie: {cookie:?}"
    );
}

/// The agent only uses what the user made: a name that isn't a profile is
/// refused, with what to tell the user, and the agent isn't shown the
/// profiles that do exist.
#[test]
fn a_profile_that_doesnt_exist_is_refused_and_made_by_no_one() {
    let w = World::start();
    new_profile(&w.env, "secret-bank");
    let mut a = w.agent();
    let reply = a.open_in(&w.web.url("/check"), "nope");
    assert!(reply.is_error(), "open_url in a missing profile succeeded");
    let text = reply.text();
    assert!(
        text.contains("nope") && text.contains("hypr-computer profile new"),
        "the error should name the profile and say the user makes it with \
         `hypr-computer profile new`: {text}"
    );
    assert!(
        !text.contains("secret-bank"),
        "the error showed the agent the user's other profiles: {text}"
    );
    assert!(
        !w.env.profile("nope").exists(),
        "open_url made the profile the user hadn't"
    );
    assert_eq!(listed(&w.env), ["secret-bank"]);
    assert!(
        w.browser.launches().is_empty(),
        "a refused profile still started the browser"
    );

    if !w.has_browser() {
        return;
    }
    // The session goes on, and may still pick: here, fresh.
    a.open(&w.web.url("/check"));
    assert!(a.screenshot().shows(BLUE));
}

#[test]
fn a_profile_argument_that_isnt_a_name_is_refused() {
    let w = World::start();
    new_profile(&w.env, "work");
    let mut a = w.agent();
    for profile in [
        json!(42),
        json!(true),
        json!(["work"]),
        json!({"name": "work"}),
        json!(""),
        json!("../work"),
        json!("work/../../x"),
        json!("/tmp"),
        json!(".hidden"),
    ] {
        let reply = a.call(
            "open_url",
            json!({"url": w.web.url("/check"), "profile": profile}),
        );
        assert!(
            reply.is_error(),
            "open_url with profile {profile} succeeded"
        );
        assert!(
            reply.text().contains("profile"),
            "the error for profile {profile} should name the `profile` argument: {}",
            reply.text()
        );
    }
    assert!(
        w.browser.launches().is_empty(),
        "a refused profile still started the browser"
    );
    assert_eq!(listed(&w.env), ["work"]);
}

/// A session's first page fixes its profile. Later pages may leave the
/// profile out, and stay in it; naming another is refused, and the session
/// goes on as it was.
#[test]
fn a_session_keeps_the_profile_its_first_page_opened_in() {
    let w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    new_profile(&w.env, "home");
    let mut a = w.agent();
    w.sign_in(&mut a, "work");

    // Left out: still the profile.
    a.open(&w.web.url("/again"));
    assert!(
        w.last_cookie("/again").contains(COOKIE),
        "a page opened without `profile` left the session's profile"
    );

    let other = a.open_in(&w.web.url("/check"), "home");
    assert!(other.is_error(), "a session switched to a second profile");
    let text = other.text();
    assert!(
        text.contains("work") && text.contains("home") && text.contains("session"),
        "the error should say the session uses profile work, not home, one per session: {text}"
    );
    assert!(
        a.screenshot().shows(GREEN),
        "the refused call changed the screen"
    );
    a.open_in_ok(&w.web.url("/check"), "work");
    assert!(w.last_cookie("/check").contains(COOKIE));
    assert_eq!(
        w.browser.launches().len(),
        1,
        "the session's browser was started again"
    );

    // A fresh session can't take a profile later either.
    let mut fresh = w.agent();
    fresh.open(&w.web.url("/other"));
    let later = fresh.open_in(&w.web.url("/check"), "home");
    assert!(later.is_error(), "a fresh session switched to a profile");
    assert!(
        later.text().contains("home") && later.text().contains("session"),
        "the error should say the session's browser is fresh, one profile per session: {}",
        later.text()
    );
}

/// A profile's files are the user's alone, wherever the browser wrote them,
/// and they stay where the user's data lives.
#[test]
fn a_profile_the_browser_wrote_is_open_to_the_user_only() {
    let w = World::start_in(Env::with_data_home());
    // The user's own browser's profile, which must stay out of it.
    let users = w.env.home().join(".config/chromium");
    fs::create_dir_all(users.join("Default")).unwrap();
    fs::write(users.join("Default/Preferences"), "{}").unwrap();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut a = w.agent();
    w.sign_in(&mut a, "work");
    let dir = w.env.path("data/hypr-computer/profiles/work");
    assert_eq!(
        w.browser.profiles(),
        vec![dir.clone()],
        "the browser should run on the profile in $XDG_DATA_HOME and nothing else"
    );
    a.close_stdin();
    w.wait_browsers_gone("the session ended");

    let modes = tree_modes(&dir);
    assert!(
        modes.iter().any(|(p, _, _)| p.ends_with("Cookies")),
        "the browser wrote no cookie store in the profile: {:?}",
        modes.iter().map(|(p, _, _)| p).collect::<Vec<_>>()
    );
    let open: Vec<(PathBuf, String)> = modes
        .iter()
        .filter(|(_, m, d)| m & 0o077 != 0 || (*d && *m & 0o700 != 0o700))
        .map(|(p, m, _)| (p.clone(), format!("{m:o}")))
        .collect();
    assert!(
        open.is_empty(),
        "these files of the profile are open to others: {open:?}"
    );
    assert_eq!(spec33::mode(&dir), 0o700);

    let mut users_files: Vec<String> = fs::read_dir(users.join("Default"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    users_files.sort();
    assert_eq!(
        users_files,
        ["Preferences"],
        "the agent's browser wrote into the user's own profile"
    );
}

/// A profile and its logins outlast the daemon: stopped as systemd does,
/// and started again.
#[test]
fn a_profiles_logins_outlast_a_daemon_restart() {
    let mut w = World::start();
    if !w.has_browser() {
        return;
    }
    new_profile(&w.env, "work");
    let mut first = w.agent();
    w.sign_in(&mut first, "work");
    first.close_stdin();
    w.wait_browsers_gone("the session ended");

    w.daemon().stop();
    w.restart_daemon();
    let mut second = w.agent();
    second.open_in_ok(&w.web.url("/again"), "work");
    assert!(second.screenshot().shows(GREEN));
    assert!(
        w.last_cookie("/again").contains(COOKIE),
        "after a daemon restart the profile lost its login: {:?}",
        w.last_cookie("/again")
    );
    assert_eq!(listed(&w.env), ["work"]);
}
