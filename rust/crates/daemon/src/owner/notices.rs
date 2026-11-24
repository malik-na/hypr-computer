//! The notice that tells the user an agent is at work (`domain::notice`,
//! specs/Notice.tla): one desktop notification per session, once its agent
//! has said its name, naming it as its live view does.

use domain::SessionId;
use domain::notice::{Effect, Event};
use domain::view::title;
use ports::{Heard, Notification};
use tracing::{info, warn};

use super::State;

impl State {
    /// Session `id`'s agent has said its name: its notice is asked for.
    pub(super) fn notice_connect(&mut self, id: SessionId) {
        if self.sessions.data_mut(id).is_some() {
            self.notice_step(Event::Connect(id));
        }
    }

    /// What the notification service did.
    pub(super) fn heard(&mut self, heard: Heard) {
        let event = match heard {
            Heard::Shown(id, note) => {
                info!("{id}: notice shown");
                Event::Shown(id, note)
            }
            Heard::Failed(id, why) => {
                warn!("{id}: no notice: {why}");
                Event::Failed(id)
            }
        };
        self.notice_step(event);
    }

    fn notice_step(&mut self, event: Event<SessionId, u32>) {
        for effect in self.notices.step(event) {
            match effect {
                Effect::Show(id) => {
                    let notice = notice(&self.label(id));
                    self.notifications.show(id, notice);
                }
                Effect::Host(_) | Effect::Done(..) | Effect::Stop(_) => {}
            }
        }
    }

    /// The session's agent as its live view names it.
    fn label(&self, id: SessionId) -> String {
        let name = self
            .sessions
            .iter()
            .find(|(session, _)| *session == id)
            .and_then(|(_, live)| live.name.clone())
            .unwrap_or_default();
        title(&name, id)
    }
}

/// The notice: it stays until the user presses OK or Stop.
fn notice(label: &str) -> Notification {
    Notification {
        summary: format!("AI agent at work: {label}"),
        body: "It's working on its own screen. Watch it with Super+0 or `hypr-computer watch`."
            .to_owned(),
        actions: vec![
            ("ok".to_owned(), "OK".to_owned()),
            ("stop".to_owned(), "Stop agent".to_owned()),
        ],
        expires_ms: None,
        replaces: None,
    }
}
