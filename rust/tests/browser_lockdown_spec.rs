//! #26, part 2: the agent's browser is locked down. No extension opens a
//! page in it, even one the system's browser command adds; it starts with
//! accessibility on; and nothing of it is left once the session ends, however
//! the agent ends. Written from the spec alone.
//!
//! The tests pass with a SKIPPED note where no Chromium-family browser is
//! installed (or `HC26_BROWSER=none`); every test first checks that the agent
//! has `open_url`.
#![forbid(unsafe_code)]

mod spec26;

use std::fs;
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use spec26::procs::{self, describe, running, tree};
use spec26::web::{Page, Web};
use spec26::{Agent, BLUE, Env, MAGENTA, ORANGE, TIMEOUT, browser, wait_until};

/// An extension that opens a page of its own as soon as it is installed, as
/// the one in the past bug did.
fn opener_extension(env: &Env, web: &Web) -> std::path::PathBuf {
    let dir = env.path("extension");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        r#"{"manifest_version": 3, "name": "opener", "version": "1",
            "background": {"service_worker": "bg.js"}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("bg.js"),
        format!(
            "chrome.runtime.onInstalled.addListener(() => chrome.tabs.create({{url: '{}'}}));",
            web.url("/from-extension")
        ),
    )
    .unwrap();
    dir
}

/// Whether this browser, started plainly (headless, off any screen) with
/// `--load-extension`, lets the extension open its page: otherwise the test
/// below would prove nothing.
fn extension_opens_its_page(env: &Env, web: &Web, ext: &std::path::Path) -> bool {
    let real = browser::find().unwrap();
    let mut child = env
        .command(&real)
        .args([
            "--headless=new",
            "--no-first-run",
            &format!("--user-data-dir={}", env.path("control-profile").display()),
            &format!("--load-extension={}", ext.display()),
            "about:blank",
        ])
        .env("HC26_MARK", "hc26-control")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let opened = wait_until(TIMEOUT, || !web.requests_for("/from-extension").is_empty());
    let _ = child.kill();
    let _ = child.wait();
    procs::reap("hc26-control");
    opened
}

#[test]
fn no_extension_opens_a_page_in_the_agents_browser() {
    // Pages served before the agent starts, so the extension's path is
    // known; the system's browser command loads the extension everywhere.
    let env = Env::new();
    let web = Web::start();
    let ext = opener_extension(&env, &web);
    let mut a = Agent::start_with(&[format!("--load-extension={}", ext.display())]);
    if !a.has_browser() {
        return;
    }
    if !extension_opens_its_page(&env, &web, &ext) {
        eprintln!("SKIPPED: this browser doesn't load an extension from --load-extension");
        return;
    }
    let seen_before = web.requests_for("/from-extension").len();
    web.page("/page", Page::solid("The agent's page", ORANGE, ""));

    a.shim.open(&web.url("/page"));
    thread::sleep(Duration::from_secs(5));
    let shot = a.shim.screenshot();
    assert!(
        shot.shows(ORANGE),
        "the screen should show the page the agent opened, and nothing else: {shot:?}"
    );
    assert_eq!(
        web.requests_for("/from-extension").len(),
        seen_before,
        "an extension opened its own page in the agent's browser"
    );
    let paths: Vec<String> = web
        .paths()
        .into_iter()
        .filter(|p| p != "/from-extension")
        .collect();
    assert_eq!(
        paths,
        ["/page"],
        "the agent's browser loaded pages it wasn't asked for"
    );
}

#[test]
fn the_browser_starts_with_accessibility_on() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/page", Page::solid("Page", MAGENTA, ""));
    a.shim.open(&a.web.url("/page"));
    let args = a.browser.args();
    assert!(
        args.iter().any(|a| a == "--force-renderer-accessibility"
            || a.starts_with("--force-renderer-accessibility=")),
        "Chromium shows its pages to accessibility tools only if started with \
         --force-renderer-accessibility; it was started with {args:?}"
    );
}

