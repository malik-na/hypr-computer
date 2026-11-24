//! Stopping agents: `hypr-computer stop` and `resume`, and the stop check of
//! every tool call and every input (`domain::stop`, specs/Stop.tla). A
//! session runs one tool call at a time; the others wait their turn here.

use std::sync::Arc;

use domain::SessionId;
use domain::control::Reply;
use domain::stop::{Effect, Event, Run};
use ports::Screen;
use tokio::sync::oneshot;
use tracing::info;

use super::pass::Pass;
use super::{Admission, State, refusal};

/// What a stopped agent's tool calls get.
pub(super) const STOPPED: &str = "Stopped by the user: your screen gets no more input from you \
     until they let you go on. Stop here and ask the user what to do next.";

/// A `hypr-computer stop` waiting for the inputs in progress to be released.
pub(super) struct Waiter {
    /// The sessions still stopping.
    sessions: Vec<SessionId>,
    answer: String,
    reply: oneshot::Sender<Reply>,
}

impl State {
    /// A tool call of session `id`. It waits until no other call of the
    /// session is in progress, then runs or is refused.
    pub(super) fn tool_call(&mut self, id: SessionId, reply: oneshot::Sender<Admission>) {
        match self.sessions.data_mut(id) {
            Some(live) => live.waiting.push_back(reply),
            None => {
                let _ = reply.send(Err(refusal(None)));
                return;
            }
        }
        self.admit_waiting();
    }

    /// Steps an event of a call in progress, and does what follows from it.
    pub(super) fn input(&mut self, event: Event<SessionId>) -> Vec<Effect<SessionId>> {
        let effects = self.stops.step(event);
        self.after_stop_step();
        effects
    }

    /// The pass of session `id`'s call number `call` went away before the
    /// call ended: it ends now, unless a later call is in progress.
    pub(super) fn dropped(&mut self, id: SessionId, call: u64) {
        if self.sessions.data_mut(id).map(|live| live.calls) != Some(call) {
            return;
        }
        for event in [Event::Release, Event::Finish, Event::Abort] {
            self.stops.step(event(id));
        }
        self.after_stop_step();
    }

    pub(super) fn stop_connect(&mut self, id: SessionId) {
        self.stops.step(Event::Connect(id));
        if self.stops.stop_new() {
            info!("{id}: stopped, as every agent is");
        }
    }

    pub(super) fn stop_disconnect(&mut self, id: SessionId) {
        self.stops.step(Event::Disconnect(id));
        self.after_stop_step();
    }

    /// `hypr-computer stop`: every agent, and every one that connects until
    /// `hypr-computer resume`. Answers once every input in progress is
    /// released.
    pub(super) fn stop_all(&mut self, reply: oneshot::Sender<Reply>) {
        self.stops.step(Event::StopAll);
        let ids = self.stop_ids();
        info!("stopped every agent ({})", ids.len());
        let answer = match ids.len() {
            0 => "No agent is working. Agents that start now are stopped until \
                  `hypr-computer resume`."
                .to_owned(),
            n => format!(
                "Stopped {}. Their screens stay; agents that start now are stopped too, \
                 until `hypr-computer resume`.",
                agents(n)
            ),
        };
        self.stop_waiters.push(Waiter {
            sessions: ids,
            answer,
            reply,
        });
        self.after_stop_step();
    }

    /// `hypr-computer resume`: every agent goes on, and agents that connect
    /// from now on work.
    pub(super) fn resume_all(&mut self, reply: oneshot::Sender<Reply>) {
        let n = self
            .stop_ids()
            .into_iter()
            .filter(|id| self.stops.get(*id).is_some_and(|s| s.run != Run::Running))
            .count();
        self.stops.step(Event::ResumeAll);
        info!("resumed every agent ({n})");
        let answer = match n {
            0 => "No agent was stopped. Agents that start now can work.".to_owned(),
            n => format!("Resumed {}.", agents(n)),
        };
        let _ = reply.send(Reply::Done(answer));
        self.after_stop_step();
    }

    /// The sessions `Stops` knows.
    fn stop_ids(&self) -> Vec<SessionId> {
        self.sessions
            .iter()
            .map(|(id, _)| id)
            .filter(|id| self.stops.get(*id).is_some())
            .collect()
    }

    /// Answers the stops whose sessions are stopped, resumed or gone, and
    /// lets through the tool calls that may run now.
    fn after_stop_step(&mut self) {
        let stops = &self.stops;
        for waiter in &mut self.stop_waiters {
            waiter
                .sessions
                .retain(|id| stops.get(*id).is_some_and(|s| s.run == Run::Stopping));
        }
        let (done, waiting) = std::mem::take(&mut self.stop_waiters)
            .into_iter()
            .partition(|w| w.sessions.is_empty());
        self.stop_waiters = waiting;
        for waiter in done {
            let _ = waiter.reply.send(Reply::Done(waiter.answer));
        }
        self.admit_waiting();
    }

    /// Lets each session's waiting tool calls through, in order, as far as
    /// `Stops` allows.
    fn admit_waiting(&mut self) {
        let ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, live)| !live.waiting.is_empty())
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            while let Some(reply) = self
                .sessions
                .data_mut(id)
                .and_then(|live| live.waiting.pop_front())
            {
                let admission = match self.serve(id) {
                    Err(why) => Err(why),
                    Ok(screen) => match self.stops.step(Event::ToolCall(id)).as_slice() {
                        [Effect::Admit(_)] => {
                            let call = self.sessions.data_mut(id).map_or(0, |live| {
                                live.calls += 1;
                                live.calls
                            });
                            let pass = Pass::new(id, call, screen, self.tx.clone());
                            Ok(Arc::new(pass) as Arc<dyn Screen>)
                        }
                        [Effect::Refuse(_)] => Err(STOPPED.to_owned()),
                        // Another call of the session is in progress.
                        _ => {
                            if let Some(live) = self.sessions.data_mut(id) {
                                live.waiting.push_front(reply);
                            }
                            break;
                        }
                    },
                };
                let admitted = admission.is_ok();
                // A caller that went away drops its pass, which ends its call.
                let _ = reply.send(admission);
                if admitted {
                    break;
                }
            }
        }
    }
}

/// "1 agent", "2 agents".
fn agents(n: usize) -> String {
    if n == 1 {
        "1 agent".to_owned()
    } else {
        format!("{n} agents")
    }
}
