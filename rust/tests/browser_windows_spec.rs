//! #26, part 4: every window the agent's browser opens (a private window, a
//! popup window a page opens, a new window) opens on the agent's screen, in
//! front, and takes clicks there; and all of them end with the session.
//! Written from the spec alone.
//!
//! The tests pass with a SKIPPED note where no Chromium-family browser is
//! installed (or `HC26_BROWSER=none`); every test first checks that the agent
//! has `open_url`.
#![forbid(unsafe_code)]

mod spec26;

use std::thread;
use std::time::Duration;

use serde_json::json;
use spec26::procs::{describe, running};
use spec26::web::Page;
use spec26::{Agent, BLUE, CYAN, GREEN, MAGENTA, ORANGE, YELLOW, centre, css, wait_until};

/// `open_url` with `private: true` opens the page in a private window on
/// the agent's screen: it has none of the session's cookies, and it ends
/// with the session.
#[test]
fn a_private_window_opens_on_the_agents_screen() {
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
    a.web.page("/private", Page::solid("Private", YELLOW, ""));
    a.shim.open(&a.web.url("/login"));

    a.shim.ok(
        "open_url",
        json!({"url": a.web.url("/private"), "private": true}),
    );
    let shot = a.shim.screenshot();
    assert!(
        shot.shows(YELLOW),
        "the private window's page isn't in front on the agent's screen: {shot:?}"
    );
    let private = a.web.wait_for("/private");
    assert!(
        !private.cookie.contains("hc26"),
        "the private window sent the session's cookie, so it isn't private: {:?}",
        private.cookie
    );

    // The normal window still has the session's cookie.
    a.shim.open(&a.web.url("/check"));
    assert!(
        a.shim.screenshot().shows(BLUE),
        "the normal page isn't shown"
    );
    assert!(
        a.web.wait_for("/check").cookie.contains("hc26=signed-in"),
        "the normal window lost the session's cookie"
    );

    thread::sleep(Duration::from_secs(1));
    let seen = a.browser_procs();
    a.shim.close_stdin();
    let gone = wait_until(Duration::from_secs(5), || {
        seen.iter().all(|p| !running(*p)) && a.browser_procs().is_empty()
    });
    assert!(
        gone,
        "after the session ended, its browser windows' processes were still running:\n{}",
        describe(&seen.iter().copied().filter(|p| running(*p)).collect())
    );
}

/// A page that opens a popup window when clicked anywhere; the popup turns
/// cyan when it is clicked.
fn opener_pages(a: &Agent) {
    a.web.page(
        "/opener",
        Page::solid(
            "Opener",
            GREEN,
            "document.onclick = () => window.open('/popup', 'hc26', 'popup,width=500,height=400');",
        ),
    );
    a.web.page(
        "/popup",
        Page::solid(
            "Popup",
            ORANGE,
            &format!(
                "document.onclick = () => document.body.style.background = '{}';",
                css(CYAN)
            ),
        ),
    );
}

#[test]
fn a_popup_window_a_page_opens_is_on_the_agents_screen_and_takes_a_click() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    opener_pages(&a);
    a.shim.open(&a.web.url("/opener"));
    let page = a.shim.screenshot();
    assert!(page.shows(GREEN), "the page isn't shown: {page:?}");
    let (x, y) = centre(page.bbox(GREEN).unwrap());

    a.shim.ok("click", json!({"x": x, "y": y}));
    a.web.wait_for("/popup");
    let (shown, shot) = a
        .shim
        .screenshot_until(Duration::from_secs(5), |s| s.share(ORANGE) > 0.1);
    assert!(
        shown,
        "the popup window the page opened isn't on the agent's screen: {shot:?}"
    );

    let (px, py) = centre(shot.bbox(ORANGE).unwrap());
    a.shim.ok("click", json!({"x": px, "y": py}));
    let (clicked, shot) = a
        .shim
        .screenshot_until(Duration::from_secs(5), |s| s.share(CYAN) > 0.1);
    assert!(
        clicked,
        "a click on the popup window (at {px},{py}) didn't reach it: {shot:?}"
    );
}

/// A new window (Ctrl+N) opens in front on the agent's screen; closing it
/// (Ctrl+W) shows the page again.
#[test]
fn a_new_window_opens_in_front_on_the_agents_screen() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/page", Page::solid("Page", MAGENTA, ""));
    a.shim.open(&a.web.url("/page"));
    let page = a.shim.screenshot();
    assert!(page.shows(MAGENTA), "the page isn't shown: {page:?}");

    a.shim.ok("key", json!({"keys": "ctrl+n"}));
    let (opened, shot) = a
        .shim
        .screenshot_until(Duration::from_secs(5), |s| s.share(MAGENTA) < 0.05);
    assert!(
        opened,
        "after Ctrl+N no new window covered the page on the agent's screen: {shot:?}"
    );
    assert_eq!((shot.width, shot.height), (spec26::WIDTH, spec26::HEIGHT));

    a.shim.ok("key", json!({"keys": "ctrl+w"}));
    let (back, shot) = a
        .shim
        .screenshot_until(Duration::from_secs(5), |s| s.shows(MAGENTA));
    assert!(
        back,
        "closing the new window didn't bring the page back: {shot:?}"
    );
}
