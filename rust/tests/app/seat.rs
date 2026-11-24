//! The app's seat: it takes the pointer and the keyboard the screen offers,
//! and notes which it was offered.

use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::App;

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            app.seat.clear();
            if caps.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
                app.seat.push("pointer");
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
                app.seat.push("keyboard");
            }
        }
    }
}
