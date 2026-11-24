//! The notice that tells the user an agent is at work (`domain::notice`,
//! specs/Notice.tla): one desktop notification per session, once its agent
//! has said its name, naming it as its live view does. OK on it lets the
//! agent go on; Stop stops that agent, as `hypr-computer stop <agent>` does.
//! When the session ends, a short-lived "done" takes its place: closing it
//! wouldn't do, as Omarchy's shell keeps a notification its sender closed on
//! screen.

use std::collections::BTreeMap;

use domain::SessionId;
use domain::notice::{Answer, Effect, Event, Notice};
use domain::stop;
use domain::view::title;
use ports::{Heard, Notification};
use tracing::{info, warn};

use super::State;

/// How long "done" stays on screen.
const DONE_MS: u32 = 4000;

/// The agents of the sessions `Notices` still knows, as their notices name
/// them: a session's name is still needed once it has gone.
pub(super) type Labels = BTreeMap<SessionId, String>;

impl State {
    /// Session `id`'s agent has said its name: its notice is asked for.
    pub(super) fn notice_connect(&mut self, id: SessionId) {
        let Some(name) = self
            .sessions
            .data_mut(id)
            .and_then(|live| live.name.clone())
        else {
            return;
        };
        self.labels.entry(id).or_insert_with(|| title(&name, id));
        self.notice_step(Event::Connect(id));
    }

    /// Session `id` is gone: its notice gives way to "done".
    pub(super) fn notice_disconnect(&mut self, id: SessionId) {
        self.notice_step(Event::Disconnect(id));
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
            Heard::Action(note, key) => match key.as_str() {
                "ok" => Event::Answer(note, Answer::Ok),
                "stop" => Event::Answer(note, Answer::Stop),
                _ => return,
            },
            Heard::Closed(note) => Event::Answer(note, Answer::Closed),
            Heard::Gone => {
                // Its notices went with it, and a new one reuses their ids:
                // they must never be taken for its own.
                warn!("the notification service went away, and the agents' notices with it");
                let shown: Vec<u32> = self
                    .labels
                    .keys()
                    .filter_map(|&id| match self.notices.get(id) {
                        Some(Notice::Shown(note)) => Some(note),
                        _ => None,
                    })
                    .collect();
                for note in shown {
                    self.notice_step(Event::Answer(note, Answer::Closed));
                }
                return;
            }
        };
        self.notice_step(event);
    }

    /// Whether a notice waits for the service's answer although its session
    /// has ended: it must be replaced when the answer comes.
    pub(super) fn notices_pending(&self) -> bool {
        self.labels
            .keys()
            .any(|&id| self.notices.get(id) == Some(Notice::Ending))
    }

    fn notice_step(&mut self, event: Event<SessionId, u32>) {
        for effect in self.notices.step(event) {
            match effect {
                Effect::Show(id) => {
                    let notice = notice(self.label(id));
                    self.notifications.show(id, notice);
                }
                Effect::Done(id, note) => {
                    info!("{id}: notice replaced by \"done\"");
                    self.notifications.send(done(self.label(id), note));
                }
                Effect::Stop(id) => {
                    // As `hypr-computer stop <agent>` does.
                    info!("Stopped {} from its notice.", self.label(id));
                    self.stops.step(stop::Event::Stop(id));
                    self.after_stop_step();
                }
                Effect::Host(_) => {}
            }
        }
        let notices = &self.notices;
        self.labels.retain(|&id, _| notices.get(id).is_some());
    }

    fn label(&self, id: SessionId) -> &str {
        self.labels.get(&id).map_or("", String::as_str)
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

/// "Done", in place of notice `note`: it goes away by itself.
fn done(label: &str, note: u32) -> Notification {
    Notification {
        summary: format!("AI agent done: {label}"),
        body: "It's no longer working.".to_owned(),
        actions: Vec::new(),
        expires_ms: Some(DONE_MS),
        replaces: Some(note),
    }
}
