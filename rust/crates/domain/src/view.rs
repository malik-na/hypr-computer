//! Live views: each session's screen as a window on the user's desktop.

use crate::SessionId;

/// The app id of every view's window, so the desktop can place them.
pub const APP_ID: &str = "hypr-computer-view";

/// The longest agent name a view's title shows.
const MAX_NAME: usize = 64;

/// A view's title: the agent's name, as its MCP client gave it, and the
/// session's number, so two agents of the same kind can be told apart.
pub fn title(agent: &str, session: SessionId) -> String {
    let name: String = agent
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let name = name.trim();
    let name = if name.is_empty() { "agent" } else { name };
    format!("{name} (session {})", session.get())
}
