//! Agent sessions: one per connection to the daemon.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

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
}

impl fmt::Display for EndReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndReason::Disconnected => f.write_str("the agent disconnected"),
            EndReason::Failed(why) => write!(f, "the connection failed: {why}"),
            EndReason::DaemonStopping => f.write_str("the daemon is stopping"),
        }
    }
}

/// A session that has ended, as reported by [`Sessions::close`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub id: SessionId,
    pub reason: EndReason,
    pub lasted: Duration,
}

/// [`Sessions::close`] was given a session that isn't open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotOpen(pub SessionId);

impl fmt::Display for NotOpen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not open", self.0)
    }
}

impl std::error::Error for NotOpen {}

#[derive(Debug)]
struct Open {
    opened_at: Instant,
}

/// Every open session. Owned by exactly one task in the daemon; everyone
/// else sends that task messages.
#[derive(Debug, Default)]
pub struct Sessions {
    last_id: u64,
    open: BTreeMap<SessionId, Open>,
}

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a session at `now` and returns its new id.
    pub fn open(&mut self, now: Instant) -> SessionId {
        self.last_id += 1;
        let id = SessionId(self.last_id);
        self.open.insert(id, Open { opened_at: now });
        id
    }

    /// Ends an open session. Ending one that isn't open is an error, so a
    /// double close shows up instead of hiding.
    pub fn close(
        &mut self,
        id: SessionId,
        reason: EndReason,
        now: Instant,
    ) -> Result<Ended, NotOpen> {
        let open = self.open.remove(&id).ok_or(NotOpen(id))?;
        Ok(Ended {
            id,
            reason,
            lasted: now.saturating_duration_since(open.opened_at),
        })
    }

    pub fn is_open(&self, id: SessionId) -> bool {
        self.open.contains_key(&id)
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

    #[test]
    fn ids_start_at_one_and_count_up() {
        let mut s = Sessions::new();
        let now = t0();
        assert_eq!(s.open(now).get(), 1);
        assert_eq!(s.open(now).get(), 2);
        assert_eq!(s.open_count(), 2);
    }

    #[test]
    fn ids_are_never_reused_after_a_close() {
        let mut s = Sessions::new();
        let now = t0();
        let a = s.open(now);
        s.close(a, EndReason::Disconnected, now).unwrap();
        let b = s.open(now);
        assert_ne!(a, b);
        assert_eq!(b.get(), 2);
    }

    #[test]
    fn close_ends_exactly_that_session_and_reports_how_long_it_lasted() {
        let mut s = Sessions::new();
        let now = t0();
        let a = s.open(now);
        let b = s.open(now);
        let ended = s
            .close(
                a,
                EndReason::Disconnected,
                now + Duration::from_millis(1500),
            )
            .unwrap();
        assert_eq!(
            ended,
            Ended {
                id: a,
                reason: EndReason::Disconnected,
                lasted: Duration::from_millis(1500)
            }
        );
        assert!(!s.is_open(a));
        assert!(s.is_open(b));
        assert_eq!(s.open_count(), 1);
    }

    #[test]
    fn closing_twice_is_an_error() {
        let mut s = Sessions::new();
        let now = t0();
        let a = s.open(now);
        s.close(a, EndReason::Disconnected, now).unwrap();
        assert_eq!(s.close(a, EndReason::Disconnected, now), Err(NotOpen(a)));
    }

    #[test]
    fn a_clock_that_steps_back_gives_zero_not_a_panic() {
        let mut s = Sessions::new();
        let later = t0() + Duration::from_secs(5);
        let a = s.open(later);
        let ended = s
            .close(a, EndReason::DaemonStopping, later - Duration::from_secs(1))
            .unwrap();
        assert_eq!(ended.lasted, Duration::ZERO);
    }

    #[test]
    fn ids_and_reasons_read_well_in_logs() {
        let mut s = Sessions::new();
        let id = s.open(t0());
        assert_eq!(id.to_string(), "session 1");
        assert_eq!(
            EndReason::Disconnected.to_string(),
            "the agent disconnected"
        );
    }
}
