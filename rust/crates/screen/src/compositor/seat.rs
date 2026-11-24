//! The screen's seat: the agent's own pointer and keyboard, which only the
//! daemon drives. The user's have no way in: there is no input device. The
//! keyboard goes to the window on the screen, or to its open menu.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::delegate_seat;
use smithay::desktop::{
    PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, find_popup_root_surface,
};
use smithay::input::keyboard::XkbConfig;
use smithay::input::pointer::Focus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{SERIAL_COUNTER, Serial};
use smithay::wayland::shell::xdg::PopupSurface;

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

impl State {
    /// An app's menu asks for the keyboard and pointer (`xdg_popup.grab`):
    /// keys go to the menu until it closes. Refused, and the menu closed, if
    /// something else holds them.
    pub fn grab_for_popup(&mut self, popup: PopupSurface, seat: &WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(seat) else {
            return;
        };
        let popup = PopupKind::Xdg(popup);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let Ok(mut grab) = self.popups.grab_popup(root, popup, &seat, serial) else {
            return;
        };
        let previous = grab.previous_serial().unwrap_or(serial);
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(previous))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed() && !(pointer.has_grab(serial) || pointer.has_grab(previous)) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
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
