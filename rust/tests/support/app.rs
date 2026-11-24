//! `hc-test-app` (in `rust/tests/app/`), the tiny Wayland app the tests put
//! on a screen, and what it wrote to its log.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Env, TIMEOUT};

pub fn program() -> PathBuf {
    super::bin("hc-test-app")
}

/// `RRGGBB`, as the app takes colours.
pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("{r:02x}{g:02x}{b:02x}")
}

/// The app's arguments: paint `top` over `bottom`, and log to `log`.
pub fn args(top: [u8; 3], bottom: [u8; 3], log: &Log) -> Vec<String> {
    vec![
        "--top".to_owned(),
        hex(top),
        "--bottom".to_owned(),
        hex(bottom),
        "--log".to_owned(),
        log.path.display().to_string(),
    ]
}

/// What an app wrote to its log: one JSON object per line.
pub struct Log {
    path: PathBuf,
}

impl Log {
    /// A log file of its own in `env`.
    pub fn new(env: &Env) -> Log {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Log {
            path: env.path(&format!("app-{n}.log")),
        }
    }

    /// Every event logged so far; a line still being written is left out.
    pub fn events(&self) -> Vec<Value> {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        let whole = &text[..text.rfind('\n').map_or(0, |end| end + 1)];
        whole
            .lines()
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|e| {
                    panic!("the app logged a line that isn't JSON ({e}): {line}")
                })
            })
            .collect()
    }

    /// The first event named `name`, waiting for it.
    pub fn wait_for(&self, name: &str) -> Value {
        let end = Instant::now() + TIMEOUT;
        loop {
            if let Some(event) = self.events().into_iter().find(|e| e["event"] == name) {
                return event;
            }
            assert!(
                Instant::now() < end,
                "the app never logged {name:?}; it logged:\n{:#?}",
                self.events()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The app, started straight on a screen; it stays there until dropped.
pub struct App {
    child: Child,
    pub log: Log,
}

impl App {
    /// Opens the app on session `n`'s screen, painted `top` over `bottom`, and
    /// returns once the screen has the picture.
    pub fn paint(env: &Env, n: u64, top: [u8; 3], bottom: [u8; 3]) -> App {
        let log = Log::new(env);
        let child = env
            .command(&program())
            .args(args(top, bottom, &log))
            .env("WAYLAND_DISPLAY", env.screen_socket(n))
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let app = App { child, log };
        app.log.wait_for("ready");
        app
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
