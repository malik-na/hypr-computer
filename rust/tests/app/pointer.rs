//! The app's pointer: it logs every pointer event, and repaints in a new
//! colour after each frame of them, so a screenshot shows that they came.

use serde_json::{Value, json};
use wayland_client::protocol::wl_pointer;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::App;

/// Where the pointer is on the window, and whether this frame of events
/// changed anything.
#[derive(Default)]
pub struct Pointer {
    at: (f64, f64),
    moved: bool,
}

impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let logged = match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer.at = (surface_x, surface_y);
                json!({"event": "enter", "x": surface_x, "y": surface_y})
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer.at = (surface_x, surface_y);
                json!({"event": "motion", "x": surface_x, "y": surface_y})
            }
            wl_pointer::Event::Button { button, state, .. } => {
                let (x, y) = app.pointer.at;
                let state = match state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => "down",
                    _ => "up",
                };
                json!({"event": "button", "button": button_name(button), "state": state, "x": x, "y": y})
            }
            wl_pointer::Event::AxisValue120 { axis, value120 } => {
                let axis = match axis {
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => "horizontal",
                    _ => "vertical",
                };
                json!({"event": "scroll", "axis": axis, "value120": value120})
            }
            wl_pointer::Event::Frame => {
                if std::mem::take(&mut app.pointer.moved) {
                    app.input_seen();
                }
                Value::Null
            }
            _ => Value::Null,
        };
        if !logged.is_null() {
            app.pointer.moved = true;
            app.note(&logged);
        }
    }
}

fn button_name(button: u32) -> Value {
    match button {
        0x110 => json!("left"),
        0x111 => json!("right"),
        0x112 => json!("middle"),
        other => json!(other),
    }
}
