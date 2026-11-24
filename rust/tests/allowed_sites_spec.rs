//! #32: `hypr-computer allow --sites ...` limits agents' browsers to the
//! sites listed, through a filtering proxy in the daemon: a listed site
//! loads; an unlisted one, and a redirect to one, are refused with a clear
//! page. Written from the spec (#32, specs/Limits.tla) alone; spec32/ says
//! what interface it assumes, and how the sites work without the internet.
//!
//! The tests that talk to the proxy themselves need no browser. The ones
//! that open pages in the browser pass with a SKIPPED note where no
//! Chromium-family browser is installed (or `HC32_BROWSER=none`).
#![forbid(unsafe_code)]

mod spec32;

use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use spec32::{
    BLUE, GREEN, ORANGE, PURPLE, Page, Proxied, Raw, Site, TEAL, Web, assert_refused, connect_via,
    get_via, give_it_time,
};

// ---- The proxy itself.

#[test]
fn without_a_site_limit_the_proxy_lets_every_site_through() {
    let p = Proxied::start();
    let site = Site::start("127.0.0.3");
    site.page("/hello", Page::html("hello from .3"));
    let got = get_via(&p.proxy, &site.url("/hello"), &site.addr.to_string());
    assert_eq!(got.status, 200, "{got:?}");
    assert!(got.body.contains("hello from .3"), "{got:?}");
    assert_eq!(site.paths(), vec!["/hello"]);
}

#[test]
fn the_proxy_refuses_an_unlisted_site_with_a_clear_page() {
    let p = Proxied::start();
    let listed = Site::start("127.0.0.2");
    let unlisted = Site::start("127.0.0.3");
    listed.page("/ok", Page::html("listed page"));
    unlisted.page("/secret", Page::html("unlisted page"));
    p.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    let got = get_via(&p.proxy, &listed.url("/ok"), &listed.addr.to_string());
    assert_eq!(got.status, 200, "a listed site must load: {got:?}");
    assert!(got.body.contains("listed page"));

    let got = get_via(
        &p.proxy,
        &unlisted.url("/secret"),
        &unlisted.addr.to_string(),
    );
    assert_eq!(got.status, 403, "an unlisted site must be refused: {got:?}");
    let page = got.body.to_lowercase();
    assert!(
        got.head.to_lowercase().contains("text/html")
            && page.contains("127.0.0.3")
            && page.contains("hypr-computer")
            && page.contains("ask the user"),
        "the proxy's page must be HTML that names the site, says hypr-computer blocked it and \
         tells the agent to ask the user: {got:?}"
    );
    assert!(!page.contains("unlisted page"));
    unlisted.assert_untouched("a GET through the proxy for an unlisted site");
}

/// The proxy goes by the URL in the request line, which is where the request
/// goes, never by the Host header, and a URL's `user@` part isn't its host.
#[test]
fn the_proxy_goes_by_the_requests_url_not_its_host_header() {
    let p = Proxied::start();
    let listed = Site::start("127.0.0.2");
    let unlisted = Site::start("127.0.0.3");
    listed.page("/x", Page::html("listed"));
    p.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    // A listed URL with an unlisted Host header goes to the listed site.
    let got = get_via(&p.proxy, &listed.url("/x"), &unlisted.addr.to_string());
    assert_eq!(got.status, 200, "{got:?}");
    assert!(listed.wait_for("/x"));

    // An unlisted URL with a listed Host header is refused.
    let got = get_via(&p.proxy, &unlisted.url("/y"), &listed.addr.to_string());
    assert_eq!(got.status, 403, "{got:?}");

    // A listed name in the user part of an unlisted URL is still unlisted.
    let tricky = format!("http://127.0.0.2@{}/z", unlisted.addr);
    let got = get_via(&p.proxy, &tricky, &unlisted.addr.to_string());
    assert_eq!(got.status, 403, "{tricky}: {got:?}");

    // A request that isn't for the proxy (no URL in its request line) goes
    // nowhere.
    let got = spec32::ask_proxy(
        &p.proxy,
        &format!(
            "GET /y HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            unlisted.addr
        ),
    );
    assert!(got.status >= 400, "{got:?}");
    unlisted.assert_untouched("requests for the unlisted site, disguised");
}

