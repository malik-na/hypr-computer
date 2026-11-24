//! Taking over an agent's screen and handing it back, as specified in specs/TakeOver.tla.
//!
//! `TakeOvers` holds what the daemon believes about take-overs: each session's `control` and `call`, and whether
//! the user's last key or click since the take-over was Escape. The clipboards are the world: they are joined from
//! a `TakenOver(k)` effect to the next `Given(k)`.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `TakeOvers` per daemon, stepped one event at a time.** `Connect` a fresh id when a session is created
//!   and `Disconnect` it when the session is gone. Ids aren't reused.
//! - **Every tool call steps `ToolCall` first.** On `Admit` it runs; on `Wait` it is held, unanswered, until a
//!   `Resume`. With no effect, a call of the session is still in progress: the new one waits for its `Finish`.
//! - **`Finish` is stepped when a running call has given all its input**, before the agent gets its result.
//! - **The view gives the screen exactly the inputs answered with `Forward`.**
//! - **`TakenOver(k)`:** banner on, clipboards joined. **`Given(k)`:** banner off, clipboards apart, the user's
//!   held buttons and keys released.

use std::collections::BTreeMap;

/// Who drives the screen. The spec's `control`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Agent,
    /// The user clicked in while a call was running; they get the screen when it ends.
    Taking,
    User,
}

/// The agent's tool call. The spec's `call`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Idle,
    /// Arrived while the screen was taken over: no answer, no input yet.
    Waiting,
    /// Let through: it may give input until it reports `Finish`.
    Running,
}

/// One session's take-over state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTakeOver {
    pub control: Control,
    pub call: Call,
}

/// A key the user pressed in a view, as far as handing back cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Other,
}

/// Something that happened, or is asked for, in session `K`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<K> {
    Connect(K),
    Disconnect(K),
    /// The agent's tool call reaches the guard.
    ToolCall(K),
    /// The admitted (or resumed) call has given all its input.
    Finish(K),
    /// The user presses a pointer button in `K`'s view.
    Click(K),
    /// Any other pointer input in `K`'s view: motion, a button's release, scroll.
    Pointer(K),
    /// The user presses a key in `K`'s view.
    Key(K, Key),
    /// `hypr-computer take <agent>`.
    Take(K),
    /// `hypr-computer give`.
    Give,
}

/// What the caller must do because of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect<K> {
    /// Run the agent's call now.
    Admit(K),
    /// Hold the agent's call, unanswered.
    Wait(K),
    /// Run the held call now: the user took over and handed back.
    Resume(K),
    /// Give the user's input to `K`'s screen.
    Forward(K),
    /// The user has `K`'s screen: show the banner, join the clipboards.
    TakenOver(K),
    /// `K`'s screen is handed back: banner off, clipboards apart, the user's held buttons and keys released.
    Given(K),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    state: SessionTakeOver,
    /// The user's last key or click since the take-over was Escape.
    escaped: bool,
}

/// The take-over state of every connected session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TakeOvers<K> {
    sessions: BTreeMap<K, Slot>,
}

impl<K: Copy + Ord> TakeOvers<K> {
    /// No session.
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    /// The session's take-over state; `None` before its `Connect` and after its `Disconnect`.
    pub fn get(&self, id: K) -> Option<SessionTakeOver> {
        self.sessions.get(&id).map(|s| s.state)
    }

    /// The session taken over (`Taking` or `User`), if any. There is at most one.
    pub fn taken(&self) -> Option<K> {
        self.sessions
            .iter()
            .find(|(_, s)| s.state.control != Control::Agent)
            .map(|(&k, _)| k)
    }

    /// Applies one event, and says what to do about it. Pure: no I/O, no clock.
    pub fn step(&mut self, event: Event<K>) -> Vec<Effect<K>> {
        match event {
            Event::Connect(k) => {
                self.sessions.entry(k).or_insert(Slot {
                    state: SessionTakeOver {
                        control: Control::Agent,
                        call: Call::Idle,
                    },
                    escaped: false,
                });
                vec![]
            }
            Event::Disconnect(k) => match self.sessions.remove(&k) {
                Some(s) if s.state.control != Control::Agent => vec![Effect::Given(k)],
                _ => vec![],
            },
            Event::Give => self.taken().map(|j| self.give(j)).unwrap_or_default(),
            Event::Click(k) | Event::Take(k) if self.control(k) == Some(Control::Agent) => {
                let mut effects = self.taken().map(|j| self.give(j)).unwrap_or_default();
                effects.extend(self.take(k));
                effects
            }
            event => self.step_one(event),
        }
    }

    fn control(&self, k: K) -> Option<Control> {
        self.get(k).map(|s| s.control)
    }

    fn step_one(&mut self, event: Event<K>) -> Vec<Effect<K>> {
        let k = match event {
            Event::ToolCall(k)
            | Event::Finish(k)
            | Event::Click(k)
            | Event::Pointer(k)
            | Event::Key(k, _)
            | Event::Take(k) => k,
            Event::Connect(_) | Event::Disconnect(_) | Event::Give => return vec![],
        };
        let Some(s) = self.sessions.get_mut(&k) else {
            return vec![];
        };
        match (event, s.state.control, s.state.call) {
            (Event::ToolCall(_), Control::Agent, Call::Idle) => {
                s.state.call = Call::Running;
                vec![Effect::Admit(k)]
            }
            (Event::ToolCall(_), Control::Taking | Control::User, Call::Idle) => {
                s.state.call = Call::Waiting;
                vec![Effect::Wait(k)]
            }
            (Event::Finish(_), Control::Taking, Call::Running) => {
                s.state.call = Call::Idle;
                s.state.control = Control::User;
                s.escaped = false;
                vec![Effect::TakenOver(k)]
            }
            (Event::Finish(_), _, Call::Running) => {
                s.state.call = Call::Idle;
                vec![]
            }
            (Event::Pointer(_), Control::User, _) => vec![Effect::Forward(k)],
            (Event::Key(_, Key::Escape), Control::User, _) if s.escaped => self.give(k),
            (Event::Key(_, key), Control::User, _) => {
                s.escaped = key == Key::Escape;
                vec![Effect::Forward(k)]
            }
            (Event::Click(_), Control::User, _) => {
                s.escaped = false;
                vec![Effect::Forward(k)]
            }
            _ => vec![],
        }
    }

    /// Takes `k`'s screen over, `k` being driven by its agent and nothing else taken over.
    fn take(&mut self, k: K) -> Vec<Effect<K>> {
        let Some(s) = self.sessions.get_mut(&k) else {
            return vec![];
        };
        s.escaped = false;
        if s.state.call == Call::Running {
            s.state.control = Control::Taking;
            vec![]
        } else {
            s.state.control = Control::User;
            vec![Effect::TakenOver(k)]
        }
    }

    /// Hands `k`'s screen back to its agent, and lets a waiting call go on.
    fn give(&mut self, k: K) -> Vec<Effect<K>> {
        let Some(s) = self.sessions.get_mut(&k) else {
            return vec![];
        };
        s.state.control = Control::Agent;
        s.escaped = false;
        if s.state.call == Call::Waiting {
            s.state.call = Call::Running;
            vec![Effect::Given(k), Effect::Resume(k)]
        } else {
            vec![Effect::Given(k)]
        }
    }
}
