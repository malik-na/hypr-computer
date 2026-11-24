//! Which desktop the user has, from the environment the daemon got.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use ports::{BoxFuture, Desktop, DesktopError};

use crate::hyprland::Hyprland;

/// The user's desktop, given its Wayland socket (from `$WAYLAND_DISPLAY`)
/// and `$HYPRLAND_INSTANCE_SIGNATURE`. Live views need a desktop that can
/// keep them out of the user's way; on any other, they are refused.
pub fn desktop(wayland_socket: Option<&Path>, hyprland: Option<&OsStr>) -> Arc<dyn Desktop> {
    let Some(socket) = wayland_socket else {
        return Arc::new(Unsupported(
            "there is no desktop to show it on: WAYLAND_DISPLAY isn't set".to_owned(),
        ));
    };
    match hyprland
        .filter(|s| !s.is_empty())
        .and_then(|signature| Hyprland::new(socket, signature))
    {
        Some(hyprland) => Arc::new(hyprland),
        None => Arc::new(Unsupported(
            "live views need Hyprland for now, and HYPRLAND_INSTANCE_SIGNATURE isn't set"
                .to_owned(),
        )),
    }
}

/// A desktop live views can't be shown on, and why.
struct Unsupported(String);

impl Desktop for Unsupported {
    fn place_views(&self) -> BoxFuture<'_, Result<(), DesktopError>> {
        Box::pin(async { Err(DesktopError(self.0.clone())) })
    }

    fn show_views(&self) -> BoxFuture<'_, Result<String, DesktopError>> {
        Box::pin(async { Err(DesktopError(self.0.clone())) })
    }
}
