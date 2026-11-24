//! Stopping agents, as specified in specs/Stop.tla.
//!
//! `Stops` holds what the daemon believes about stopping: each session's `run` and `call`, and stop-all (the
//! spec's `stopNew`). What the screen holds down is the world, reported by the caller as events.
//!
//! What the caller must do, because `step` can't check it:
//! - **One `Stops` per daemon, stepped one event at a time.** `Connect` a fresh id when a session is created and
//!   `Disconnect` it when the session is gone. Ids aren't reused.
//! - **Every tool call steps `ToolCall` first**, and runs only on `Admit`. With no effect, a call of the session
//!   is still in progress: the new one waits for its `Finish` or `Abort`.
//! - **Every input is asked for first** (`Press` for one that leaves something held, `Move` for any other that
//!   isn't a release) and given only on `Give`. `Release` is stepped once everything held is released, `Finish`
//!   when the call is done.
//! - **On `Effect::Abort`** the call gives no more input: it releases what it holds, then steps `Abort`.
//! - **`Effect::Stopped`:** nothing of the session's is in progress or held; a waiting `hypr-computer stop`
//!   answers.

use std::collections::BTreeMap;

/// Whether a session serves. The spec's `run`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Running,
    /// No longer served; its call in progress hasn't released yet.
    Stopping,
    Stopped,
}

/// The session's tool call in progress. The spec's `call`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Idle,
    /// Let through, holding nothing.
    Admitted,
    /// Holding a button or key down.
    Input,
    /// Stopped while admitted or holding: it gives no more input, only its release.
    Aborting,
}

/// One session's stop state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionStop {
    pub run: Run,
    pub call: Call,
}

/// Something that happened, or is asked for, in session `K`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<K> {
    Connect(K),
    Disconnect(K),
    ToolCall(K),
    Press(K),
    Move(K),
    Release(K),
    Finish(K),
    Abort(K),
    Stop(K),
    StopAll,
    Resume(K),
    ResumeAll,
}

/// What the caller must do because of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect<K> {
    /// Run the call.
    Admit(K),
    /// Answer the call "Stopped by the user" and leave the screen alone.
    Refuse(K),
    /// Give the input that was asked for.
    Give(K),
    /// Give no more input: release what is held, then step `Abort`.
    Abort(K),
    /// Nothing of the session's is in progress or held: `hypr-computer stop` answers.
    Stopped(K),
}

/// The stop state of every connected session, and the stop-all switch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stops<K> {
    sessions: BTreeMap<K, SessionStop>,
    stop_new: bool,
}

impl<K: Copy + Ord> Stops<K> {
    /// No session, stop-all off.
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            stop_new: false,
        }
    }

    /// The session's stop state; `None` before its `Connect` and after its `Disconnect`.
    pub fn get(&self, id: K) -> Option<SessionStop> {
        self.sessions.get(&id).copied()
    }

    /// Whether `hypr-computer stop` (all) is in force: a session that connects starts stopped.
    pub fn stop_new(&self) -> bool {
        self.stop_new
    }

    /// Applies one event, and says what to do about it.
    pub fn step(&mut self, event: Event<K>) -> Vec<Effect<K>> {
        match event {
            Event::Connect(k) => {
                let run = if self.stop_new {
                    Run::Stopped
                } else {
                    Run::Running
                };
                self.sessions.entry(k).or_insert(SessionStop {
                    run,
                    call: Call::Idle,
                });
                vec![]
            }
            Event::Disconnect(k) => {
                self.sessions.remove(&k);
                vec![]
            }
            Event::StopAll => {
                self.stop_new = true;
                self.sessions
                    .iter_mut()
                    .filter_map(|(&k, s)| stop(s).then_some(Effect::Stopped(k)))
                    .collect()
            }
            Event::ResumeAll => {
                self.stop_new = false;
                for s in self.sessions.values_mut() {
                    s.run = Run::Running;
                }
                vec![]
            }
            event => self.step_one(event).into_iter().collect(),
        }
    }

    fn step_one(&mut self, event: Event<K>) -> Option<Effect<K>> {
        let (k, s) = match event {
            Event::ToolCall(k)
            | Event::Press(k)
            | Event::Move(k)
            | Event::Release(k)
            | Event::Finish(k)
            | Event::Abort(k)
            | Event::Stop(k)
            | Event::Resume(k) => (k, self.sessions.get_mut(&k)?),
            Event::Connect(_) | Event::Disconnect(_) | Event::StopAll | Event::ResumeAll => {
                return None;
            }
        };
        match (event, s.run, s.call) {
            (Event::ToolCall(_), Run::Stopping | Run::Stopped, _) => Some(Effect::Refuse(k)),
            (Event::ToolCall(_), Run::Running, Call::Idle) => {
                s.call = Call::Admitted;
                Some(Effect::Admit(k))
            }
            (Event::Press(_), _, Call::Admitted | Call::Input) => {
                s.call = Call::Input;
                Some(Effect::Give(k))
            }
            (Event::Move(_), _, Call::Admitted | Call::Input) => Some(Effect::Give(k)),
            (Event::Release(_), _, Call::Input) => {
                s.call = Call::Admitted;
                None
            }
            (Event::Finish(_), _, Call::Admitted) => {
                s.call = Call::Idle;
                None
            }
            (
                Event::Press(_) | Event::Move(_) | Event::Release(_) | Event::Finish(_),
                _,
                Call::Aborting,
            ) => Some(Effect::Abort(k)),
            (Event::Abort(_), run, Call::Aborting) => {
                s.call = Call::Idle;
                (run == Run::Stopping).then(|| {
                    s.run = Run::Stopped;
                    Effect::Stopped(k)
                })
            }
            (Event::Stop(_), Run::Running, _) => stop(s).then_some(Effect::Stopped(k)),
            (Event::Resume(_), Run::Stopping | Run::Stopped, _) => {
                s.run = Run::Running;
                None
            }
            _ => None,
        }
    }
}

/// Stops a running session and its call in progress. True if it is stopped at once: nothing was in progress.
fn stop(s: &mut SessionStop) -> bool {
    if s.run != Run::Running {
        return false;
    }
    s.call = match s.call {
        Call::Admitted | Call::Input => Call::Aborting,
        call => call,
    };
    s.run = if s.call == Call::Idle {
        Run::Stopped
    } else {
        Run::Stopping
    };
    s.run == Run::Stopped
}
