//! Live views: each session's screen opens as a window on the user's desktop
//! once the screen is ready, and closes with it.

use domain::SessionId;
use domain::lifecycle::SessionState;
use domain::view::title;
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
    /// It couldn't open; it isn't tried again.
    Failed,
}

impl State {
    /// Opens session `id`'s view, if its screen is ready and it has none yet.
    pub(super) fn open_view(&mut self, id: SessionId) {
        if self.sessions.state(id) != Some(SessionState::Ready) {
            return;
        }
        let Some(live) = self.sessions.data_mut(id) else {
            return;
        };
        let (View::None, Some(screen)) = (&live.view, &live.screen) else {
            return;
        };
        let title = title(id);
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
                live.view = View::Failed;
            }
        }
    }
}
