//! A fake Hyprland: its IPC socket, which records every request and answers
//! `ok`, or whatever a test tells it to.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::TIMEOUT;

/// What the fake Hyprland's instance is called.
pub const SIGNATURE: &str = "fake-hyprland";

/// What Hyprland is told before each view opens: put it on workspace 10
/// without switching there, and never give it the user's focus.
pub const PLACE_VIEWS: &str = "/eval hl.window_rule({name = \"hypr-computer-live-views\", \
    match = {class = \"^hypr-computer-view$\"}, workspace = \"10 silent\", \
    no_initial_focus = true, focus_on_activate = false, enabled = true})";

/// What Hyprland is told to take the user to the views.
pub const SHOW_VIEWS: &str = "/dispatch hl.dsp.focus({workspace = 10})";

pub struct Hyprland {
    requests: Arc<Mutex<Vec<String>>>,
    /// What it answers, and whether it holds its answers back for now.
    answer: Arc<(Mutex<(String, bool)>, Condvar)>,
}

impl Hyprland {
    /// A Hyprland whose Wayland socket would be in `dir`: its IPC socket is
    /// next to it, as a real one's is.
    pub fn start(dir: &Path) -> Hyprland {
        let ipc = dir.join("hypr").join(SIGNATURE);
        std::fs::create_dir_all(&ipc).unwrap();
        let listener = UnixListener::bind(ipc.join(".socket.sock")).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new((Mutex::new(("ok".to_owned(), false)), Condvar::new()));
        let (recorded, answering) = (requests.clone(), answer.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = String::new();
                if stream.read_to_string(&mut request).is_err() {
                    continue;
                }
                recorded.lock().unwrap().push(request);
                let (lock, released) = &*answering;
                let answer = released.wait_while(lock.lock().unwrap(), |a| a.1).unwrap();
                let _ = stream.write_all(answer.0.as_bytes());
            }
        });
        Hyprland { requests, answer }
    }

    /// Makes it take requests without answering them, until [`Hyprland::release`].
    pub fn hold(&self) {
        self.answer.0.lock().unwrap().1 = true;
    }

    pub fn release(&self) {
        self.answer.0.lock().unwrap().1 = false;
        self.answer.1.notify_all();
    }

    /// Makes it answer `text` from now on.
    pub fn answer_with(&self, text: &str) {
        self.answer.0.lock().unwrap().0 = text.to_owned();
    }

    /// Every request it got, oldest first.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    // The daemon's notice fallback (#29) also talks to Hyprland, with `/notify` requests.
    /// Every request it got except the `/notify` ones, oldest first.
    pub fn view_requests(&self) -> Vec<String> {
        let mut requests = self.requests();
        requests.retain(|request| !request.starts_with("/notify "));
        requests
    }

    /// Waits until it has had `n` requests other than `/notify` ones, and returns them all.
    pub fn wait_for_view_requests(&self, n: usize) -> Vec<String> {
        let end = Instant::now() + TIMEOUT;
        loop {
            let requests = self.view_requests();
            if requests.len() >= n {
                return requests;
            }
            assert!(
                Instant::now() < end,
                "wanted {n} view requests to Hyprland, got {requests:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits until it has had `n` requests, and returns them all.
    pub fn wait_for_requests(&self, n: usize) -> Vec<String> {
        let end = Instant::now() + TIMEOUT;
        loop {
            let requests = self.requests();
            if requests.len() >= n {
                return requests;
            }
            assert!(
                Instant::now() < end,
                "wanted {n} requests to Hyprland, got {requests:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
