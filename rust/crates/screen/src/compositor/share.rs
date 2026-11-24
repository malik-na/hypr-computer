//! The user's clipboard, shared with the screen only while the user has it
//! (specs/TakeOver.tla's `clip`). The view is a client of the desktop's
//! clipboard as any app is: it gets what the user copies while it has the
//! keyboard, and it can set the user's clipboard.
//!
//! Joined at `taken`: the screen's clipboard becomes the user's, and what an
//! app on the screen copies then becomes the user's. Parted at `given`: the
//! screen's clipboard is emptied, and the desktop gets nothing more from the
//! screen, not even what was copied on it during the take-over.

use std::os::fd::AsFd;
use std::sync::Mutex;

use smithay::wayland::selection::data_device::{
    clear_data_device_selection, request_data_device_client_selection, set_data_device_selection,
};
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_seat,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};

use crate::state::State;

/// The view's hold on the desktop's clipboard.
pub struct Share {
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    device: Option<wl_data_device::WlDataDevice>,
    /// What the user's clipboard holds, as last offered to the view.
    offer: Option<wl_data_offer::WlDataOffer>,
    /// The selection the view set on the desktop for an app on the screen,
    /// until the desktop cancels it.
    ours: Option<wl_data_source::WlDataSource>,
    /// The serial of the user's last input in the view.
    pub serial: u32,
}

impl Share {
    pub fn new(
        manager: Option<wl_data_device_manager::WlDataDeviceManager>,
        seat: Option<&wl_seat::WlSeat>,
        qh: &QueueHandle<State>,
    ) -> Self {
        let device = match (&manager, seat) {
            (Some(manager), Some(seat)) => Some(manager.get_data_device(seat, qh, ())),
            _ => None,
        };
        Self {
            manager,
            device,
            offer: None,
            ours: None,
            serial: 0,
        }
    }
}

/// The types an offer has, as they come.
type Types = Mutex<Vec<String>>;

fn types(offer: &wl_data_offer::WlDataOffer) -> Vec<String> {
    offer
        .data::<Types>()
        .and_then(|t| t.lock().ok().map(|t| t.clone()))
        .unwrap_or_default()
}

impl State {
    /// The screen's clipboard becomes the user's.
    pub fn join_clipboards(&mut self) {
        let types = self
            .view
            .as_ref()
            .and_then(|v| v.share.offer.as_ref())
            .map(types);
        match types {
            Some(types) if !types.is_empty() => {
                set_data_device_selection(&self.display, &self.seat, types, ());
            }
            _ => clear_data_device_selection(&self.display, &self.seat),
        }
    }

    /// The screen's clipboard is its own again, and empty.
    pub fn part_clipboards(&mut self) {
        clear_data_device_selection(&self.display, &self.seat);
    }

    /// An app on the screen copied something: while the user has the
    /// screen, the user's clipboard gets it.
    pub fn copied_on_screen(&mut self, types: Vec<String>) {
        if !self.user.has_screen {
            return;
        }
        let Some(view) = &mut self.view else { return };
        let qh = view.queue_handle();
        let share = &mut view.share;
        let (Some(manager), Some(device)) = (&share.manager, &share.device) else {
            return;
        };
        let source = manager.create_data_source(&qh, ());
        for ty in types {
            source.offer(ty);
        }
        device.set_selection(Some(&source), share.serial);
        share.ours = Some(source);
        self.flush_view();
    }

    /// The user's last input in the view, for setting their clipboard.
    pub fn user_serial(&mut self, serial: u32) {
        if let Some(view) = &mut self.view {
            view.share.serial = serial;
        }
    }

    /// An app on the screen pastes the user's clipboard.
    pub fn paste_from_user(&mut self, mime_type: String, fd: std::os::fd::OwnedFd) {
        if !self.user.has_screen {
            return;
        }
        if let Some(offer) = self.view.as_ref().and_then(|v| v.share.offer.as_ref()) {
            offer.receive(mime_type, fd.as_fd());
        }
        self.flush_view();
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_data_device::Event::Selection { id } = event else {
            return;
        };
        let Some(view) = &mut state.view else { return };
        let share = &mut view.share;
        if let Some(old) = share.offer.take() {
            old.destroy();
        }
        if share.ours.is_some() {
            // The desktop shows the view its own selection: the screen has it.
            if let Some(id) = id {
                id.destroy();
            }
            return;
        }
        share.offer = id;
        if state.user.has_screen {
            state.join_clipboards();
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, Mutex::new(Vec::new())),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, Types> for State {
    fn event(
        _: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        types: &Types,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Ok(mut types) = types.lock()
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // The user pastes what an app on the screen copied: only while
            // they have the screen.
            wl_data_source::Event::Send { mime_type, fd } => {
                if state.user.has_screen {
                    let _ = request_data_device_client_selection(&state.seat, mime_type, fd);
                }
            }
            wl_data_source::Event::Cancelled => {
                if let Some(view) = &mut state.view
                    && view.share.ours.as_ref() == Some(source)
                {
                    view.share.ours = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_data_device_manager::WlDataDeviceManager,
        _: wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
