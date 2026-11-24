//! The app's keyboard: it notes when the keyboard comes to it, and whether
//! its window had drawn by then.

use serde_json::json;
use wayland_client::protocol::wl_keyboard;
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::App;

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { .. } = event {
            let drawn = app.drawn;
            app.note(&json!({"event": "keyboard", "drawn": drawn}));
        }
    }
}