/// Opens a page and returns the browser's processes while it runs.
fn browse(a: &mut Agent) -> std::collections::BTreeSet<procs::Proc> {
    a.web.page("/page", Page::solid("Page", MAGENTA, ""));
    a.shim.open(&a.web.url("/page"));
    assert!(a.shim.screenshot().shows(MAGENTA), "the page isn't shown");
    // Let the browser start its helpers (GPU, network, renderers).
    thread::sleep(Duration::from_secs(2));
    let procs = a.browser_procs();
    assert!(
        procs.len() >= 2,
        "found too few of the browser's processes to check: {}",
        describe(&procs)
    );
    procs
}

fn assert_all_gone(a: &Agent, seen: &std::collections::BTreeSet<procs::Proc>, how: &str) {
    let gone = wait_until(Duration::from_secs(5), || {
        seen.iter().all(|p| !running(*p)) && a.browser_procs().is_empty()
    });
    let mut left: std::collections::BTreeSet<_> =
        seen.iter().copied().filter(|p| running(*p)).collect();
    left.extend(a.browser_procs());
    assert!(
        gone,
        "5 s after the agent {how}, these of its browser's processes were still running:\n{}",
        describe(&left)
    );
}

#[test]
fn nothing_of_the_browser_is_left_when_the_agent_leaves() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    let seen = browse(&mut a);
    a.shim.close_stdin();
    assert_all_gone(&a, &seen, "left");
}

#[test]
fn nothing_of_the_browser_is_left_when_the_agent_is_killed() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    let seen = browse(&mut a);
    a.shim.kill_9();
    assert_all_gone(&a, &seen, "was killed with kill -9");
}

/// The fresh profile the session's browser had is deleted with it.
#[test]
fn the_sessions_browser_profile_is_deleted_when_it_ends() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    let seen = browse(&mut a);
    let profiles = a.browser.profiles();
    assert!(
        !profiles.is_empty(),
        "the browser was started with neither --user-data-dir nor a home of its own: {:?}",
        a.browser.args()
    );
    for p in &profiles {
        assert_ne!(
            *p,
            a.env.home(),
            "the agent's browser used the user's home as its profile"
        );
        assert!(
            p.is_dir(),
            "the profile {} isn't there while the browser runs",
            p.display()
        );
    }
    a.shim.close_stdin();
    assert_all_gone(&a, &seen, "left");
    let ok = wait_until(Duration::from_secs(5), || {
        profiles.iter().all(|p| !p.exists())
    });
    assert!(
        ok,
        "the session's profile is still there after it ended: {profiles:?}"
    );
}

/// One agent leaving takes only its own browser.
#[test]
fn another_agents_browser_keeps_running_when_one_leaves() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/a", Page::solid("A", ORANGE, ""));
    a.web.page("/b", Page::solid("B", BLUE, ""));
    a.web.page("/b2", Page::solid("B2", MAGENTA, ""));
    a.shim.open(&a.web.url("/a"));
    let a_launches = a.browser.launches();
    let mut b = a.another();
    b.open(&a.web.url("/b"));
    thread::sleep(Duration::from_secs(2));
    let b_roots: Vec<u32> = a
        .browser
        .launches()
        .into_iter()
        .filter(|p| !a_launches.contains(p))
        .collect();
    let a_procs = tree(&a_launches);
    let b_procs = tree(&b_roots);
    assert!(
        !a_procs.is_empty() && !b_procs.is_empty(),
        "each agent should have a browser"
    );

    a.shim.close_stdin();
    let gone = wait_until(Duration::from_secs(5), || {
        a_procs.iter().all(|p| !running(*p))
    });
    assert!(
        gone,
        "agent A's browser outlived A:\n{}",
        describe(&a_procs)
    );
    let left: Vec<_> = b_procs.iter().filter(|p| !running(**p)).collect();
    assert!(
        left.is_empty(),
        "agent B's browser processes ended with A: {left:?}"
    );
    assert!(b.screenshot().shows(BLUE), "agent B's page went away");
    b.open(&a.web.url("/b2"));
    assert!(
        b.screenshot().shows(MAGENTA),
        "agent B can't open another page"
    );
}
