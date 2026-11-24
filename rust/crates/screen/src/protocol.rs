//! What the daemon and a screen process say to each other.
//!
//! The daemon starts the screen with the path of the Wayland socket to make,
//! then, if there is one, the path of the user's desktop's Wayland socket,
//! where the screen's live view opens; and pipes for its stdin and stdout:
//!
//! - The screen writes [`READY`] on a line once apps can connect to it.
//! - Then the daemon writes one [`Request`] per line, and reads one reply per
//!   request: a [`Reply`] line, followed for `png <n>` and `json <n>` by that
//!   many bytes.
//! - The screen exits when its stdin closes, so it never outlives the daemon.
//!
//! Pipes, not a socket: nothing but the daemon can reach the screen this way,
//! and there is no file to clean up.
//!
//! With a desktop, the screen also listens on [`USER_SOCKET`], next to its
//! Wayland socket, before it says [`READY`]. The daemon connects once, and
//! the screen then stops listening and removes the file, before any app can
//! start. On it the screen reports the user's input in the live view, one
//! [`UserInput`] line each, and the daemon writes [`ViewNotice`] lines: one
//! `forward` or `drop` per input, in order, and `taken` and `given`.

use ports::{UserInput, ViewNotice};

use std::time::Duration;

use ports::{Action, Button, Point};

/// The file name of the socket for the user's input, in the Wayland
/// socket's directory.
pub const USER_SOCKET: &str = "user";

/// A [`UserInput`] as a line.
pub fn input_line(input: UserInput) -> &'static str {
    match input {
        UserInput::Click => "click\n",
        UserInput::Key { escape: true } => "key escape\n",
        UserInput::Key { escape: false } => "key other\n",
        UserInput::Other => "other\n",
    }
}

/// A line as read, without its newline.
pub fn parse_input(line: &str) -> Option<UserInput> {
    match line {
        "click" => Some(UserInput::Click),
        "key escape" => Some(UserInput::Key { escape: true }),
        "key other" => Some(UserInput::Key { escape: false }),
        "other" => Some(UserInput::Other),
        _ => None,
    }
}

/// A [`ViewNotice`] as a line.
pub fn notice_line(notice: ViewNotice) -> &'static str {
    match notice {
        ViewNotice::Forward => "forward\n",
        ViewNotice::Drop => "drop\n",
        ViewNotice::TakenOver => "taken\n",
        ViewNotice::Given => "given\n",
    }
}

/// A line as read, without its newline.
pub fn parse_notice(line: &str) -> Option<ViewNotice> {
    match line {
        "forward" => Some(ViewNotice::Forward),
        "drop" => Some(ViewNotice::Drop),
        "taken" => Some(ViewNotice::TakenOver),
        "given" => Some(ViewNotice::Given),
        _ => None,
    }
}

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
    /// Move there and press the left button, or release it: a drag's ends,
    /// sent apart so the app can start a drag in between. Answered with
    /// [`Reply::Done`].
    Press(Point),
    Release(Point),
    /// The apps' windows, oldest first (one an app brought back to the front
    /// counts as new), as a JSON array of
    /// `{"app_id": .., "title": ..}`.
    Windows,
    /// Open the live view, a window with this title on the user's desktop.
    /// Answered with [`Reply::Done`]; opening it again changes nothing.
    View(String),
}

impl Request {
    pub fn line(&self) -> String {
        match self {
            Request::Screenshot => "screenshot\n".to_owned(),
            Request::Status => "status\n".to_owned(),
            Request::Act(Action::Move(to)) => format!("move {} {}\n", to.x, to.y),
            Request::Act(Action::Click { at, button, count }) => {
                format!("click {} {} {} {count}\n", at.x, at.y, button.name())
            }
            Request::Act(Action::Type(text)) => format!("type {}\n", escape(text)),
            Request::Act(Action::Key(combo)) => format!("key {}\n", escape(combo)),
            Request::Act(Action::Scroll { at, down, right }) => {
                format!("scroll {} {} {down} {right}\n", at.x, at.y)
            }
            Request::Act(Action::Drag { from, to }) => {
                format!("drag {} {} {} {}\n", from.x, from.y, to.x, to.y)
            }
            Request::Press(at) => format!("press {} {}\n", at.x, at.y),
            Request::Release(at) => format!("release {} {}\n", at.x, at.y),
            Request::Windows => "windows\n".to_owned(),
            Request::View(title) => format!("view {}\n", escape(title)),
        }
    }

    /// A line as read, without its newline.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match (word, rest) {
            ("screenshot", "") => Some(Request::Screenshot),
            ("status", "") => Some(Request::Status),
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
            ("press", _) => {
                let [x, y] = words(rest)?;
                Some(Request::Press(point(x, y)?))
            }
            ("release", _) => {
                let [x, y] = words(rest)?;
                Some(Request::Release(point(x, y)?))
            }
            ("drag", _) => {
                let [x1, y1, x2, y2] = words(rest)?;
                Some(Request::Act(Action::Drag {
                    from: point(x1, y1)?,
                    to: point(x2, y2)?,
                }))
            }
            ("windows", "") => Some(Request::Windows),
            ("view", _) => unescape(rest).map(Request::View),
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
    /// The process id of the app whose window is in front, if there is one.
    pub front: Option<u32>,
    /// How many windows were refused as password managers' or password
    /// prompts', and their apps ended.
    pub refused: u64,
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
    /// JSON of this many bytes follows.
    Json(usize),
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
                front,
                refused,
            }) => format!(
                "status {windows} {commits} {} {} {} {refused}\n",
                idle.as_millis(),
                u8::from(*unread),
                front.unwrap_or(0)
            ),
            Reply::Done(commits) => format!("done {commits}\n"),
            Reply::Json(len) => format!("json {len}\n"),
            Reply::Error(why) => format!("error {}\n", why.replace('\n', " ")),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "png" => rest.parse().ok().map(Reply::Png),
            "status" => {
                let [windows, commits, idle, unread, front, refused] = words(rest)?;
                Some(Reply::Status(Status {
                    windows: windows.parse().ok()?,
                    commits: commits.parse().ok()?,
                    idle: Duration::from_millis(idle.parse().ok()?),
                    unread: unread == "1",
                    front: Some(front.parse().ok()?).filter(|pid| *pid != 0),
                    refused: refused.parse().ok()?,
                }))
            }
            "done" => rest.parse().ok().map(Reply::Done),
            "json" => rest.parse().ok().map(Reply::Json),
            "error" => Some(Reply::Error(rest.to_owned())),
            _ => None,
        }
    }
}
