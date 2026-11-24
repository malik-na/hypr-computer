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

/// The screen's first line.
pub const READY: &str = "ready";

/// A request from the daemon, one per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The whole screen, as a PNG.
    Screenshot,
    /// The screen's [`Status`].
    Status,
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
            _ => None,
        }
    }
}

/// What has happened on a screen so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// How many app windows have drawn their first picture.
    pub windows: u64,
}

/// The line that starts a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// A PNG of this many bytes follows.
    Png(usize),
    Status(Status),
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
            Reply::Status(Status { windows }) => format!("status {windows}\n"),
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
            "status" => Some(Reply::Status(Status {
                windows: rest.parse().ok()?,
            })),
            "launched" => rest.parse().ok().map(Reply::Launched),
            "running" => Some(Reply::Running),
            "ended" => Some(Reply::Ended(rest.to_owned())),
            "error" => Some(Reply::Error(rest.to_owned())),
            _ => None,
        }
    }
}
