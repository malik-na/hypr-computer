//! Taking over an agent's screen (`domain::takeover`, specs/TakeOver.tla):
//! the user's input in its live view, `hypr-computer give`, and the agent's
//! tool call that waits meanwhile.
//!
//! With stop (`stops.rs`): a stopped agent's calls are refused at once,
//! taken over or not, and a call waiting on a take-over is refused when its
//! agent is stopped. A stop doesn't hand the screen back: the user keeps it.

use domain::SessionId;
use domain::control::Reply;
use domain::stop::{self, Run};
use domain::takeover::{Call, Effect, Event, Key};
use domain::view::title;
use ports::{UserInput, ViewNotice};
use tokio::sync::oneshot;
use tracing::info;

use super::stops::stopped;
use super::{Admission, State};

/// What the first call let through after a take-over tells the agent: the
/// one that waited, or, if a stop refused that one, the next.
pub(super) const TOOK_OVER: &str = "The user took over your screen while this call waited, and \
     has handed it back: what is on it may have changed since you last looked.";

impl State {
    pub(super) fn takeover_connect(&mut self, id: SessionId) {
        self.takeovers.step(Event::Connect(id));
    }

    pub(super) fn takeover_disconnect(&mut self, id: SessionId) {
        let effects = self.takeovers.step(Event::Disconnect(id));
        self.take_over_effects(effects);
    }

    /// Reports the user's input in session `id`'s view to the owner, in
    /// order, until its screen can report no more.
    pub(super) fn watch_user(&mut self, id: SessionId) {
        let Some(screen) = self.screen(id) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(input) = screen.user_input().await {
                if tx.send(super::Msg::User(id, input)).is_err() {
                    return;
                }
            }
        });
    }

    /// The user gave `input` in session `id`'s view: it goes to the screen
    /// only if the take-over says so.
    pub(super) fn user_input(&mut self, id: SessionId, input: UserInput) {
        let event = match input {
            UserInput::Click => Event::Click(id),
            UserInput::Key { escape } => {
                Event::Key(id, if escape { Key::Escape } else { Key::Other })
            }
            UserInput::Other => Event::Pointer(id),
        };
        let effects = self.takeovers.step(event);
        let notice = if effects.contains(&Effect::Forward(id)) {
            ViewNotice::Forward
        } else {
            ViewNotice::Drop
        };
        self.tell(id, notice);
        self.take_over_effects(effects);
    }

    /// `hypr-computer give`.
    pub(super) fn give(&mut self, reply: oneshot::Sender<Reply>) {
        let Some(id) = self.takeovers.taken() else {
            let _ = reply.send(Reply::Refused(
                "No agent's screen is taken over: there is nothing to give back.".to_owned(),
            ));
            return;
        };
        let effects = self.takeovers.step(Event::Give);
        let _ = reply.send(Reply::Done(format!(
            "Gave {} its screen back.",
            self.title(id)
        )));
        self.take_over_effects(effects);
    }

    /// Does what the take-over's effects say, but `Admit`, `Wait` and
    /// `Forward`, which the caller answers.
    pub(super) fn take_over_effects(&mut self, effects: Vec<Effect<SessionId>>) {
        for effect in effects {
            match effect {
                Effect::TakenOver(id) => {
                    info!("{id}: the user took over its screen");
                    if let Some(live) = self.sessions.data_mut(id) {
                        live.taken_over = true;
                    }
                    self.tell(id, ViewNotice::TakenOver);
                }
                Effect::Given(id) => {
                    info!("{id}: the user handed its screen back");
                    self.tell(id, ViewNotice::Given);
                }
                Effect::Resume(id) => self.resume(id),
                Effect::Admit(_) | Effect::Wait(_) | Effect::Forward(_) => {}
            }
        }
    }

    /// The tool call of session `id` that waited runs now. One that is no
    /// longer there is over at once.
    fn resume(&mut self, id: SessionId) {
        let held = self.sessions.data_mut(id).and_then(|live| live.held.take());
        let admission = held.map(|reply| (reply, self.serve(id)));
        match admission {
            Some((reply, Ok(screen))) => {
                let admission = self.pass(id, screen);
                // A caller that went away drops its pass, which ends its call.
                let _ = reply.send(admission);
            }
            Some((reply, Err(why))) => {
                let _ = reply.send(Err(why));
            }
            None => {}
        }
        // A call that is no longer there is over; the next one may go.
        self.after_stop_step();
    }

    /// A tool call of session `id` the take-over holds, unanswered.
    pub(super) fn hold(&mut self, id: SessionId, reply: oneshot::Sender<Admission>) {
        info!("{id}: a tool call waits: the user has its screen");
        if let Some(live) = self.sessions.data_mut(id) {
            live.held = Some(reply);
        }
    }

    /// Ends, in the take-over, every call `Stops` has ended; and refuses
    /// the held calls of agents that were stopped.
    pub(super) fn finish_calls(&mut self) {
        let ids: Vec<SessionId> = self.sessions.iter().map(|(id, _)| id).collect();
        for id in ids {
            let stop = self.stops.get(id);
            if stop.is_some_and(|s| s.run != Run::Running)
                && let Some(reply) = self.sessions.data_mut(id).and_then(|live| live.held.take())
            {
                let _ = reply.send(Err(stopped()));
            }
            let running = self
                .takeovers
                .get(id)
                .is_some_and(|t| t.call == Call::Running);
            if running && stop.is_none_or(|s| s.call == stop::Call::Idle) {
                let effects = self.takeovers.step(Event::Finish(id));
                self.take_over_effects(effects);
            }
        }
    }

    fn tell(&mut self, id: SessionId, notice: ViewNotice) {
        if let Some(screen) = self.screen(id) {
            screen.tell_view(notice);
        }
    }

    /// "alpha (session 1)".
    pub(super) fn title(&self, id: SessionId) -> String {
        let name = self
            .sessions
            .iter()
            .find(|(i, _)| *i == id)
            .and_then(|(_, live)| live.name.clone())
            .unwrap_or_default();
        title(&name, id)
    }
}
