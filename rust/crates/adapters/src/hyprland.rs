//! The user's desktop when it is Hyprland, reached through its IPC socket as
//! `hyprctl` does: one request per connection, answered with `ok`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::view::APP_ID;
use ports::{BoxFuture, Desktop, DesktopError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// How long Hyprland's own notice stays, in milliseconds.
const NOTICE_MS: u32 = 8000;

/// How long Hyprland may take to answer.
const ANSWER_TIME: Duration = Duration::from_secs(5);

/// The workspace the views open on: Super+0 in Hyprland's example config
/// and in Omarchy.
const WORKSPACE: u32 = 10;

/// The name of the window rule that places the views. Declaring a rule again
/// by its name merges what it gives into the rule, `match` included, so a
/// key once matched on can't be taken out. This name is used for nothing
/// else (the Python server's view rule is `hypr-computer-view`, matching
/// another class and a title), and its `match` must only ever be the class:
/// to change that, use a new name.
const RULE: &str = "hypr-computer-live-views";

#[derive(Debug)]
pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    /// The Hyprland instance `signature` whose Wayland socket is
    /// `wayland_socket`. Its IPC socket is next to it, in the runtime dir
    /// Hyprland itself uses, which may not be the daemon's.
    pub fn new(wayland_socket: &Path, signature: &OsStr) -> Option<Self> {
        let dir = wayland_socket.parent()?;
        Some(Hyprland {
            socket: dir.join("hypr").join(signature).join(".socket.sock"),
        })
    }

    async fn ask(&self, request: String) -> Result<(), DesktopError> {
        let failed = |e: &dyn std::fmt::Display| {
            DesktopError(format!(
                "Hyprland at {} didn't answer: {e}",
                self.socket.display()
            ))
        };
        let asked = async {
            let mut stream = UnixStream::connect(&self.socket).await?;
            stream.write_all(request.as_bytes()).await?;
            stream.shutdown().await?;
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await?;
            Ok::<_, std::io::Error>(reply)
        };
        let reply = timeout(ANSWER_TIME, asked)
            .await
            .map_err(|e| failed(&e))?
            .map_err(|e| failed(&e))?;
        match reply.trim() {
            "ok" => Ok(()),
            refused => Err(DesktopError(format!(
                "Hyprland refused {request:?}: {refused}"
            ))),
        }
    }
}

impl Desktop for Hyprland {
    fn place_views(&self) -> BoxFuture<'_, Result<(), DesktopError>> {
        // Declaring the rule again updates it (checked on Hyprland 0.57.2),
        // so this can be repeated: it puts the rule back after a config
        // reload drops it, and turns it back on if someone turned it off.
        let rule = format!(
            "/eval hl.window_rule({{name = \"{RULE}\", match = {{class = \"^{APP_ID}$\"}}, \
             workspace = \"{WORKSPACE} silent\", no_initial_focus = true, \
             focus_on_activate = false, enabled = true}})"
        );
        Box::pin(self.ask(rule))
    }

    fn show_views(&self) -> BoxFuture<'_, Result<String, DesktopError>> {
        Box::pin(async {
            self.ask(format!(
                "/dispatch hl.dsp.focus({{workspace = {WORKSPACE}}})"
            ))
            .await?;
            Ok(format!("workspace {WORKSPACE}"))
        })
    }

    fn notify(&self, message: String) -> BoxFuture<'_, Result<(), DesktopError>> {
        // Icon 1 (info), the theme's colour; the rest of the line is the text.
        let message = message.replace('\n', " ");
        Box::pin(self.ask(format!("/notify 1 {NOTICE_MS} 0 {message}")))
    }
}
