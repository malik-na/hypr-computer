//! Live views: each session's screen as a window on the user's desktop.

use crate::SessionId;

/// The app id of every view's window, so the desktop can place them.
pub const APP_ID: &str = "hypr-computer-view";

/// How many views are shown at once, at most.
pub const LIMIT: usize = 4;

/// The longest agent name a view's title shows.
const MAX_NAME: usize = 64;

/// A view's title: the agent's name, as its MCP client gave it, and the
/// session's number, so two agents of the same kind can be told apart.
pub fn title(agent: &str, session: SessionId) -> String {
    format!("{} (session {})", agent_name(agent), session.get())
}

/// An agent's name as its view shows it: no control characters, at most
/// `MAX_NAME` characters, trimmed, and "agent" if that leaves nothing.
pub fn agent_name(agent: &str) -> String {
    let name: String = agent
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "agent".to_owned()
    } else {
        name.to_owned()
    }
}
