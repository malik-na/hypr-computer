//! The screen's clipboard, between the apps on it: copy in one, paste in
//! another. It is the screen's own: it is joined with the user's only while
//! the user has taken the screen over (`share.rs`).

use std::os::fd::OwnedFd;

use smithay::delegate_data_device;
use smithay::input::Seat;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};

use crate::state::State;

impl State {
    /// The app with the keyboard is the one that can paste.
    pub fn clipboard_to(&mut self, seat: &Seat<State>, focused: Option<&WlSurface>) {
        let app = focused.and_then(Resource::client);
        set_data_device_focus(&self.display, seat, app);
    }
}

impl SelectionHandler for State {
    /// The only selection the screen sets itself is the user's clipboard.
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _: Seat<Self>,
    ) {
        if let (SelectionTarget::Clipboard, Some(source)) = (ty, source) {
            self.copied_on_screen(source.mime_types());
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _: Seat<Self>,
        _: &(),
    ) {
        if ty == SelectionTarget::Clipboard {
            self.paste_from_user(mime_type, fd);
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);
