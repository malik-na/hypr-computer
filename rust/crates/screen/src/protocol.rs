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
//! The screen starts the agent's apps itself, so they and all they start are
//! below it (see `compositor/apps.rs`). An app's output goes to
//! [`app_log`]`(n)` in the directory of the Wayland socket.
//!
//! Pipes, not a socket: nothing but the daemon can reach the screen this way,
//! and there is no file to clean up.

use std::time::Duration;

use ports::{Action, Button, Point};

/// The screen's first line.
pub const READY: &str = "ready";

/// A request from the daemon, one per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The whole screen, as a PNG.
    Screenshot,
    /// The screen's [`Status`].
    Status,
    /// Do this with the pointer or keyboard; answered with [`Reply::Done`].
    Act(Action),
    /// Start a program with these arguments, its output going to
    /// [`app_log`]`(n)`. Answered with its pid.
    Launch {
        n: u64,
        program: String,
        args: Vec<String>,
    },
    /// Whether the app with this pid, started by `Launch`, has ended.
    App(u32),
}

/// The file an app's output goes to, in the screen's directory.
pub fn app_log(n: u64) -> String {
    format!("app-{n}.log")
}

impl Request {
    pub fn line(&self) -> String {
        match self {
            Request::Screenshot => "screenshot\n".to_owned(),
            Request::Status => "status\n".to_owned(),
            Request::Launch { n, program, args } => {
                // JSON keeps each argument whole, spaces and newlines too.
                let argv = serde_json::to_string(&(program, args)).unwrap_or_default();
                format!("launch {n} {argv}\n")
            }
            Request::App(pid) => format!("app {pid}\n"),
            Request::Act(Action::Move(to)) => format!("move {} {}\n", to.x, to.y),
            Request::Act(Action::Click { at, button, count }) => {
                format!("click {} {} {} {count}\n", at.x, at.y, button.name())
            }
            Request::Act(Action::Type(text)) => format!("type {}\n", escape(text)),
            Request::Act(Action::Key(combo)) => format!("key {}\n", escape(combo)),
            Request::Act(Action::Scroll { at, down, right }) => {
                format!("scroll {} {} {down} {right}\n", at.x, at.y)
            }
        }
    }

    /// A line as read, without its newline.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match (word, rest) {
            ("screenshot", "") => Some(Request::Screenshot),
            ("status", "") => Some(Request::Status),
            ("launch", rest) => {
                let (n, argv) = rest.split_once(' ')?;
                let (program, args) = serde_json::from_str(argv).ok()?;
                Some(Request::Launch {
                    n: n.parse().ok()?,
                    program,
                    args,
                })
            }
            ("app", pid) => pid.parse().ok().map(Request::App),
            ("move", _) => {
                let [x, y] = words(rest)?;
                Some(Request::Act(Action::Move(point(x, y)?)))
            }
            ("click", _) => {
                let [x, y, button, count] = words(rest)?;
                Some(Request::Act(Action::Click {
                    at: point(x, y)?,
                    button: Button::from_name(button)?,
                    count: count.parse().ok()?,
                }))
            }
            ("type", _) => unescape(rest).map(|text| Request::Act(Action::Type(text))),
            ("key", _) => unescape(rest).map(|combo| Request::Act(Action::Key(combo))),
            ("scroll", _) => {
                let [x, y, down, right] = words(rest)?;
                Some(Request::Act(Action::Scroll {
                    at: point(x, y)?,
                    down: down.parse().ok()?,
                    right: right.parse().ok()?,
                }))
            }
            _ => None,
        }
    }
}

/// `text` on one line: backslash, newline and carriage return escaped.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape(line: &str) -> Option<String> {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        text.push(match c {
            '\\' => match chars.next()? {
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                _ => return None,
            },
            c => c,
        });
    }
    Some(text)
}

/// Exactly `N` words.
fn words<const N: usize>(text: &str) -> Option<[&str; N]> {
    text.split(' ').collect::<Vec<_>>().try_into().ok()
}

fn point(x: &str, y: &str) -> Option<Point> {
    Some(Point {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
    })
}

/// What has happened on a screen so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// How many app windows have drawn their first picture.
    pub windows: u64,
    /// How many times apps have drawn (committed a surface).
    pub commits: u64,
    /// How long since an app last drew.
    pub idle: Duration,
    /// Whether the app on the screen may not have read all the input yet.
    pub unread: bool,
}

/// The line that starts a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// A PNG of this many bytes follows.
    Png(usize),
    Status(Status),
    /// The action is done: the app has the input. The number is
    /// [`Status::commits`] at that moment, to tell when the app has drawn its
    /// response.
    Done(u64),
    /// The app started, with this pid.
    Launched(u32),
    /// The app hasn't ended.
    Running,
    /// The app ended, with this exit status.
    Ended(String),
    /// The request failed, and why. Nothing follows.
    Error(String),
}

impl Reply {
    pub fn line(&self) -> String {
        match self {
            Reply::Png(len) => format!("png {len}\n"),
            Reply::Status(Status {
                windows,
                commits,
                idle,
                unread,
            }) => format!(
                "status {windows} {commits} {} {}\n",
                idle.as_millis(),
                u8::from(*unread)
            ),
            Reply::Done(commits) => format!("done {commits}\n"),
            Reply::Launched(pid) => format!("launched {pid}\n"),
            Reply::Running => "running\n".to_owned(),
            Reply::Ended(status) => format!("ended {}\n", status.replace('\n', " ")),
            Reply::Error(why) => format!("error {}\n", why.replace('\n', " ")),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "png" => rest.parse().ok().map(Reply::Png),
            "status" => {
                let [windows, commits, idle, unread] = words(rest)?;
                Some(Reply::Status(Status {
                    windows: windows.parse().ok()?,
                    commits: commits.parse().ok()?,
                    idle: Duration::from_millis(idle.parse().ok()?),
                    unread: unread == "1",
                }))
            }
            "done" => rest.parse().ok().map(Reply::Done),
            "launched" => rest.parse().ok().map(Reply::Launched),
            "running" => Some(Reply::Running),
            "ended" => Some(Reply::Ended(rest.to_owned())),
            "error" => Some(Reply::Error(rest.to_owned())),
            _ => None,
        }
    }
}
