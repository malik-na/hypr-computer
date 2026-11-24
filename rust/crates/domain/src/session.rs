//! Agent sessions: one per connection to the daemon, each moving through
//! [`lifecycle`](crate::lifecycle) until its teardown is done.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use crate::lifecycle::{self, Effect, Event, SessionState};

/// Names one agent session. Ids start at 1 and are never reused while the
/// daemon runs, so a log line always points at exactly one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session {}", self.0)
    }
}

/// Why a session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The agent's side of the connection closed, however the agent died.
    Disconnected,
    /// The connection broke before or during the MCP handshake.
    Failed(String),
    /// The daemon is stopping.
    DaemonStopping,
    /// The screen process exited on its own.
    ScreenExited,
    /// The screen didn't start, or didn't pass its readiness check.
    ScreenFailed(String),
}

impl fmt::Display for EndReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndReason::Disconnected => f.write_str("the agent disconnected"),
            EndReason::Failed(why) => write!(f, "the connection failed: {why}"),
            EndReason::DaemonStopping => f.write_str("the daemon is stopping"),
            EndReason::ScreenExited => f.write_str("its screen exited"),
            EndReason::ScreenFailed(why) => write!(f, "its screen failed: {why}"),
        }
    }
}

/// A session whose teardown is done, as reported by [`Sessions::step`].
#[derive(Debug, PartialEq, Eq)]
pub struct Gone<T> {
    pub id: SessionId,
    pub lasted: Duration,
    /// What the caller kept for the session.
    pub data: T,
}

/// What [`Sessions::step`] did with an event.
#[derive(Debug, PartialEq, Eq)]
pub struct Stepped<T> {
    /// What the caller must do now.
    pub effect: Option<Effect>,
    /// Set when the session has just gone: it has been dropped.
    pub gone: Option<Gone<T>>,
}

#[derive(Debug)]
struct Open<T> {
    opened_at: Instant,
    state: SessionState,
    data: T,
}

/// Every session that isn't gone, each with its lifecycle state and the
/// caller's own data `T` for it. Owned by exactly one task in the daemon;
/// everyone else sends that task messages.
///
/// Keeps the promises `lifecycle` asks of its caller: one state per session
/// id; ids are never reused; a session that reaches `Absent` again is
/// dropped; an event for a session that is gone is ignored.
#[derive(Debug)]
pub struct Sessions<T> {
    last_id: u64,
    open: BTreeMap<SessionId, Open<T>>,
}

impl<T> Default for Sessions<T> {
    fn default() -> Self {
        Self {
            last_id: 0,
            open: BTreeMap::new(),
        }
    }
}

impl<T> Sessions<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// An agent connected at `now`: a new session, and what to do first.
    pub fn connect(&mut self, now: Instant, data: T) -> (SessionId, Option<Effect>) {
        self.last_id += 1;
        let id = SessionId(self.last_id);
        let (state, effect) = lifecycle::step(SessionState::Absent, Event::Connect);
        self.open.insert(
            id,
            Open {
                opened_at: now,
                state,
                data,
            },
        );
        (id, effect)
    }

    /// Applies `event` to session `id`. `None` if that session is gone: the
    /// event is ignored.
    pub fn step(&mut self, id: SessionId, event: Event, now: Instant) -> Option<Stepped<T>> {
        let open = self.open.get_mut(&id)?;
        let (state, effect) = lifecycle::step(open.state, event);
        open.state = state;
        let gone = if state == SessionState::Absent {
            self.open.remove(&id).map(|open| Gone {
                id,
                lasted: now.saturating_duration_since(open.opened_at),
                data: open.data,
            })
        } else {
            None
        };
        Some(Stepped { effect, gone })
    }

    pub fn state(&self, id: SessionId) -> Option<SessionState> {
        self.open.get(&id).map(|open| open.state)
    }

    pub fn data_mut(&mut self, id: SessionId) -> Option<&mut T> {
        self.open.get_mut(&id).map(|open| &mut open.data)
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    /// Takes session `id` from connected to gone, the way an agent leaving does.
    fn disconnect(
        s: &mut Sessions<&'static str>,
        id: SessionId,
        now: Instant,
    ) -> Gone<&'static str> {
        s.step(id, Event::Disconnect, now).unwrap();
        s.step(id, Event::TeardownDone, now).unwrap().gone.unwrap()
    }

    #[test]
    fn ids_start_at_one_and_count_up() {
        let mut s = Sessions::new();
        let now = t0();
        assert_eq!(s.connect(now, ()).0.get(), 1);
        assert_eq!(s.connect(now, ()).0.get(), 2);
        assert_eq!(s.open_count(), 2);
    }

    #[test]
    fn a_new_session_starts_by_spawning_its_screen() {
        let mut s = Sessions::new();
        let (id, effect) = s.connect(t0(), ());
        assert_eq!(effect, Some(Effect::SpawnScreen));
        assert_eq!(s.state(id), Some(SessionState::Starting));
    }

    #[test]
    fn ids_are_never_reused_after_a_session_is_gone() {
        let mut s = Sessions::new();
        let now = t0();
        let (a, _) = s.connect(now, "a");
        disconnect(&mut s, a, now);
        let (b, _) = s.connect(now, "b");
        assert_ne!(a, b);
        assert_eq!(b.get(), 2);
    }

    #[test]
    fn a_session_is_dropped_once_its_teardown_is_done_and_says_how_long_it_lasted() {
        let mut s = Sessions::new();
        let now = t0();
        let (a, _) = s.connect(now, "a");
        let (b, _) = s.connect(now, "b");

        let stepped = s.step(a, Event::Disconnect, now).unwrap();
        assert_eq!(stepped.effect, Some(Effect::Teardown));
        assert_eq!(stepped.gone, None, "gone only once the teardown is done");
        assert_eq!(s.open_count(), 2);

        let stepped = s
            .step(a, Event::TeardownDone, now + Duration::from_millis(1500))
            .unwrap();
        assert_eq!(stepped.effect, Some(Effect::CloseConnection));
        assert_eq!(
            stepped.gone,
            Some(Gone {
                id: a,
                lasted: Duration::from_millis(1500),
                data: "a"
            })
        );
        assert_eq!(s.state(a), None);
        assert_eq!(s.state(b), Some(SessionState::Starting));
        assert_eq!(s.open_count(), 1);
    }

    #[test]
    fn events_for_a_session_that_is_gone_are_ignored() {
        let mut s = Sessions::new();
        let now = t0();
        let (a, _) = s.connect(now, "a");
        disconnect(&mut s, a, now);
        for event in [
            Event::Connect,
            Event::ScreenCrashed,
            Event::ToolCall,
            Event::TeardownDone,
        ] {
            assert_eq!(s.step(a, event, now), None, "{event:?}");
        }
        assert_eq!(s.open_count(), 0);
    }

    #[test]
    fn a_clock_that_steps_back_gives_zero_not_a_panic() {
        let mut s = Sessions::new();
        let later = t0() + Duration::from_secs(5);
        let (a, _) = s.connect(later, "a");
        let gone = disconnect(&mut s, a, later - Duration::from_secs(1));
        assert_eq!(gone.lasted, Duration::ZERO);
    }

    #[test]
    fn ids_and_reasons_read_well_in_logs() {
        let mut s = Sessions::new();
        let (id, _) = s.connect(t0(), ());
        assert_eq!(id.to_string(), "session 1");
        assert_eq!(
            EndReason::Disconnected.to_string(),
            "the agent disconnected"
        );
        assert_eq!(EndReason::ScreenExited.to_string(), "its screen exited");
    }
}
