//! The screen's clipboard, between the apps on it: copy in one, paste in
//! another. It is the screen's own; nothing on it reaches the user's
//! clipboard, nor the other way round.

use smithay::delegate_data_device;
use smithay::input::Seat;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};

use crate::state::State;

impl State {
    /// The app with the keyboard is the one that can paste.
    pub fn clipboard_to(&mut self, seat: &Seat<State>, focused: Option<&WlSurface>) {
        let app = focused.and_then(Resource::client);
        set_data_device_focus(&self.display, seat, app);
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);
