//! The agent's keyboard: text, typed as key presses given straight to the
//! screen's seat, to the app on the screen. A character a US keyboard lacks
//! gets a key of its own in the keymap (see `keymap.rs`).

use std::ops::Range;

use smithay::backend::input::KeyState;
use smithay::input::keyboard::xkb::{self, Keysym};
use smithay::input::keyboard::{FilterResult, KeyboardHandle, Keycode};
use smithay::utils::SERIAL_COUNTER;

use crate::keymap::Key;
use crate::state::State;

/// Some of the symbols to type, and those of them the keymap adds to the US
/// layout to type them.
struct Run {
    syms: Range<usize>,
    added: Vec<Keysym>,
}

impl State {
    /// Types `text`. A control character other than newline and tab is
    /// refused before anything is typed.
    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        let keyboard = self.keyboard()?;
        let syms = text.chars().map(keysym).collect::<Result<Vec<_>, _>>()?;
        for run in self.runs(&syms)? {
            if self.keymap.add(&run.added)? {
                self.send_keymap()?;
            }
            for sym in &syms[run.syms] {
                let key = self.keymap.find(*sym).ok_or_else(|| {
                    format!("can't type {}: no key for it", xkb::keysym_get_name(*sym))
                })?;
                self.tap(&keyboard, key);
            }
        }
        Ok(())
    }

    /// Splits `syms` into runs that one keymap each can type.
    fn runs(&self, syms: &[Keysym]) -> Result<Vec<Run>, String> {
        let room = self.keymap.room();
        let mut runs = Vec::new();
        let (mut start, mut added) = (0, Vec::new());
        for (i, sym) in syms.iter().enumerate() {
            if self.keymap.on_us_keyboard(*sym) || added.contains(sym) {
                continue;
            }
            if room == 0 {
                return Err(
                    "can't type characters a US keyboard lacks: no keys are free".to_owned(),
                );
            }
            if added.len() == room {
                runs.push(Run {
                    syms: start..i,
                    added: std::mem::take(&mut added),
                });
                start = i;
            }
            added.push(*sym);
        }
        runs.push(Run {
            syms: start..syms.len(),
            added,
        });
        Ok(runs)
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
