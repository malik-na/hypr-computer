//! The screen's seat: the agent's own pointer and keyboard, which only the
//! daemon drives. The user's have no way in: there is no input device. The
//! keyboard goes to the window on the screen.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::delegate_seat;
use smithay::input::keyboard::XkbConfig;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::SERIAL_COUNTER;

use crate::state::State;

/// The seat apps see.
pub fn new_seat(
    seat_state: &mut SeatState<State>,
    display: &DisplayHandle,
) -> Result<Seat<State>, String> {
    let mut seat = seat_state.new_wl_seat(display, "agent");
    seat.add_pointer();
    let us = XkbConfig {
        rules: "evdev",
        model: "pc105",
        layout: "us",
        ..XkbConfig::default()
    };
    // No key repeat: the agent never holds a key down.
    seat.add_keyboard(us, 0, 0)
        .map_err(|e| format!("can't make a keyboard: {e}"))?;
    Ok(seat)
}

impl State {
    /// Gives the keyboard, and so every app, the keymap that `keymap` looks
    /// keys up in.
    pub fn send_keymap(&mut self) -> Result<(), String> {
        let keyboard = self
            .seat
            .get_keyboard()
            .ok_or("the screen has no keyboard")?;
        keyboard
            .set_keymap_from_string(self, self.keymap.source())
            .map_err(|e| format!("can't set the keymap: {e}"))
    }

    /// Gives the keyboard to the window on the screen once it has drawn:
    /// before that, the app hasn't set it up (foot 1.16 crashed on a
    /// keyboard that came earlier).
    pub fn focus_shown(&mut self) {
        let surface = self
            .shown
            .as_ref()
            .and_then(|w| w.toplevel())
            .map(|t| t.wl_surface().clone())
            .filter(|s| with_renderer_surface_state(s, |s| s.buffer().is_some()) == Some(true));
        if let Some(keyboard) = self.seat.get_keyboard()
            && keyboard.current_focus() != surface
        {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        self.clipboard_to(seat, focused);
    }
}

delegate_seat!(State);
