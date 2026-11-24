//! A tiny HTTP server on 127.0.0.1 that serves the test's pages and records
//! every request the browser makes, so a test can see what was loaded, when,
//! and with which cookies.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{Rgb, TIMEOUT, css, wait_until};

/// One request the browser made.
#[derive(Clone, Debug)]
pub struct Request {
    pub path: String,
    /// The Cookie header, or "" without one.
    pub cookie: String,
    /// When the server finished answering it.
    pub answered: Instant,
}

/// What the server answers for a path.
#[derive(Clone)]
pub struct Page {
    pub content_type: &'static str,
    pub body: String,
    /// Extra header lines, like `Set-Cookie: a=b`.
    pub headers: Vec<String>,
    /// How long to wait before answering.
    pub delay: Duration,
}

impl Page {
    pub fn html(body: impl Into<String>) -> Self {
        Page {
            content_type: "text/html; charset=utf-8",
            body: body.into(),
            headers: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    /// A page that is all colour `c`, titled `title`, with `script` run on it.
    pub fn solid(title: &str, c: Rgb, script: &str) -> Self {
        Page::html(format!(
            "<!doctype html><html><head><title>{title}</title></head>\
             <body style=\"margin:0;height:100vh;background:{}\">\
             <script>{script}</script></body></html>",
            css(c)
        ))
    }

    pub fn header(mut self, line: &str) -> Self {
        self.headers.push(line.to_owned());
        self
    }

    pub fn delayed(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }
}

#[derive(Default)]
struct State {
    pages: HashMap<String, Page>,
    log: Vec<Request>,
}

/// The server; it runs until the test ends.
pub struct Web {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Web {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let state = shared.clone();
                thread::spawn(move || serve(conn, &state));
            }
        });
        Web { port, state }
    }

    /// Serves `page` at `path` (which starts with '/').
    pub fn page(&self, path: &str, page: Page) {
        self.state
            .lock()
            .unwrap()
            .pages
            .insert(path.to_owned(), page);
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().log.clone()
    }

    /// The requests so far for `path`.
    pub fn requests_for(&self, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }

    /// Waits for a request for `path`; returns the first.
    pub fn wait_for(&self, path: &str) -> Request {
        let mut got = None;
        wait_until(TIMEOUT, || {
            got = self.requests_for(path).into_iter().next();
            got.is_some()
        });
        got.unwrap_or_else(|| {
            panic!(
                "the browser never asked for {path}; it asked for {:?}",
                self.paths()
            )
        })
    }

    /// The paths asked for so far, leaving out the favicon.
    pub fn paths(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|r| r.path)
            .filter(|p| p != "/favicon.ico")
            .collect()
    }
}

fn serve(conn: TcpStream, state: &Mutex<State>) {
    let Ok(read_half) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut cookie = String::new();
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
            && name.eq_ignore_ascii_case("cookie")
        {
            cookie = value.trim().to_owned();
        }
    }
    let page = state.lock().unwrap().pages.get(&path).cloned();
    let (status, page) = match page {
        Some(page) => ("200 OK", page),
        None => ("404 Not Found", Page::html("not found")),
    };
    thread::sleep(page.delay);
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n",
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
    state.lock().unwrap().log.push(Request {
        path,
        cookie,
        answered: Instant::now(),
    });
}
