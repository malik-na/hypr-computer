//! #32: an agent can't start the browser without the proxy. The daemon
//! always starts it pointed at its proxy, limits or not; `launch_app` of a
//! browser is refused; and every app on the screen is told of the proxy.
//! Written from the spec (#32, specs/Limits.tla) alone; spec32/ says what
//! interface it assumes. None of these needs a real browser.
#![forbid(unsafe_code)]

mod spec32;

use std::fs;
use std::thread;

use serde_json::json;
use spec32::{
    FakeBrowser, Page, Proxied, QUIET, Site, TIMEOUT, World, get_via, proxy_of, wait_until,
};

/// The proxy the browser was started with must be live: a request through it
/// reaches a site.
#[track_caller]
fn assert_live_proxy(proxy: &str) {
    let site = Site::start("127.0.0.3");
    site.page("/alive", Page::html("alive"));
    let got = get_via(proxy, &site.url("/alive"), &site.addr.to_string());
    assert!(
        got.status == 200 && got.body.contains("alive"),
        "a request through the browser's proxy {proxy} didn't reach the site: {got:?}"
    );
}

#[track_caller]
fn assert_pointed_at_proxy(args: &[String]) -> String {
    let proxy = proxy_of(args)
        .unwrap_or_else(|| panic!("the browser was started without --proxy-server: {args:?}"));
    assert!(
        proxy.starts_with("127.0.0.1:"),
        "the proxy must be the daemon's, on 127.0.0.1: {proxy}"
    );
    assert!(
        args.iter().any(|a| a == "--proxy-bypass-list=<-loopback>"),
        "without --proxy-bypass-list=<-loopback>, pages on this machine skip the proxy: {args:?}"
    );
    for off in [
        "--no-proxy-server",
        "--proxy-pac-url",
        "--proxy-auto-detect",
    ] {
        assert!(
            !args.iter().any(|a| a.starts_with(off)),
            "the browser was started with {off}: {args:?}"
        );
    }
    proxy
}

#[test]
fn the_browser_is_always_started_pointed_at_the_proxy() {
    let p = Proxied::start();
    let args = p.fake.args().remove(0);
    let proxy = assert_pointed_at_proxy(&args);
    assert_eq!(proxy, p.proxy);
    assert_live_proxy(&proxy);
}

#[test]
fn the_browser_is_pointed_at_the_proxy_under_a_site_limit_and_after_a_restart() {
    let mut fake = None;
    let mut world = World::with_browser(|env| {
        let browser = env.fake_browser();
        let path = browser.path.clone();
        fake = Some(browser);
        path
    });
    let fake: FakeBrowser = fake.unwrap();
    world.user_runs(&["allow", "--sites", "127.0.0.2"]);
    let mut agent = world.agent("alpha");
    agent
        .shim
        .begin("open_url", json!({"url": "http://127.0.0.2:9/"}));
    assert!(wait_until(TIMEOUT, || !fake.launches().is_empty()));
    let first = fake.args().remove(0);
    let proxy = assert_pointed_at_proxy(&first);
    assert_live_proxy_refuses_unlisted(&proxy);

    // The browser goes away (the agent closed it, or it crashed): the one
    // that starts next is pointed at the proxy too.
    for pid in fake.launches() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    let mut second = World::agent(&mut world, "beta");
    second
        .shim
        .begin("open_url", json!({"url": "http://127.0.0.2:9/"}));
    assert!(wait_until(TIMEOUT, || fake.launches().len() >= 2));
    let again = fake.args().remove(1);
    assert_pointed_at_proxy(&again);
}

#[track_caller]
fn assert_live_proxy_refuses_unlisted(proxy: &str) {
    let site = Site::start("127.0.0.3");
    let got = get_via(proxy, &site.url("/"), &site.addr.to_string());
    assert_eq!(
        got.status, 403,
        "the site limit must hold at the proxy: {got:?}"
    );
}

/// An agent can't get a browser that skips the proxy by launching one
/// itself: launch_app of a browser is refused, whatever its limits, and says
/// to use open_url.
#[test]
fn launch_app_of_a_browser_is_refused() {
    let mut fake = None;
    let mut world = World::with_browser(|env| {
        let browser = env.fake_browser();
        let path = browser.path.clone();
        fake = Some(browser);
        path
    });
    let fake: FakeBrowser = fake.unwrap();
    let mut agent = world.agent("alpha");

    // The daemon's own browser, by its path.
    let reply = agent.shim.call(
        "launch_app",
        json!({"app": fake.path.display().to_string(), "args": ["--no-proxy-server"]}),
    );
    assert!(
        reply.is_error() && reply.text().contains("open_url"),
        "launch_app of the agent's browser must be refused, saying to use open_url: {}",
        reply.raw
    );
    // Browsers by name, wherever they are.
    for name in [
        "chromium",
        "google-chrome-stable",
        "firefox",
        "brave-browser",
    ] {
        let (reply, app) = agent.try_launch(
            &world.env,
            &format!("other/{name}"),
            (spec32::GREEN, spec32::BLUE),
            &[],
        );
        assert!(
            reply.is_error() && reply.text().contains("open_url"),
            "launch_app of {name} must be refused, saying to use open_url: {}",
            reply.raw
        );
        thread::sleep(std::time::Duration::from_millis(200));
        assert!(!app.log.exists(), "{name} was launched");
    }
    thread::sleep(QUIET);
    assert!(
        fake.launches().is_empty(),
        "the browser was started by launch_app: {:?}",
        fake.args()
    );
}

/// Every app on the screen is told of the proxy, so that a browser or a tool
/// it starts that honours the usual variables uses it too.
#[test]
fn apps_on_the_screen_are_told_of_the_proxy() {
    let mut world = World::new();
    let mut agent = world.agent("alpha");
    let out = world.env.path("app-env");
    let dump = world.env.script(
        "envdump",
        &format!("env > {}", spec32::quote(&out.to_string_lossy())),
    );
    let reply = agent
        .shim
        .call("launch_app", json!({"app": dump.display().to_string()}));
    assert!(
        wait_until(TIMEOUT, || fs::read_to_string(&out)
            .is_ok_and(|s| s.contains("PATH="))),
        "the app never ran: {}",
        reply.raw
    );
    let env: Vec<(String, String)> = fs::read_to_string(&out)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let get = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let proxy = get("http_proxy").unwrap_or_else(|| panic!("no http_proxy in {env:?}"));
    for name in [
        "https_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        assert_eq!(
            get(name).as_deref(),
            Some(proxy.as_str()),
            "{name} in {env:?}"
        );
    }
    for name in ["no_proxy", "NO_PROXY"] {
        assert!(
            get(name).is_none_or(|v| v.is_empty()),
            "{name} lets programs skip the proxy: {env:?}"
        );
    }
    let address = proxy
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("the proxy should be an http:// URL: {proxy}"))
        .trim_end_matches('/');
    assert!(address.starts_with("127.0.0.1:"), "{proxy}");
    assert_live_proxy(address);
}
