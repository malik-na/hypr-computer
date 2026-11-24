//! What the daemon and a screen process say to each other.
//!
//! The daemon starts the screen with the path of the Wayland socket to make
//! as its only argument, and pipes for its stdin and stdout:
//!
//! - The screen writes [`READY`] on a line once apps can connect to it.
//! - Then the daemon writes one [`Request`] per line, and reads one reply per
//!   request: a [`Reply`] line, followed for `png <n>` by the image's `n` bytes.
//! - The screen exits when its stdin closes, so it never outlives the daemon.
//!
//! Pipes, not a socket: nothing but the daemon can reach the screen this way,
//! and there is no file to clean up.

/// The screen's first line.
pub const READY: &str = "ready";

/// A request from the daemon, one per line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// The whole screen, as a PNG.
    Screenshot,
}

impl Request {
    pub fn line(self) -> &'static str {
        match self {
            Request::Screenshot => "screenshot\n",
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        match line.trim_end() {
            "screenshot" => Some(Request::Screenshot),
            _ => None,
        }
    }
}

/// The line that starts a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// A PNG of this many bytes follows.
    Png(usize),
    /// The request failed, and why. Nothing follows.
    Error(String),
}

impl Reply {
    pub fn line(&self) -> String {
        match self {
            Reply::Png(len) => format!("png {len}\n"),
            Reply::Error(why) => format!("error {}\n", why.replace('\n', " ")),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end();
        if let Some(len) = line.strip_prefix("png ") {
            return len.parse().ok().map(Reply::Png);
        }
        line.strip_prefix("error ")
            .map(|why| Reply::Error(why.to_owned()))
    }
}
