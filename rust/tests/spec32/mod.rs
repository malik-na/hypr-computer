//! Helpers for the black-box tests of #32 ("Limit: allowed websites"),
//! written from the spec (#32, specs/Limits.tla) by an independent test
//! agent, on top of #31's (spec31/, whose top says how `hypr-computer allow`
//! works).
//!
//! # The interface these tests assume
//!
//! - **The proxy.** The daemon runs a filtering HTTP proxy on 127.0.0.1, on
//!   a port of its choosing. It takes absolute-form requests
//!   (`GET http://host:port/path HTTP/1.1`) and `CONNECT host:port`, and goes
//!   by the host in the request line, never the `Host` header; a URL's
//!   `user@` part is not its host. It answers anything else with an error
//!   and connects nowhere.
//! - **The browser is always started pointed at it,** limits or not: its
//!   arguments include `--proxy-server=<127.0.0.1:port>` (with or without
//!   `http://`) and `--proxy-bypass-list=<-loopback>`, so that pages on
//!   127.0.0.1, localhost and the like go through the proxy too, and nothing
//!   that turns it off (`--no-proxy-server`, `--proxy-pac-url`,
//!   `--proxy-auto-detect`). Every app `launch_app` starts gets it as
//!   `http_proxy`, `https_proxy`, `HTTP_PROXY`, `HTTPS_PROXY`, `all_proxy`
//!   and `ALL_PROXY` (`http://127.0.0.1:<port>`), with no `no_proxy` or
//!   `NO_PROXY`. `launch_app` of a web browser is refused at all times,
//!   saying to use `open_url` (`domain::limits::is_browser`, and the
//!   daemon's own browser by its path). Programs that ignore the proxy
//!   aren't limited; the docs say so.
//! - **`hypr-computer allow --sites SITE,...`.** The proxy lets a request
//!   through only to a listed site: the host itself or a subdomain of it,
//!   any port, http or https; an IP address matches only itself. Every other
//!   request gets `403` and the proxy's page, an HTML page that names the
//!   host, says hypr-computer blocked it, and tells the agent to ask the
//!   user; a refused `CONNECT` gets `403` and no tunnel. Nothing reaches an
//!   unlisted site, whether it was the page `open_url` asked for, a redirect
//!   (HTTP or script), or a request of the page's own (an image, a frame, a
//!   `fetch`). Without a site limit every site is let through.
//! - **`open_url`** of a page on an unlisted site is refused as a tool error
//!   that names the host and tells the agent to ask the user, before the
//!   browser asks for it. If the page `open_url` lands on, after redirects,
//!   is on an unlisted site, the call fails the same way. Private windows
//!   are limited the same.
//!
//! # Sites without the internet
//!
//! Every site here is a server in the test on its own loopback address:
//! 127.0.0.2, 127.0.0.3 and so on (Linux routes all of 127.0.0.0/8 to the
//! loopback device), so two sites have two hosts. `localhost` is a name the
//! machine itself resolves. Nothing needs DNS or the internet.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

#[path = "../spec31/mod.rs"]
pub mod spec31;

pub use spec31::*;

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One request a site got.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// The request target as it arrived (a path: the proxy forwards
    /// origin-form, or the absolute URL it was given).
    pub target: String,
    pub host: String,
}

impl Request {
    /// The path part of the target.
    pub fn path(&self) -> String {
        let t = self.target.as_str();
        match t.split_once("://") {
            Some((_, rest)) => rest
                .find('/')
                .map_or("/".to_owned(), |i| rest[i..].to_owned()),
            None => t.to_owned(),
        }
    }
}

/// What a site answers for a path.
#[derive(Clone)]
pub struct Page {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: String,
    pub headers: Vec<String>,
}

impl Page {
    pub fn html(body: impl Into<String>) -> Self {
        Page {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: body.into(),
            headers: Vec::new(),
        }
    }

