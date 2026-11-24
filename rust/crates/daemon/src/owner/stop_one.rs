//! `hypr-computer stop <agent>` and `resume <agent>`: the agents of that
//! name, or the one of that session number, both as on their live views.
//! The others go on as they were.

use domain::SessionId;
use domain::control::Reply;
use domain::stop::Event;
use domain::view::title;
use tokio::sync::oneshot;
use tracing::info;

use super::State;
use super::views::and_list;

impl State {
    pub(super) fn stop_one(&mut self, agent: &str, reply: oneshot::Sender<Reply>) {
        let ids = match self.pick(agent) {
            Ok(ids) => ids,
            Err(why) => {
                let _ = reply.send(Reply::Refused(why));
                return;
            }
        };
        for &id in &ids {
            self.stops.step(Event::Stop(id));
        }
        let stays = if ids.len() == 1 {
            "Its screen stays"
        } else {
            "Their screens stay"
        };
        let answer = format!("Stopped {}. {stays}.", self.titles(&ids));
        info!("{answer}");
        self.answer_when_stopped(ids, answer, reply);
    }

    pub(super) fn resume_one(&mut self, agent: &str, reply: oneshot::Sender<Reply>) {
        let ids = match self.pick(agent) {
            Ok(ids) => ids,
            Err(why) => {
                let _ = reply.send(Reply::Refused(why));
                return;
            }
        };
        for &id in &ids {
            self.stops.step(Event::Resume(id));
        }
        let answer = format!("Resumed {}.", self.titles(&ids));
        info!("{answer}");
        let _ = reply.send(Reply::Done(answer));
        self.after_stop_step();
    }

    /// The sessions `agent` names, or why there are none.
    fn pick(&self, agent: &str) -> Result<Vec<SessionId>, String> {
        let number = agent.parse::<u64>().ok();
        // The name as its view shows it: `title` tidies both alike.
        let named = |id, name: &str| title(name, id) == title(agent, id);
        let ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(id, live)| {
                self.stops.get(*id).is_some()
                    && (Some(id.get()) == number || named(*id, live.name.as_deref().unwrap_or("")))
            })
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            return Err(format!(
                "No agent {agent:?} is working: give the name or the session number on its \
                 screen, as in \"claude-code (session 2)\"."
            ));
        }
        Ok(ids)
    }

    /// "alpha (session 1) and beta (session 2)".
    fn titles(&self, ids: &[SessionId]) -> String {
        let titles: Vec<String> = self
            .sessions
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(id, live)| title(live.name.as_deref().unwrap_or(""), id))
            .collect();
        and_list(&titles)
    }
}