/// HTTPS goes through a CONNECT tunnel, so the proxy sees only the host and
/// port: it refuses an unlisted one before connecting, and tunnels to a
/// listed one.
#[test]
fn https_is_checked_when_the_tunnel_is_asked_for() {
    let p = Proxied::start();
    let listed = Raw::start("127.0.0.2");
    let unlisted = Raw::start("127.0.0.3");
    p.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    let status = connect_via(&p.proxy, &unlisted.addr.to_string(), b"\x16\x03\x01");
    assert_eq!(status, 403, "CONNECT to an unlisted site must be refused");
    thread::sleep(spec32::QUIET);
    assert!(
        unlisted.connections().is_empty(),
        "the proxy connected to the unlisted site: {:?}",
        unlisted.connections()
    );

    let status = connect_via(&p.proxy, &listed.addr.to_string(), b"\x16\x03\x01hello");
    assert_eq!(status, 200, "CONNECT to a listed site must open a tunnel");
    let got = spec32::wait_until(spec32::TIMEOUT, || {
        listed
            .connections()
            .iter()
            .any(|c| c.starts_with(b"\x16\x03\x01hello"))
    });
    assert!(
        got,
        "the tunnel didn't carry the bytes: {:?}",
        listed.connections()
    );
}

/// A site includes its subdomains, a name is not its address, and a site
/// given as a URL is its host.
#[test]
fn sites_are_hosts() {
    let p = Proxied::start();
    let local = Site::start("127.0.0.1");
    local.page("/", Page::html("local"));
    p.world
        .user_runs(&["allow", "--sites", "http://LocalHost:1/some/path"]);
    let port = local.addr.port();

    let got = get_via(&p.proxy, &format!("http://localhost:{port}/"), "localhost");
    assert_eq!(got.status, 200, "localhost is listed: {got:?}");
    let got = get_via(&p.proxy, &format!("http://127.0.0.1:{port}/"), "127.0.0.1");
    assert_eq!(got.status, 403, "127.0.0.1 isn't the name listed: {got:?}");
    let got = get_via(
        &p.proxy,
        &format!("http://localhost.evil.test:{port}/"),
        "localhost.evil.test",
    );
    assert_eq!(got.status, 403, "{got:?}");
    assert_eq!(local.requests().len(), 1, "{:?}", local.requests());
}

/// A site limit set mid-session applies to the next request, and one for a
/// set time stops applying when the time is up.
#[test]
fn a_site_limit_applies_from_the_next_request_until_its_time_is_up() {
    const FOR: Duration = Duration::from_secs(12);
    let p = Proxied::start();
    let site = Site::start("127.0.0.3");
    site.page("/a", Page::html("a"));
    assert_eq!(get_via(&p.proxy, &site.url("/a"), "x").status, 200);

    let set = Instant::now();
    p.world
        .user_runs(&["allow", "--sites", "127.0.0.2", "--for", "12s"]);
    let got = get_via(&p.proxy, &site.url("/a"), "x");
    assert!(
        set.elapsed() < FOR - Duration::from_secs(2),
        "the machine is too slow"
    );
    assert_eq!(got.status, 403, "{got:?}");

    thread::sleep(FOR.saturating_sub(set.elapsed()) + Duration::from_secs(2));
    let got = get_via(&p.proxy, &site.url("/a"), "x");
    assert_eq!(
        got.status, 200,
        "an expired site limit stops applying: {got:?}"
    );

    p.world.user_runs(&["allow", "--sites", "127.0.0.2"]);
    assert_eq!(get_via(&p.proxy, &site.url("/a"), "x").status, 403);
    p.world.user_runs(&["allow", "--off"]);
    assert_eq!(get_via(&p.proxy, &site.url("/a"), "x").status, 200);
}

// ---- In the browser.

#[test]
fn a_listed_site_loads_and_an_unlisted_one_is_refused() {
    let mut w = Web::start();
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);
    if !w.has_browser() {
        return;
    }
    w.listed.page("/", Page::solid("Listed", GREEN, ""));
    w.unlisted.page("/", Page::solid("Unlisted", ORANGE, ""));

    let reply = w.open(&w.listed.url("/"));
    assert!(!reply.is_error(), "a listed site must open: {}", reply.raw);
    assert!(w.screen_shows(GREEN), "the listed page isn't on the screen");

    let url = w.unlisted.url("/");
    let reply = w.open(&url);
    assert_refused(&reply, "127.0.0.3", &format!("open_url {url}"));
    w.unlisted.assert_untouched("open_url of an unlisted site");
    assert!(!w.agent.shim.screenshot().has(ORANGE));
}