    /// A page all of colour `c`, titled `title`, with `extra` HTML in it.
    pub fn solid(title: &str, c: Rgb, extra: &str) -> Self {
        Page::html(format!(
            "<!doctype html><html><head><title>{title}</title></head>\
             <body style=\"margin:0;height:100vh;background:rgb({},{},{})\">{extra}</body></html>",
            c[0], c[1], c[2]
        ))
    }

    /// A redirect to `url`.
    pub fn redirect(url: &str) -> Self {
        Page {
            status: "302 Found",
            content_type: "text/html",
            body: String::new(),
            headers: vec![format!("Location: {url}")],
        }
    }
}

#[derive(Default)]
struct SiteState {
    pages: HashMap<String, Page>,
    log: Vec<Request>,
    connections: usize,
}

/// A web site on its own loopback address; it records every request.
pub struct Site {
    pub addr: SocketAddr,
    state: Arc<Mutex<SiteState>>,
}

impl Site {
    /// A site on `ip` (`127.0.0.2`), on a free port.
    pub fn start(ip: &str) -> Self {
        let listener = TcpListener::bind((ip, 0))
            .unwrap_or_else(|e| panic!("can't listen on {ip}: {e} (is 127.0.0.0/8 on lo?)"));
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(SiteState::default()));
        let shared = state.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                shared.lock().unwrap().connections += 1;
                let state = shared.clone();
                thread::spawn(move || serve(conn, &state));
            }
        });
        Site { addr, state }
    }

    pub fn page(&self, path: &str, page: Page) -> &Self {
        self.state
            .lock()
            .unwrap()
            .pages
            .insert(path.to_owned(), page);
        self
    }

    /// `http://ip:port/path`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().log.clone()
    }

    /// How many connections were made to the site at all.
    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    /// The paths asked for, leaving out the favicon.
    pub fn paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(Request::path)
            .filter(|p| p != "/favicon.ico")
            .collect()
    }

    /// Waits for a request for `path`.
    pub fn wait_for(&self, path: &str) -> bool {
        wait_until(TIMEOUT, || self.paths().iter().any(|p| p == path))
    }

    /// Fails the test if the site got any connection, after waiting `QUIET`
    /// for a late one.
    #[track_caller]
    pub fn assert_untouched(&self, why: &str) {
        thread::sleep(QUIET);
        assert_eq!(
            self.connections(),
            0,
            "{why}: the site {} must never be reached, yet it got {:?}",
            self.addr,
            self.requests()
        );
    }
}

fn serve(conn: TcpStream, state: &Mutex<SiteState>) {
    let Ok(read_half) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("").to_owned();
    let target = words.next().unwrap_or("").to_owned();
    let mut host = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("host")
        {
            host = value.trim().to_owned();
        }
    }
    let request = Request {
        method,
        target,
        host,
    };
    let path = request.path();
    state.lock().unwrap().log.push(request);
    let page = state.lock().unwrap().pages.get(&path).cloned();
    let page = page.unwrap_or(Page {
        status: "404 Not Found",
        ..Page::html("not found")
    });
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n",
        page.status,
        page.content_type,
        page.body.len()
    );
    for h in &page.headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut conn = conn;
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(page.body.as_bytes());
    let _ = conn.flush();
}

/// A TCP server that answers nothing: it records each connection and the
/// first bytes it got, to see whether a tunnel reached it, and with what.
pub struct Raw {
    pub addr: SocketAddr,
    got: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Raw {
    pub fn start(ip: &str) -> Self {
        let listener = TcpListener::bind((ip, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let shared = got.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let shared = shared.clone();
                let i = {
                    let mut all = shared.lock().unwrap();
                    all.push(Vec::new());
                    all.len() - 1
                };
                thread::spawn(move || {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = [0u8; 512];
                    if let Ok(n) = conn.read(&mut buf) {
                        shared.lock().unwrap()[i].extend_from_slice(&buf[..n]);
                    }
                    thread::sleep(Duration::from_secs(5));
                });
            }
        });
        Raw { addr, got }
    }

    /// What each connection sent first.
    pub fn connections(&self) -> Vec<Vec<u8>> {
        self.got.lock().unwrap().clone()
    }
}

