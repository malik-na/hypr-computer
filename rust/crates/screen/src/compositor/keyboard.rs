//! The agent's keyboard: text, typed as key presses given straight to the
//! screen's seat, to the app on the screen.

use smithay::backend::input::KeyState;
use smithay::input::keyboard::xkb::{self, Keysym};
use smithay::input::keyboard::{FilterResult, KeyboardHandle, Keycode};
use smithay::utils::SERIAL_COUNTER;

use crate::keymap::Key;
use crate::state::State;

impl State {
    /// Types `text`. A control character other than newline and tab is
    /// refused before anything is typed.
    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        let keyboard = self.keyboard()?;
        let keys = text
            .chars()
            .map(|c| {
                let sym = keysym(c)?;
                self.keymap
                    .find(sym)
                    .ok_or_else(|| format!("can't type {c:?}: it isn't on a US keyboard"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for key in keys {
            self.tap(&keyboard, key);
        }
        Ok(())
    }

    /// Presses and releases `key`, holding Shift if it needs it.
    fn tap(&mut self, keyboard: &KeyboardHandle<State>, key: Key) {
        let shift = self.keymap.shift;
        if key.shift {
            self.key(keyboard, shift, KeyState::Pressed);
        }
        self.key(keyboard, key.code, KeyState::Pressed);
        self.key(keyboard, key.code, KeyState::Released);
        if key.shift {
            self.key(keyboard, shift, KeyState::Released);
        }
    }

    fn key(&mut self, keyboard: &KeyboardHandle<State>, code: Keycode, state: KeyState) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.time();
        keyboard.input::<(), _>(self, code, state, serial, time, |_, _, _| {
            FilterResult::Forward
        });
    }

    /// The keyboard, if an app has it to type into.
    fn keyboard(&self) -> Result<KeyboardHandle<State>, String> {
        let keyboard = self
            .seat
            .get_keyboard()
            .ok_or("the screen has no keyboard")?;
        if keyboard.current_focus().is_none() {
            return Err("there is no app on your screen to type into".to_owned());
        }
        Ok(keyboard)
    }
}

/// The key symbol that types `c`.
fn keysym(c: char) -> Result<Keysym, String> {
    match c {
        '\n' => Ok(Keysym::from(xkb::keysyms::KEY_Return)),
        '\t' => Ok(Keysym::from(xkb::keysyms::KEY_Tab)),
        c if c.is_control() => Err(format!("can't type the control character {c:?}")),
        c => Ok(xkb::utf32_to_keysym(u32::from(c))),
    }
}
