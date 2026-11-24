//! Live views: each session's screen as a window on the user's desktop.

use crate::SessionId;

/// The app id of every view's window, so the desktop can place them.
pub const APP_ID: &str = "hypr-computer-view";

/// A view's title.
pub fn title(session: SessionId) -> String {
    format!("hypr-computer: session {}", session.get())
}