#[test]
fn a_redirect_to_an_unlisted_site_is_refused() {
    let mut w = Web::start();
    if !w.has_browser() {
        return;
    }
    w.unlisted
        .page("/landing", Page::solid("Unlisted", ORANGE, ""));
    let target = w.unlisted.url("/landing");
    w.listed.page("/go", Page::redirect(&target));
    w.listed.page(
        "/later",
        Page::solid(
            "Later",
            BLUE,
            &format!("<script>setTimeout(() => location.href = '{target}', 300)</script>"),
        ),
    );
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    // An HTTP redirect: open_url lands on a page of an unlisted site.
    let reply = w.open(&w.listed.url("/go"));
    assert_refused(
        &reply,
        "127.0.0.3",
        "open_url of a page that redirects to an unlisted site",
    );
    assert!(w.listed.wait_for("/go"));

    // A script that sends the browser on after the page has loaded.
    let reply = w.open(&w.listed.url("/later"));
    assert!(!reply.is_error(), "{}", reply.raw);
    give_it_time();
    w.unlisted.assert_untouched("redirects to an unlisted site");
    let shot = w.agent.shim.screenshot();
    assert!(
        !shot.has(ORANGE),
        "the unlisted page is on the screen: {shot:?}"
    );
}

/// A listed page's own requests to an unlisted site (an image, a frame, a
/// fetch) never reach it.
#[test]
fn a_listed_pages_requests_to_unlisted_sites_never_reach_them() {
    let mut w = Web::start();
    if !w.has_browser() {
        return;
    }
    let other = w.unlisted.addr;
    w.listed.page(
        "/page",
        Page::solid(
            "Page",
            TEAL,
            &format!(
                "<img src=\"http://{other}/pixel.png\" width=1 height=1>\
                 <iframe src=\"http://{other}/frame\" width=10 height=10></iframe>\
                 <script>fetch('http://{other}/fetch', {{mode: 'no-cors'}}).catch(() => {{}});\
                 new Image().src = 'https://{other}/tls.png';</script>"
            ),
        ),
    );
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    let reply = w.open(&w.listed.url("/page"));
    assert!(!reply.is_error(), "{}", reply.raw);
    assert!(w.screen_shows(TEAL));
    give_it_time();
    w.unlisted
        .assert_untouched("a listed page's requests to an unlisted site");
}

#[test]
fn a_private_window_is_limited_too() {
    let mut w = Web::start();
    if !w.has_browser() {
        return;
    }
    w.listed.page("/p", Page::solid("Private", PURPLE, ""));
    w.unlisted.page("/p", Page::solid("Unlisted", ORANGE, ""));
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    let reply = w.agent.shim.call(
        "open_url",
        json!({"url": w.listed.url("/p"), "private": true}),
    );
    assert!(!reply.is_error(), "{}", reply.raw);
    assert!(w.screen_shows(PURPLE));
    let url = w.unlisted.url("/p");
    let reply = w
        .agent
        .shim
        .call("open_url", json!({"url": url, "private": true}));
    assert_refused(
        &reply,
        "127.0.0.3",
        "open_url of an unlisted site in a private window",
    );
    w.unlisted.assert_untouched("a private window");
}

/// Pages on loopback addresses go through the proxy too: without that, a
/// site limit wouldn't cover the services on the user's own machine.
#[test]
fn an_unlisted_page_on_the_users_own_machine_is_refused() {
    let mut w = Web::start();
    if !w.has_browser() {
        return;
    }
    let local = Site::start("127.0.0.1");
    local.page("/", Page::solid("Local", ORANGE, ""));
    w.listed.page("/", Page::solid("Listed", GREEN, ""));
    w.listed.page(
        "/link",
        Page::solid(
            "Link",
            BLUE,
            &format!(
                "<script>setTimeout(() => location.href = 'http://localhost:{}/', 300)</script>",
                local.addr.port()
            ),
        ),
    );
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);
    let reply = w.open(&w.listed.url("/link"));
    assert!(!reply.is_error(), "{}", reply.raw);
    give_it_time();
    local.assert_untouched("a page on localhost the user didn't list");
}

/// An https page of a listed site is tunnelled to it; one of an unlisted site
/// is refused before anything reaches it.
#[test]
fn https_pages_are_limited_by_host() {
    let mut w = Web::start();
    if !w.has_browser() {
        return;
    }
    let listed = Raw::start("127.0.0.2");
    let unlisted = Raw::start("127.0.0.3");
    w.world.user_runs(&["allow", "--sites", "127.0.0.2"]);

    let url = format!("https://{}/", unlisted.addr);
    let reply = w.open(&url);
    assert_refused(&reply, "127.0.0.3", &format!("open_url {url}"));

    // The listed one gets the browser's TLS handshake through the tunnel (it
    // never answers, so the page itself doesn't load).
    let _ = w.open(&format!("https://{}/", listed.addr));
    let tunnelled = spec32::wait_until(spec32::TIMEOUT, || {
        listed
            .connections()
            .iter()
            .any(|c| c.first() == Some(&0x16))
    });
    assert!(
        tunnelled,
        "no TLS handshake reached the listed site: {:?}",
        listed.connections()
    );
    thread::sleep(spec32::QUIET);
    assert!(
        unlisted.connections().is_empty(),
        "{:?}",
        unlisted.connections()
    );
}
