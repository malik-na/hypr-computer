//! `hypr-computer take <agent>`: the user takes over that agent's screen, as
//! a click into its live view does. `<agent>` must name exactly one agent,
//! whose view is open: the user's input reaches a screen only through it.

use domain::control::Reply;
use domain::takeover::{Control, Effect, Event};
use tokio::sync::oneshot;

use super::State;
use super::views::View;

impl State {
    pub(super) fn take(&mut self, agent: &str, reply: oneshot::Sender<Reply>) {
        let ids = match self.pick(agent) {
            Ok(ids) => ids,
            Err(why) => {
                let _ = reply.send(Reply::Refused(why));
                return;
            }
        };
        let id = match ids.as_slice() {
            [id] => *id,
            _ => {
                let _ = reply.send(Reply::Refused(format!(
                    "{} agents are called {agent:?}: give the session number on the screen you \
                     want, as in \"{}\".",
                    ids.len(),
                    self.titles(&ids)
                )));
                return;
            }
        };
        let open = self
            .sessions
            .data_mut(id)
            .is_some_and(|live| live.view == View::Open);
        if !open {
            let _ = reply.send(Reply::Refused(format!(
                "{} has no live view to take its screen over in. Run `hypr-computer watch` to \
                 open the views.",
                self.title(id)
            )));
            return;
        }
        match self.takeovers.get(id).map(|t| t.control) {
            Some(Control::User) => {
                let _ = reply.send(Reply::Done(format!(
                    "You already have {}'s screen.",
                    self.title(id)
                )));
                return;
            }
            Some(Control::Taking) => {
                let _ = reply.send(Reply::Done(format!(
                    "You get {}'s screen once its tool call in progress ends.",
                    self.title(id)
                )));
                return;
            }
            _ => {}
        }
        let effects = self.takeovers.step(Event::Take(id));
        let waits = !effects.contains(&Effect::TakenOver(id));
        let _ = reply.send(Reply::Done(format!(
            "Took over {}'s screen{}. Press Escape twice in its view, or run `hypr-computer give`, \
             to hand it back.",
            self.title(id),
            if waits {
                ", once its tool call in progress ends"
            } else {
                ""
            }
        )));
        self.take_over_effects(effects);
    }
}
