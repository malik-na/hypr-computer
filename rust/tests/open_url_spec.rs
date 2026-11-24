//! #26, part 1: `open_url` opens a web page on the agent's screen in one
//! call, in a browser with a fresh profile of the session's own, and returns
//! once the page has loaded. Only http and https URLs are opened. Written
//! from the spec alone.
//!
//! The tests that need a browser pass with a SKIPPED note where no
//! Chromium-family browser is installed (or `HC26_BROWSER=none`); every test
//! first checks that the agent has `open_url`.
#![forbid(unsafe_code)]

mod spec26;

use std::fs;
use std::time::{Duration, Instant};

use serde_json::json;
use spec26::web::Page;
use spec26::{
    Agent, BLUE, Daemon, Env, GREEN, MAGENTA, ORANGE, RED, Shim, TIMEOUT, YELLOW, wait_until,
};

#[test]
fn the_page_is_on_the_screen_when_open_url_returns() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    // Red until the page has loaded: its image comes 2 s late, and the
    // page turns green on `load`.
    a.web.page(
        "/slow",
        Page::html(format!(
            "<!doctype html><html><head><title>Slow page</title></head>\
             <body style=\"margin:0;height:100vh;background:{red}\">\
             <img src=\"/slow.png\" width=\"1\" height=\"1\">\
             <script>addEventListener('load', () => {{\
               document.body.style.background = '{green}';\
               document.title = 'Loaded';\
             }});</script></body></html>",
            red = spec26::css(RED),
            green = spec26::css(GREEN),
        )),
    );
    a.web
        .page("/slow.png", Page::html("").delayed(Duration::from_secs(2)));

    let reply = a.shim.open(&a.web.url("/slow"));
    let returned = Instant::now();

    let image = a.web.wait_for("/slow.png");
    assert!(
        image.answered <= returned,
        "open_url returned before the page had loaded (its image was still on its way)"
    );
    let shot = a.shim.screenshot();
    assert!(
        shot.shows(GREEN),
        "right after open_url returned, the screen didn't show the loaded page: {shot:?} \
         (red means it showed the page before it had loaded); open_url said: {}",
        reply.text()
    );
}

#[test]
fn a_second_page_opens_in_the_same_session_without_starting_over() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/first", Page::solid("First", ORANGE, ""));
    a.web.page("/second", Page::solid("Second", BLUE, ""));

    a.shim.open(&a.web.url("/first"));
    assert!(
        a.shim.screenshot().shows(ORANGE),
        "the first page isn't shown"
    );

    let started = Instant::now();
    a.shim.open(&a.web.url("/second"));
    let took = started.elapsed();
    let shot = a.shim.screenshot();
    assert!(
        shot.shows(BLUE),
        "after the second open_url the screen doesn't show the second page: {shot:?}"
    );
    assert!(
        took < Duration::from_secs(5),
        "opening a quick local page in a browser already running took {took:?}"
    );
}

#[test]
fn only_http_and_https_urls_are_opened() {
    let mut a = Agent::start();
    let file = a.env.path("secret.html");
    fs::write(&file, "<title>secret</title>").unwrap();
    for url in [
        "file:///etc/passwd".to_owned(),
        format!("file://{}", file.display()),
        "javascript:alert(1)".to_owned(),
        "data:text/html,<h1>hi</h1>".to_owned(),
        "chrome://settings".to_owned(),
        "about:blank".to_owned(),
        "ftp://127.0.0.1/".to_owned(),
        "view-source:http://127.0.0.1/".to_owned(),
        "http://".to_owned(),
        "".to_owned(),
    ] {
        let reply = a.shim.call("open_url", json!({"url": url}));
        assert!(reply.is_error(), "open_url {url:?} was accepted");
        let text = reply.text().to_lowercase();
        assert!(
            text.contains("http"),
            "refusing {url:?} should say that only http(s) URLs open; it said: {text}"
        );
    }
    for args in [json!({}), json!({"url": 42}), json!({"url": ["http://a/"]})] {
        let reply = a.shim.call("open_url", args.clone());
        assert!(reply.is_error(), "open_url {args} was accepted");
        assert!(
            reply.text().contains("url"),
            "the error for {args} should name the url argument: {}",
            reply.text()
        );
    }
    assert!(
        a.browser.launches().is_empty(),
        "a refused URL still started the browser"
    );

    if !a.has_browser() {
        return;
    }
    // The session still works.
    a.web.page("/fine", Page::solid("Fine", YELLOW, ""));
    a.shim.open(&a.web.url("/fine"));
    assert!(a.shim.screenshot().shows(YELLOW));
}

