//! The one task that owns the sessions. Connections ask it for a session and
//! hand theirs back through a [`Ticket`] that ends the session when dropped,
//! so a session ends however its connection ends: closed, broken, or a panic.

use std::sync::Arc;

use domain::{EndReason, Ended, SessionId, Sessions};
use ports::Clock;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug)]
enum Msg {
    Open(oneshot::Sender<SessionId>),
    Close(SessionId, EndReason),
}

/// What happened, for the log.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    Started { id: SessionId, open: usize },
    Ended { ended: Ended, open: usize },
    DoubleClose(SessionId),
}

/// A handle to the owner. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Owner {
    tx: mpsc::UnboundedSender<Msg>,
}

impl Owner {
    /// Starts the owner. It stops once every `Owner` and `Ticket` is gone.
    pub fn spawn(clock: Arc<dyn Clock>) -> (Owner, JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Owner { tx }, tokio::spawn(run(rx, clock)))
    }

    /// Opens a session; `None` only if the owner has stopped.
    pub async fn open(&self) -> Option<Ticket> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(Msg::Open(reply)).ok()?;
        let id = answer.await.ok()?;
        Some(Ticket {
            id,
            reason: EndReason::Disconnected,
            tx: self.tx.clone(),
        })
    }
}

/// One open session. Dropping it ends the session.
#[derive(Debug)]
pub struct Ticket {
    id: SessionId,
    reason: EndReason,
    tx: mpsc::UnboundedSender<Msg>,
}

impl Ticket {
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Ends the session, saying why.
    pub fn end(mut self, reason: EndReason) {
        self.reason = reason;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let reason = std::mem::replace(&mut self.reason, EndReason::Disconnected);
        // Fails only if the owner is gone, and then there is nothing to end.
        let _ = self.tx.send(Msg::Close(self.id, reason));
    }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Msg>, clock: Arc<dyn Clock>) {
    let mut sessions = Sessions::new();
    while let Some(msg) = rx.recv().await {
        match handle(&mut sessions, clock.as_ref(), msg) {
            Some(Event::Started { id, open }) => info!("{id} started; {open} open"),
            Some(Event::Ended { ended, open }) => info!(
                "{} ended ({}) after {:.2}s; {open} open",
                ended.id,
                ended.reason,
                ended.lasted.as_secs_f64()
            ),
            Some(Event::DoubleClose(id)) => warn!("{id} was closed twice"),
            None => {}
        }
    }
}

fn handle(sessions: &mut Sessions, clock: &dyn Clock, msg: Msg) -> Option<Event> {
    match msg {
        Msg::Open(reply) => {
            let id = sessions.open(clock.now());
            if reply.send(id).is_err() {
                // The connection went away while waiting: end it at once.
                let ended = sessions.close(id, EndReason::Disconnected, clock.now());
                return ended.ok().map(|ended| Event::Ended {
                    ended,
                    open: sessions.open_count(),
                });
            }
            Some(Event::Started {
                id,
                open: sessions.open_count(),
            })
        }
        Msg::Close(id, reason) => Some(match sessions.close(id, reason, clock.now()) {
            Ok(ended) => Event::Ended {
                ended,
                open: sessions.open_count(),
            },
            Err(_) => Event::DoubleClose(id),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct FakeClock(Mutex<Instant>);

    impl FakeClock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }
    }

    fn open(sessions: &mut Sessions, clock: &FakeClock) -> (Option<Event>, SessionId) {
        let (tx, mut rx) = oneshot::channel();
        let event = handle(sessions, clock, Msg::Open(tx));
        (event, rx.try_recv().unwrap())
    }

    #[test]
    fn open_then_close_reports_the_session_and_how_long_it_lasted() {
        let clock = FakeClock(Mutex::new(Instant::now()));
        let mut sessions = Sessions::new();
        let (started, id) = open(&mut sessions, &clock);
        assert_eq!(started, Some(Event::Started { id, open: 1 }));

        clock.advance(Duration::from_secs(3));
        let ended = handle(
            &mut sessions,
            &clock,
            Msg::Close(id, EndReason::Disconnected),
        );
        assert_eq!(
            ended,
            Some(Event::Ended {
                ended: Ended {
                    id,
                    reason: EndReason::Disconnected,
                    lasted: Duration::from_secs(3)
                },
                open: 0
            })
        );
    }

    #[test]
    fn a_session_whose_connection_left_while_opening_is_ended_at_once() {
        let clock = FakeClock(Mutex::new(Instant::now()));
        let mut sessions = Sessions::new();
        let (tx, rx) = oneshot::channel();
        drop(rx);
        let event = handle(&mut sessions, &clock, Msg::Open(tx));
        assert!(
            matches!(event, Some(Event::Ended { open: 0, .. })),
            "{event:?}"
        );
        assert_eq!(sessions.open_count(), 0);
    }

    #[tokio::test]
    async fn dropping_a_ticket_ends_its_session_with_the_reason_given() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ticket = Ticket {
            id: Sessions::new().open(Instant::now()),
            reason: EndReason::Disconnected,
            tx,
        };
        let id = ticket.id();
        ticket.end(EndReason::DaemonStopping);
        match rx.recv().await {
            Some(Msg::Close(got, EndReason::DaemonStopping)) => assert_eq!(got, id),
            other => panic!("expected a close, got {other:?}"),
        }
    }
}