/// The proxy, as the browser is told of it: `host:port` from its
/// `--proxy-server` argument.
pub fn proxy_of(args: &[String]) -> Option<String> {
    let value = args
        .iter()
        .find_map(|a| a.strip_prefix("--proxy-server="))?;
    let value = value.trim_matches('"');
    let value = value.strip_prefix("http://").unwrap_or(value);
    Some(value.trim_end_matches('/').to_owned())
}

/// What the proxy answered: its status code, headers and body.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub head: String,
    pub body: String,
}

/// Sends `request` (a whole request, CRLFs and all) to the proxy at `proxy`
/// and reads its whole answer.
pub fn ask_proxy(proxy: &str, request: &str) -> Answer {
    let mut conn = TcpStream::connect(proxy)
        .unwrap_or_else(|e| panic!("can't connect to the proxy at {proxy}: {e}"));
    conn.set_read_timeout(Some(TIMEOUT)).unwrap();
    conn.write_all(request.as_bytes()).unwrap();
    let mut all = Vec::new();
    let _ = conn.read_to_end(&mut all);
    parse_answer(&all)
}

fn parse_answer(all: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(all).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Answer {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

/// `GET <url>` through the proxy, with `Host: <host>`.
pub fn get_via(proxy: &str, url: &str, host: &str) -> Answer {
    ask_proxy(
        proxy,
        &format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
    )
}

/// `CONNECT <target>` through the proxy. On a 200 it sends `then` down the
/// tunnel. Returns the proxy's status code.
pub fn connect_via(proxy: &str, target: &str, then: &[u8]) -> u16 {
    let mut conn = TcpStream::connect(proxy).unwrap();
    conn.set_read_timeout(Some(TIMEOUT)).unwrap();
    conn.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match conn.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let status = parse_answer(&head).status;
    if status == 200 {
        conn.write_all(then).unwrap();
        thread::sleep(Duration::from_millis(500));
    }
    status
}

/// This machine's Chromium-family browser: `$HC32_BROWSER` if set (`none`
/// for no browser), otherwise the first of the usual names on PATH.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(given) = std::env::var_os("HC32_BROWSER") {
        return (given != "none").then(|| PathBuf::from(given));
    }
    let path = std::env::var_os("PATH")?;
    [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
    ]
    .iter()
    .find_map(|name| {
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// The real browser, started through a wrapper named `chromium` that notes
/// each launch's arguments, as the fake one does, and marks the browser's
/// processes so they can be found and ended after the test.
pub struct RealBrowser {
    pub path: PathBuf,
    dir: PathBuf,
    mark: String,
}

impl RealBrowser {
    pub fn new(env: &Env, real: &Path) -> Self {
        let dir = env.path("real-browser");
        fs::create_dir_all(&dir).unwrap();
        let mark = env
            .path("")
            .display()
            .to_string()
            .replace('/', "")
            .to_owned();
        let path = env.script(
            "chromium",
            &format!(
                "echo $$ >> {pids}\n\
                 {{ echo '--- launch'; printf '%s\\n' \"$@\"; }} >> {args}\n\
                 export HC32_MARK={mark}\n\
                 exec {real} \"$@\"",
                pids = quote(&dir.join("pids").to_string_lossy()),
                args = quote(&dir.join("args").to_string_lossy()),
                mark = quote(&mark),
                real = quote(&real.to_string_lossy()),
            ),
        );
        RealBrowser { path, dir, mark }
    }

    /// The arguments of every launch, each launch's apart.
    pub fn args(&self) -> Vec<Vec<String>> {
        let text = fs::read_to_string(self.dir.join("args")).unwrap_or_default();
        let mut launches: Vec<Vec<String>> = Vec::new();
        for line in text.lines() {
            if line == "--- launch" {
                launches.push(Vec::new());
            } else if let Some(last) = launches.last_mut() {
                last.push(line.to_owned());
            }
        }
        launches
    }

    pub fn launches(&self) -> usize {
        self.args().len()
    }
}

impl Drop for RealBrowser {
    /// Ends any process of this browser that outlived the test.
    fn drop(&mut self) {
        let needle = format!("HC32_MARK={}", self.mark);
        let Ok(dir) = fs::read_dir("/proc") else {
            return;
        };
        for pid in dir
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_string_lossy().parse::<i32>().ok())
        {
            let environ = fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            if environ.split(|b| *b == 0).any(|kv| kv == needle.as_bytes()) {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}

/// A daemon whose browser is this machine's Chromium, two sites (127.0.0.2
/// and 127.0.0.3), and one agent whose screen is ready. (Fields drop in this
/// order: the agent first.)
pub struct Web {
    pub agent: Agent,
    pub world: World,
    pub browser: RealBrowser,
    pub listed: Site,
    pub unlisted: Site,
    found: bool,
}

impl Web {
    pub fn start() -> Self {
        let real = find_browser();
        let found = real.is_some();
        let real = real.unwrap_or_else(|| PathBuf::from("/nonexistent/chromium"));
        let mut browser = None;
        let mut world = World::with_browser(|env| {
            let b = RealBrowser::new(env, &real);
            let path = b.path.clone();
            browser = Some(b);
            path
        });
        let agent = world.agent("alpha");
        Web {
            agent,
            world,
            browser: browser.unwrap(),
            listed: Site::start("127.0.0.2"),
            unlisted: Site::start("127.0.0.3"),
            found,
        }
    }

    /// Whether this machine has a browser; if not, says the test is skipped.
    pub fn has_browser(&self) -> bool {
        if !self.found {
            eprintln!("SKIPPED: no Chromium-family browser is installed on this machine");
        }
        self.found
    }

    /// `open_url` of `url`, whatever comes back.
    pub fn open(&mut self, url: &str) -> Reply {
        self.agent
            .shim
            .call("open_url", serde_json::json!({"url": url}))
    }

    /// Screenshots until one shows colour `c` over most of the screen.
    pub fn screen_shows(&mut self, c: Rgb) -> bool {
        let mut last = None;
        let ok = wait_until(TIMEOUT, || {
            let shot = self.agent.shim.screenshot();
            let ok = shot.share(c) > 0.5;
            last = Some(shot);
            ok
        });
        if !ok {
            eprintln!("the screen never showed {c:?}: {last:?}");
        }
        ok
    }

    /// The proxy the browser was started with.
    pub fn proxy(&self) -> String {
        let args = self.browser.args();
        let first = args.first().cloned().unwrap_or_default();
        proxy_of(&first)
            .unwrap_or_else(|| panic!("the browser wasn't started with --proxy-server: {first:?}"))
    }
}

/// A daemon whose browser is a fake that only records how it was started,
/// and one agent whose `open_url` has started it once: for the tests that
/// talk to the proxy themselves, with no real browser needed.
pub struct Proxied {
    pub agent: Agent,
    pub world: World,
    pub fake: FakeBrowser,
    pub proxy: String,
}

impl Proxied {
    pub fn start() -> Self {
        let mut fake = None;
        let mut world = World::with_browser(|env| {
            let browser = env.fake_browser();
            let path = browser.path.clone();
            fake = Some(browser);
            path
        });
        let fake = fake.unwrap();
        let mut agent = world.agent("alpha");
        // Not waited for: the fake never shows a page.
        agent.shim.begin(
            "open_url",
            serde_json::json!({"url": "http://127.0.0.2:9/start"}),
        );
        let started = wait_until(TIMEOUT, || !fake.args().is_empty());
        assert!(started, "open_url never started the browser");
        let args = fake.args().remove(0);
        let proxy = proxy_of(&args)
            .unwrap_or_else(|| panic!("the browser wasn't started with --proxy-server: {args:?}"));
        Proxied {
            agent,
            world,
            fake,
            proxy,
        }
    }
}

/// How long to give the browser to make a request it shouldn't.
pub const LATE: Duration = Duration::from_secs(3);

/// Waits `LATE`, then returns how long it waited.
pub fn give_it_time() -> Instant {
    thread::sleep(LATE);
    Instant::now()
}
