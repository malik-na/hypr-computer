//! What hypr-computer needs from the outside world, as traits.
//!
//! The daemon and the tools depend on these traits; `adapters` and `screen`
//! implement them for real, and tests implement them with fakes. Add a trait
//! here only when code in this workspace uses it.
#![forbid(unsafe_code)]

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::SessionId;

/// A boxed future, so async methods can sit behind `dyn` traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Tells the time. Session lengths and, later, timeouts read the time only
/// through this, so tests can control it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// An image encoded as PNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Png(pub Vec<u8>);

/// Why a screen couldn't do what was asked, in words for a log or an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenError(pub String);

impl fmt::Display for ScreenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScreenError {}

/// A program to start on a screen, and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppCommand {
    /// A name to look up on `PATH`, or a path.
    pub program: String,
    pub args: Vec<String>,
}

/// What became of an app started on a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launched {
    /// It opened a window, and the window is on the screen now.
    Window { pid: u32 },
    /// It is running, but opened no window in the time it was given.
    NoWindow { pid: u32, waited: Duration },
    /// It ended, and no window opened: it `failed`, or it ran and was done,
    /// like a launcher whose app opened nothing. `output` is the end of what
    /// it printed, which usually says why.
    Exited {
        status: String,
        failed: bool,
        output: String,
    },
}

/// A point on a screen, in pixels from its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::Left, Button::Middle, Button::Right];

    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
        }
    }

    pub fn from_name(name: &str) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// What an agent does with its screen's own pointer and keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Moves the pointer to the point.
    Move(Point),
    /// Moves the pointer to `at`, then clicks `button` `count` times: two is
    /// a double click.
    Click {
        at: Point,
        button: Button,
        count: u32,
    },
    /// Types the text as key presses: `\n` is Enter, `\t` is Tab.
    Type(String),
    /// Presses a key combination, like `ctrl+shift+t`, and lets go of it.
    Key(String),
    /// Moves the pointer to `at`, then turns the wheel `down` notches down
    /// and `right` notches right; negative is up or left.
    Scroll { at: Point, down: i32, right: i32 },
    /// Presses the left button at `from`, moves to `to` and releases it.
    Drag { from: Point, to: Point },
}

/// A web page for the screen's browser to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebPage {
    /// An `http` or `https` URL with a host.
    pub url: String,
}

/// A page the screen's browser opened, as it ended up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedPage {
    /// Where it ended up, after any redirects.
    pub url: String,
    pub title: String,
    /// Whether its `load` event fired; `false` if it was still loading when
    /// the browser stopped waiting.
    pub loaded: bool,
}

/// An agent's screen, as its tools see it.
pub trait Screen: Send + Sync {
    /// The whole screen, as it is now.
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>>;
    /// Does `action` with the screen's pointer or keyboard, which nothing
    /// else drives, and returns once the app has drawn what it did in
    /// response, so the next screenshot shows it.
    fn act(&self, action: Action) -> BoxFuture<'_, Result<(), ScreenError>>;
    /// Starts an app on this screen and on no other, and waits a while for
    /// its window. The app ends with the screen, and so does all it starts.
    fn launch(&self, app: AppCommand) -> BoxFuture<'_, Result<Launched, ScreenError>>;
    /// Opens `page` in this screen's browser, starting the browser with a
    /// fresh profile the first time, and returns once the page has loaded
    /// and is in front, so the next screenshot shows it. The browser ends
    /// with the screen, and its profile is deleted. A screen without a
    /// browser says so.
    fn open_page(&self, page: WebPage) -> BoxFuture<'_, Result<OpenedPage, ScreenError>> {
        let _ = page;
        Box::pin(async { Err(ScreenError("this screen has no browser".to_owned())) })
    }
    /// The guard is done with this screen for the tool call it was given
    /// for: `Some(why)` if the call was stopped on the way, and the agent
    /// gets `why` instead of the tool's result.
    fn end_call(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }
}

/// Asked before each input a tool call gives its screen: how a stop reaches
/// an input in progress (`domain::stop`).
pub trait InputGate: Send + Sync {
    /// Whether the next input may be given; `holds` if it leaves a button or
    /// key held down. `false`: give no more input, and release what is held.
    fn ask(&self, holds: bool) -> BoxFuture<'_, bool>;
    /// Everything the call held down is released.
    fn released(&self) -> BoxFuture<'_, ()>;
}

/// An agent's screen, as the daemon runs it: the steps of
/// `domain::lifecycle` that touch the world.
pub trait ScreenProcess: Screen {
    /// The process id, for the log.
    fn pid(&self) -> u32;
    /// Resolves once the screen says it is up (`Event::ScreenStarted`), or
    /// fails if it doesn't in time.
    fn started(&self) -> BoxFuture<'_, Result<(), ScreenError>>;
    /// The readiness check (`Effect::VerifyScreen`): passes only if the
    /// screen really serves what the agent will ask of it.
    fn verify(&self) -> BoxFuture<'_, Result<(), ScreenError>>;
    /// Opens the screen's live view, a window called `title` on the user's
    /// desktop. It closes when the screen does.
    fn open_view(&self, title: String) -> BoxFuture<'_, Result<(), ScreenError>>;
    /// Resolves when the process has exited, for whatever reason.
    fn exited(&self) -> BoxFuture<'_, ()>;
    /// `Screen::act`, asking `gate` before each input. Refused, it releases
    /// what the action holds down and fails.
    fn act_gated<'a>(
        &'a self,
        action: Action,
        gate: &'a dyn InputGate,
    ) -> BoxFuture<'a, Result<(), ScreenError>>;
    /// `Effect::Teardown`: kills the screen and everything it started, waits
    /// until they are gone, and removes the files they leave. Needs nothing
    /// from the screen itself.
    fn teardown(&self) -> BoxFuture<'_, ()>;
}

/// Why the user's desktop couldn't do what was asked, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopError(pub String);

impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DesktopError {}

/// The user's desktop, as the live views need it: the one place that knows
/// which desktop it is. Hyprland first; other desktops implement this later.
pub trait Desktop: Send + Sync {
    /// Makes every window with app id `domain::view::APP_ID` open on the
    /// views' workspace without taking the user there or taking their
    /// focus. Asked before every view opens, so it must be safe to repeat.
    fn place_views(&self) -> BoxFuture<'_, Result<(), DesktopError>>;
    /// Takes the user to the views. Returns where they are, in words for
    /// the user, such as "workspace 10".
    fn show_views(&self) -> BoxFuture<'_, Result<String, DesktopError>>;
}

/// Starts screens: one per session.
pub trait Screens: Send + Sync {
    /// `Effect::SpawnScreen`. Returns as soon as the process exists, so a
    /// teardown can stop it while it is still starting.
    fn spawn(&self, session: SessionId) -> Result<Arc<dyn ScreenProcess>, ScreenError>;
}
