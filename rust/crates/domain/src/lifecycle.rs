//! One agent session's lifecycle, as specified in specs/SessionLifecycle.tla.
//!
//! `step` is the spec's `session` variable moving through its actions. It decides only what the daemon
//! believes and what it must do next; the world (the screen process, the apps, the connection) is the
//! adapters' business, reported back as events.
//!
//! What the caller must do, because `step` can't check it:
//! - **One state per session id, never reused.** A new connection is a new session id starting at `Absent`; a
//!   session that reaches `Absent` again is dropped, not kept for the next one. Events for a session id that is
//!   gone are ignored, so a late event from an old screen can't reach a new session.
//! - **`Effect::Teardown` runs until the screen process and its apps are seen gone**, then reports
//!   `Event::TeardownDone`. It kills them itself (every process below the screen, in any process group or
//!   session, and what a screen that died first left behind), so it needs nothing from the agent or the screen;
//!   the screen's process id is known from `Effect::SpawnScreen` on. A screen that finishes starting during a
//!   teardown is stopped by that same teardown.
//! - **`Effect::CloseConnection` must be harmless on a connection that is already closed:** when the agent
//!   disconnected, it is.

/// What the daemon believes about one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Absent,
    Starting,
    Verifying,
    Ready,
    TearingDown,
}

/// Something the daemon saw happen to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Connect,
    ScreenStarted,
    ScreenReady,
    ScreenFailed,
    ScreenCrashed,
    ToolCall,
    Disconnect,
    TeardownDone,
}

/// What the daemon must do because of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    SpawnScreen,
    VerifyScreen,
    ServeToolCall,
    RejectToolCall,
    Teardown,
    CloseConnection,
}

/// The session's next state, and what to do about it.
pub fn step(state: SessionState, event: Event) -> (SessionState, Option<Effect>) {
    match (state, event) {
        (SessionState::Absent, Event::Connect) => {
            (SessionState::Starting, Some(Effect::SpawnScreen))
        }
        (SessionState::Starting, Event::ScreenStarted) => {
            (SessionState::Verifying, Some(Effect::VerifyScreen))
        }
        (SessionState::Verifying, Event::ScreenReady) => (SessionState::Ready, None),
        (SessionState::Ready, Event::ToolCall) => {
            (SessionState::Ready, Some(Effect::ServeToolCall))
        }
        (
            SessionState::Starting | SessionState::Verifying | SessionState::Ready,
            Event::Disconnect,
        ) => (SessionState::TearingDown, Some(Effect::Teardown)),
        (SessionState::Starting | SessionState::Verifying, Event::ScreenFailed)
        | (
            SessionState::Starting | SessionState::Verifying | SessionState::Ready,
            Event::ScreenCrashed,
        ) => (SessionState::TearingDown, Some(Effect::Teardown)),
        (SessionState::TearingDown, Event::TeardownDone) => {
            (SessionState::Absent, Some(Effect::CloseConnection))
        }
        (state, Event::ToolCall) => (state, Some(Effect::RejectToolCall)),
        (state, _) => (state, None),
    }
}
