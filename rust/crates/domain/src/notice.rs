//! The notice that tells the user an agent is at work, as specified in specs/Notice.tla.
//!
//! `Notices` holds what the daemon believes about each session's notice: the spec's `notice`. The notification
//! service, the host desktop and the user are the world: what they do comes in as events, and what the daemon
//! must do to them goes out as effects.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `Notices` per daemon, stepped one event at a time.** `Connect` a fresh id once the session's agent has
//!   said its name, and `Disconnect` it when the session is gone. Ids aren't reused.
//! - **On `Show(k)`, never drop the Notify call.** The timer only brings the host's notice: the call goes on, and
//!   its answer is stepped whenever it comes, even after the session has ended, so that a late notice is replaced
//!   by "done".
//! - **Step the service's messages in the order they came**, so that a Stop on a notice that has just appeared
//!   isn't lost.
//! - **On `Stop(k)`,** step `stop::Event::Stop(k)`: the notice's Stop is `hypr-computer stop <agent>`.

use std::collections::BTreeMap;

/// What the daemon believes about a session's notice. The spec's `notice`; its "None" is `get` giving `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice<N> {
    /// It asked the service to show the notice, and waits for the answer for a limited time.
    Asking,
    /// The service showed it, with this id.
    Shown(N),
    /// No answer in time: the host desktop showed its notice. Still waiting for the service's answer.
    Late,
    /// The service failed: the host desktop showed its notice.
    Fallback,
    /// The user pressed OK or Stop on it, or closed it: it is gone.
    Answered,
    /// The session ended while the daemon still waited for the answer: when it comes, it shows "done".
    Ending,
}

/// What the user did with a notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Pressed OK.
    Ok,
    /// Pressed Stop.
    Stop,
    /// Closed it.
    Closed,
}

/// Something that happened to session `K`'s notice, whose id with the notification service is an `N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<K, N> {
    /// The session is open and its agent has said its name.
    Connect(K),
    /// The session is gone.
    Disconnect(K),
    /// The service answered the session's Notify with this id: the notice is on screen.
    Shown(K, N),
    /// The session's Notify failed: no service, no session bus, or an error.
    Failed(K),
    /// The time the daemon waits for the service's answer is over.
    Timeout(K),
    /// The user answered the notification with this id.
    Answer(N, Answer),
}

/// What the daemon must do because of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect<K, N> {
    /// Ask the service to show the session's notice, and start the timer.
    Show(K),
    /// Have the host desktop show its own notice for the session; it goes away by itself.
    Host(K),
    /// Replace notification `N` with the session's short-lived "done" notice.
    Done(K, N),
    /// Stop the session, as `hypr-computer stop <agent>` does.
    Stop(K),
}

/// The notice of every session the daemon still has something to do for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notices<K, N> {
    notices: BTreeMap<K, Notice<N>>,
}

impl<K: Copy + Ord, N: Copy + Ord> Notices<K, N> {
    /// No session.
    pub fn new() -> Self {
        Self {
            notices: BTreeMap::new(),
        }
    }

    /// The session's notice; `None` before its `Connect`, and once nothing is left to do for it.
    pub fn get(&self, id: K) -> Option<Notice<N>> {
        self.notices.get(&id).copied()
    }

    /// Applies one event, and says what to do about it.
    pub fn step(&mut self, event: Event<K, N>) -> Vec<Effect<K, N>> {
        let k = match event {
            Event::Connect(k) => {
                if self.notices.contains_key(&k) {
                    return vec![];
                }
                self.notices.insert(k, Notice::Asking);
                return vec![Effect::Show(k)];
            }
            Event::Answer(n, _) => match self.shown_with(n) {
                Some(k) => k,
                None => return vec![],
            },
            Event::Disconnect(k) | Event::Shown(k, _) | Event::Failed(k) | Event::Timeout(k) => k,
        };
        let Some(&notice) = self.notices.get(&k) else {
            return vec![];
        };
        let (next, effect) = match (event, notice) {
            (Event::Disconnect(_), Notice::Asking | Notice::Late) => (Some(Notice::Ending), None),
            (Event::Disconnect(_), Notice::Shown(n)) => (None, Some(Effect::Done(k, n))),
            (Event::Disconnect(_), Notice::Fallback | Notice::Answered) => (None, None),
            (Event::Shown(_, n), Notice::Asking | Notice::Late) => (Some(Notice::Shown(n)), None),
            (Event::Shown(_, n), Notice::Ending) => (None, Some(Effect::Done(k, n))),
            (Event::Failed(_), Notice::Asking) => (Some(Notice::Fallback), Some(Effect::Host(k))),
            (Event::Failed(_), Notice::Late) => (Some(Notice::Fallback), None),
            (Event::Failed(_), Notice::Ending) => (None, None),
            (Event::Timeout(_), Notice::Asking) => (Some(Notice::Late), Some(Effect::Host(k))),
            (Event::Answer(_, Answer::Stop), Notice::Shown(_)) => {
                (Some(Notice::Answered), Some(Effect::Stop(k)))
            }
            (Event::Answer(_, Answer::Ok | Answer::Closed), Notice::Shown(_)) => {
                (Some(Notice::Answered), None)
            }
            _ => return vec![],
        };
        match next {
            Some(notice) => self.notices.insert(k, notice),
            None => self.notices.remove(&k),
        };
        effect.into_iter().collect()
    }

    /// The session whose notice is on screen with id `n`.
    fn shown_with(&self, n: N) -> Option<K> {
        self.notices
            .iter()
            .find_map(|(&k, &notice)| (notice == Notice::Shown(n)).then_some(k))
    }
}
