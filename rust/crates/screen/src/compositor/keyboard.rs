//! The agent's keyboard: key combinations, and text typed as key presses,
//! given straight to the screen's seat, to the app on the screen. A character
//! a US keyboard lacks gets a key of its own in the keymap (see `keymap.rs`).

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

    /// Presses a combination like `ctrl+shift+t`, then lets go of it. A name
    /// that isn't a key is refused before anything is pressed.
    pub fn press_keys(&mut self, combo: &str) -> Result<(), String> {
        let keyboard = self.keyboard()?;
        let syms = parts(combo)
            .into_iter()
            .map(|name| key_named(name, combo))
            .collect::<Result<Vec<_>, _>>()?;
        let mut added: Vec<Keysym> = Vec::new();
        for sym in &syms {
            if !self.keymap.on_us_keyboard(*sym) && !added.contains(sym) {
                added.push(*sym);
            }
        }
        if added.len() > self.keymap.room() {
            return Err(format!("{combo:?} has too many keys a US keyboard lacks"));
        }
        if self.keymap.add(&added)? {
            self.send_keymap()?;
        }
        let shift = self.keymap.shift;
        let mut held = Vec::new();
        for sym in syms {
            let key = self.keymap.find(sym).ok_or_else(|| {
                format!("can't press {}: no key for it", xkb::keysym_get_name(sym))
            })?;
            if key.shift && !held.contains(&shift) {
                self.key(&keyboard, shift, KeyState::Pressed);
                held.push(shift);
            }
            self.key(&keyboard, key.code, KeyState::Pressed);
            held.push(key.code);
        }
        for code in held.into_iter().rev() {
            self.key(&keyboard, code, KeyState::Released);
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

/// Names agents use for keys, besides xkb's own (`Return`, `Page_Down`).
const KEY_NAMES: &[(&str, u32)] = &[
    ("ctrl", xkb::keysyms::KEY_Control_L),
    ("control", xkb::keysyms::KEY_Control_L),
    ("shift", xkb::keysyms::KEY_Shift_L),
    ("alt", xkb::keysyms::KEY_Alt_L),
    ("super", xkb::keysyms::KEY_Super_L),
    ("meta", xkb::keysyms::KEY_Super_L),
    ("win", xkb::keysyms::KEY_Super_L),
    ("cmd", xkb::keysyms::KEY_Super_L),
    ("enter", xkb::keysyms::KEY_Return),
    ("esc", xkb::keysyms::KEY_Escape),
    ("backspace", xkb::keysyms::KEY_BackSpace),
    ("del", xkb::keysyms::KEY_Delete),
    ("pageup", xkb::keysyms::KEY_Page_Up),
    ("pgup", xkb::keysyms::KEY_Page_Up),
    ("pagedown", xkb::keysyms::KEY_Page_Down),
    ("pgdn", xkb::keysyms::KEY_Page_Down),
];

/// The keys of a combination: `ctrl+shift+t` is `ctrl`, `shift`, `t`, and
/// `ctrl++` is `ctrl`, `+`.
fn parts(combo: &str) -> Vec<&str> {
    match combo.strip_suffix("++") {
        Some(rest) => rest.split('+').chain(["+"]).collect(),
        None if combo == "+" => vec!["+"],
        None => combo.split('+').collect(),
    }
}

/// The key symbol of a key's name, in any case: a name from `KEY_NAMES`,
/// an xkb key name, or a single character, which is its key (`L` is `l`).
fn key_named(name: &str, combo: &str) -> Result<Keysym, String> {
    let lower = name.to_lowercase();
    if let Some((_, sym)) = KEY_NAMES.iter().find(|(alias, _)| *alias == lower) {
        return Ok(Keysym::from(*sym));
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(xkb::utf32_to_keysym(u32::from(c.to_ascii_lowercase())));
    }
    for flags in [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE] {
        let sym = xkb::keysym_from_name(name, flags);
        if sym.raw() != xkb::keysyms::KEY_NoSymbol {
            return Ok(sym);
        }
    }
    Err(format!(
        "{name:?} in {combo:?} isn't a key. Name keys like ctrl, shift, alt, super, Return, \
         Tab, Escape, BackSpace, Delete, Up, Page_Down or F5, or give a single character, \
         joined with +"
    ))
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
