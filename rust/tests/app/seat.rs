//! The app's seat: it notes what the screen offers.

use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::App;

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            app.seat.clear();
            if caps.contains(wl_seat::Capability::Pointer) {
                app.seat.push("pointer");
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                app.seat.push("keyboard");
            }
        }
    }
}