/// Without a browser to start, the agent is told so in plain words, and
/// the session goes on.
#[test]
fn the_agent_is_told_when_there_is_no_browser() {
    let env = Env::new();
    let missing = env.path("bin/no-such-browser");
    let _daemon = Daemon::with_browser(&env, &missing);
    let mut shim = Shim::ready(&env);
    let reply = shim.call("open_url", json!({"url": "http://127.0.0.1:9/"}));
    assert!(reply.is_error(), "open_url without a browser succeeded");
    assert!(
        reply.text().to_lowercase().contains("browser"),
        "the error should say the browser is missing: {}",
        reply.text()
    );
    // The session goes on.
    shim.screenshot();
}

/// A cookie lasts the session, not beyond: each session's browser starts
/// fresh.
#[test]
fn a_new_session_has_no_cookies_from_the_last_one() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page(
        "/login",
        Page::solid("Logged in", ORANGE, "")
            .header("Set-Cookie: hc26=signed-in; Max-Age=86400; Path=/"),
    );
    a.web.page("/check", Page::solid("Check", BLUE, ""));

    a.shim.open(&a.web.url("/login"));
    a.shim.open(&a.web.url("/check"));
    let first = a.web.wait_for("/check");
    assert!(
        first.cookie.contains("hc26=signed-in"),
        "within a session the browser should keep its cookie; /check got {:?}",
        first.cookie
    );

    // The agent leaves, and comes back as a new session.
    a.shim.close_stdin();
    let gone = wait_until(TIMEOUT, || a.browser_procs().is_empty());
    assert!(gone, "the first session's browser is still running");
    let mut again = a.another();
    again.open(&a.web.url("/check"));
    assert!(again.screenshot().shows(BLUE), "the page isn't shown");
    let checks = a.web.requests_for("/check");
    let last = checks.last().unwrap();
    assert!(
        !last.cookie.contains("hc26"),
        "the new session's browser sent the last session's cookie: {:?}",
        last.cookie
    );
}

/// The agent's browser never uses the user's own browser profile.
#[test]
fn the_users_browser_profile_is_left_alone() {
    let mut a = Agent::start();
    // The user's own profiles, as Chromium and Chrome keep them.
    let mut before = Vec::new();
    for dir in [".config/chromium", ".config/google-chrome"] {
        let profile = a.env.home().join(dir);
        fs::create_dir_all(profile.join("Default")).unwrap();
        let state = profile.join("Local State");
        let prefs = profile.join("Default/Preferences");
        fs::write(&state, "{\"users\": \"the user's\"}").unwrap();
        fs::write(&prefs, "{\"homepage\": \"the user's\"}").unwrap();
        before.push((
            profile,
            fs::read(&state).unwrap(),
            fs::read(&prefs).unwrap(),
        ));
    }
    if !a.has_browser() {
        return;
    }
    a.web.page(
        "/page",
        Page::solid("Page", MAGENTA, "").header("Set-Cookie: x=1; Max-Age=86400"),
    );
    a.shim.open(&a.web.url("/page"));
    assert!(a.shim.screenshot().shows(MAGENTA));
    a.shim.close_stdin();
    wait_until(TIMEOUT, || a.browser_procs().is_empty());

    for (profile, state, prefs) in before {
        let mut files: Vec<String> = fs::read_dir(profile.join("Default"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(
            files,
            ["Preferences"],
            "the agent's browser wrote into the user's profile {}",
            profile.display()
        );
        assert_eq!(fs::read(profile.join("Local State")).unwrap(), state);
        assert_eq!(
            fs::read(profile.join("Default/Preferences")).unwrap(),
            prefs
        );
    }
}

/// Two agents at once: each one's page opens on its own screen, in its own
/// browser.
#[test]
fn two_agents_each_see_their_own_page() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/a", Page::solid("A", ORANGE, ""));
    a.web.page("/b", Page::solid("B", BLUE, ""));
    let mut b = a.another();

    a.shim.open(&a.web.url("/a"));
    b.open(&a.web.url("/b"));

    let shot_b = b.screenshot();
    assert!(
        shot_b.shows(BLUE),
        "agent B doesn't see its page: {shot_b:?}"
    );
    let shot_a = a.shim.screenshot();
    assert!(
        shot_a.shows(ORANGE),
        "agent A's screen changed when B opened a page: {shot_a:?}"
    );
}
