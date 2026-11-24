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
use std::time::Instant;

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

/// An agent's screen, as its tools see it.
pub trait Screen: Send + Sync {
    /// The whole screen, as it is now.
    fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>>;
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
    /// Resolves when the process has exited, for whatever reason.
    fn exited(&self) -> BoxFuture<'_, ()>;
    /// `Effect::Teardown`: kills the screen and everything it started, waits
    /// until they are gone, and removes the files they leave. Needs nothing
    /// from the screen itself.
    fn teardown(&self) -> BoxFuture<'_, ()>;
}

/// Starts screens: one per session.
pub trait Screens: Send + Sync {
    /// `Effect::SpawnScreen`. Returns as soon as the process exists, so a
    /// teardown can stop it while it is still starting.
    fn spawn(&self, session: SessionId) -> Result<Arc<dyn ScreenProcess>, ScreenError>;
}
