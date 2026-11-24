//! Apps bringing their own windows to the front (`xdg_activation_v1`), as a
//! browser does when a page in a window behind the shown one is opened. On
//! the agent's own screen every app is the agent's, so every request is
//! granted: the window is shown, as a new window would be.

use std::time::Duration;

use smithay::delegate_xdg_activation;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::state::State;

/// How long a token an app never used is kept.
const TOKEN_LIFE: Duration = Duration::from_secs(10);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, _data: XdgActivationTokenData) -> bool {
        // Tokens an app asked for and never used don't pile up.
        self.xdg_activation_state
            .retain_tokens(|_, data| data.timestamp.elapsed() < TOKEN_LIFE);
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        _token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // A token is good for one activation.
        self.xdg_activation_state.remove_token(&token);
        if let Some(window) = self.window_for(&surface).cloned() {
            // It is now the newest: when the window in front of it closes,
            // it is the one that comes back.
            self.windows.retain(|w| *w != window);
            self.windows.push(window.clone());
            self.show(window);
        }
    }
}

delegate_xdg_activation!(State);
