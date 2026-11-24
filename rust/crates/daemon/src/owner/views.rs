//! Live views: each session's screen opens as a window on the user's desktop
//! once the screen is ready and the agent has said its name, [`LIMIT`] at a
//! time, and `hypr-computer watch` takes the user to them.

use std::sync::Arc;

use domain::SessionId;
use domain::control::Reply;
use domain::lifecycle::SessionState;
use domain::view::{LIMIT, title};
use ports::ScreenProcess;
use tokio::sync::oneshot;
use tracing::{info, warn};

use super::{News, State};

/// Where a session's view is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Not asked for yet.
    #[default]
    None,
    Opening,
    Open,
    /// Not shown: [`LIMIT`] other views were.
    Refused,
    /// It couldn't open, and why; it isn't tried again.
    Failed(String),
}

impl View {
    /// Whether it counts towards [`LIMIT`].
    fn shown(&self) -> bool {
        matches!(self, View::Opening | View::Open)
    }
}

impl State {
    pub(super) fn introduce(&mut self, id: SessionId, name: String) {
        if let Some(live) = self.sessions.data_mut(id) {
            live.name = Some(name);
        }
        self.open_view(id);
    }

    /// Opens session `id`'s view, if its screen is ready, its agent has said
    /// its name, it has none yet, and fewer than [`LIMIT`] are shown.
    pub(super) fn open_view(&mut self, id: SessionId) {
        if self.sessions.state(id) != Some(SessionState::Ready) {
            return;
        }
        let shown = self.shown_count();
        let Some(live) = self.sessions.data_mut(id) else {
            return;
        };
        let (View::None, Some(name), Some(screen)) = (&live.view, &live.name, &live.screen) else {
            return;
        };
        if shown >= LIMIT {
            live.view = View::Refused;
            warn!(
                "{id}: no live view: at most {LIMIT} agents are shown at once; it gets one \
                 when another agent's session ends"
            );
            return;
        }
        let title = title(name, id);
        let (screen, desktop) = (screen.clone(), self.desktop.clone());
        live.view = View::Opening;
        self.report(id, async move {
            // Placed first, so it never opens in the user's way.
            let opened = match desktop.place_views().await {
                Ok(()) => screen.open_view(title).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            News::View(opened)
        });
    }

    /// Opens the views that were refused, oldest first, while there is room.
    pub(super) fn show_waiting(&mut self) {
        let room = LIMIT.saturating_sub(self.shown_count());
        let waiting: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, live)| live.view == View::Refused)
            .map(|(id, _)| id)
            // One that is ending can't take the room.
            .filter(|id| self.sessions.state(*id) == Some(SessionState::Ready))
            .take(room)
            .collect();
        for id in waiting {
            if let Some(live) = self.sessions.data_mut(id) {
                live.view = View::None;
            }
            self.open_view(id);
        }
    }

    fn shown_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|(_, live)| live.view.shown())
            .count()
    }

    pub(super) fn view_opened(&mut self, id: SessionId, opened: Result<(), String>) {
        let Some(live) = self.sessions.data_mut(id) else {
            return;
        };
        match opened {
            Ok(()) => {
                live.view = View::Open;
                info!("{id}: live view open");
            }
            Err(why) => {
                warn!("{id}: no live view: {why}");
                live.view = View::Failed(why);
                // Its room goes to an agent that is waiting.
                self.show_waiting();
            }
        }
    }

    /// `hypr-computer watch`: takes the user to the views, and says whose
    /// they are. A view the desktop lost (it restarted, or the user closed
    /// the window) opens again first: opening an open view changes nothing.
    pub(super) fn watch(&self, reply: oneshot::Sender<Reply>) {
        let shown: Vec<(String, Arc<dyn ScreenProcess>)> = self
            .sessions
            .iter()
            .filter(|(_, live)| live.view.shown())
            .filter_map(|(id, live)| Some((title(live.name.as_ref()?, id), live.screen.clone()?)))
            .collect();
        if shown.is_empty() {
            let _ = reply.send(Reply::Refused(self.nothing_to_watch()));
            return;
        }
        let waiting = self
            .sessions
            .iter()
            .filter(|(_, live)| live.view == View::Refused)
            .count();
        let desktop = self.desktop.clone();
        tokio::spawn(async move {
            if desktop.place_views().await.is_ok() {
                for (title, screen) in &shown {
                    if let Err(e) = screen.open_view(title.clone()).await {
                        warn!("{title}: its live view didn't open again: {e}");
                    }
                }
            }
            let titles: Vec<String> = shown.into_iter().map(|(title, _)| title).collect();
            let answer = match desktop.show_views().await {
                Ok(place) => Reply::Done(format!(
                    "Showing {} on {place}.{}",
                    and_list(&titles),
                    not_shown(waiting)
                )),
                Err(e) => Reply::Refused(format!("Can't show the agents' screens: {e}.")),
            };
            let _ = reply.send(answer);
        });
    }

    fn nothing_to_watch(&self) -> String {
        if self.sessions.open_count() == 0 {
            return "No agent is working, so there is nothing to watch.".to_owned();
        }
        let failed = self.sessions.iter().find_map(|(_, live)| match &live.view {
            View::Failed(why) => Some(why.clone()),
            _ => None,
        });
        match failed {
            Some(why) => format!("No agent's screen can be shown: {why}."),
            None => "No agent's screen is ready to watch yet.".to_owned(),
        }
    }
}

/// "a", "a and b", "a, b and c".
pub(super) fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the user isn't shown, if anything.
fn not_shown(waiting: usize) -> String {
    match waiting {
        0 => String::new(),
        1 => format!(" 1 more agent isn't shown: at most {LIMIT} are shown at once."),
        n => format!(" {n} more agents aren't shown: at most {LIMIT} are shown at once."),
    }
}
