//! Drag and drop, with `--dnd`: moving with the left button down starts a
//! Wayland drag from the window, as apps do to drag something. The app takes
//! the drop itself, and logs where it landed.

use serde_json::json;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, event_created_child};

use crate::App;

const MIME: &str = "text/plain;charset=utf-8";

#[derive(Default)]
pub struct Dnd {
    /// `--dnd` was given.
    pub on: bool,
    pub manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    device: Option<wl_data_device::WlDataDevice>,
    /// The serial of the left button's press, while it is down.
    pub pressed: Option<u32>,
    started: bool,
    /// Where a drag is over the window.
    at: (f64, f64),
}

impl Dnd {
    pub fn new(on: bool) -> Dnd {
        Dnd {
            on,
            ..Dnd::default()
        }
    }
}

impl App {
    /// Takes the seat's data device, once both are there.
    pub fn take_data_device(&mut self, qh: &QueueHandle<App>) {
        if let (Some(manager), Some(seat)) = (&self.dnd.manager, &self.wl_seat) {
            self.dnd.device = Some(manager.get_data_device(seat, qh, ()));
        }
    }

    /// The pointer moved: with the left button down, a drag starts.
    pub fn start_drag(&mut self, qh: &QueueHandle<App>) {
        let dnd = &self.dnd;
        let (true, false, Some(manager), Some(device), Some(serial)) =
            (dnd.on, dnd.started, &dnd.manager, &dnd.device, dnd.pressed)
        else {
            return;
        };
        let source = manager.create_data_source(qh, ());
        source.offer(MIME.to_owned());
        source.set_actions(DndAction::Copy);
        device.start_drag(Some(&source), &self.window.surface, None, serial);
        self.dnd.started = true;
        self.note(&json!({"event": "drag", "state": "started"}));
    }

    /// The left button went up: the next press may start a new drag.
    pub fn release(&mut self) {
        self.dnd.pressed = None;
        self.dnd.started = false;
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Enter {
                serial, x, y, id, ..
            } => {
                if let Some(offer) = id {
                    offer.accept(serial, Some(MIME.to_owned()));
                    offer.set_actions(DndAction::Copy, DndAction::Copy);
                }
                app.dnd.at = (x, y);
            }
            wl_data_device::Event::Motion { x, y, .. } => app.dnd.at = (x, y),
            wl_data_device::Event::Drop => {
                let (x, y) = app.dnd.at;
                app.note(&json!({"event": "drop", "x": x, "y": y}));
            }
            _ => {}
        }
    }

    event_created_child!(App, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

delegate_noop!(App: wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(App: ignore wl_data_offer::WlDataOffer);
delegate_noop!(App: ignore wl_data_source::WlDataSource);
