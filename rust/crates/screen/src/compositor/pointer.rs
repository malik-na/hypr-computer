//! The agent's pointer: moves and clicks, given straight to the screen's seat.

use ports::{Button, Point};
use screen::SIZE;
use smithay::backend::input::ButtonState;
use smithay::desktop::WindowSurfaceType;
use smithay::input::pointer::{ButtonEvent, MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point as Position, SERIAL_COUNTER};

use crate::state::State;

/// Where `point` is on the screen, or why it isn't.
fn on_screen(point: Point) -> Result<Position<f64, Logical>, String> {
    let (width, height) = SIZE;
    if point.x >= width || point.y >= height {
        return Err(format!(
            "({}, {}) is off your screen, which is {width}x{height}: x is 0 to {}, y is 0 to {}",
            point.x,
            point.y,
            width - 1,
            height - 1
        ));
    }
    Ok((f64::from(point.x), f64::from(point.y)).into())
}

/// The evdev code of `button` (linux/input-event-codes.h).
fn code(button: Button) -> u32 {
    match button {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
    }
}

impl State {
    /// Moves the pointer to `to`, which gives it to whatever is there.
    pub fn move_pointer(&mut self, to: Point) -> Result<(), String> {
        let location = on_screen(to)?;
        let pointer = self.pointer()?;
        let under = self.surface_under(location);
        let event = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.time(),
        };
        pointer.motion(self, under, &event);
        pointer.frame(self);
        Ok(())
    }

    pub fn click(&mut self, at: Point, button: Button, count: u32) -> Result<(), String> {
        self.move_pointer(at)?;
        for _ in 0..count {
            self.button(button, ButtonState::Pressed)?;
            self.button(button, ButtonState::Released)?;
        }
        Ok(())
    }

    fn button(&mut self, button: Button, state: ButtonState) -> Result<(), String> {
        let pointer = self.pointer()?;
        let event = ButtonEvent {
            button: code(button),
            state,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.time(),
        };
        pointer.button(self, &event);
        pointer.frame(self);
        Ok(())
    }

    fn pointer(&self) -> Result<PointerHandle<State>, String> {
        self.seat
            .get_pointer()
            .ok_or_else(|| "the screen has no pointer".to_owned())
    }

    /// The surface at `location`, popups and subsurfaces included, and where
    /// that surface's corner is on the screen.
    fn surface_under(
        &self,
        location: Position<f64, Logical>,
    ) -> Option<(WlSurface, Position<f64, Logical>)> {
        let (window, at) = self.space.element_under(location)?;
        let (surface, offset) =
            window.surface_under(location - at.to_f64(), WindowSurfaceType::ALL)?;
        Some((surface, (offset + at).to_f64()))
    }
}
