//! The app's keyboard: it notes when the keyboard comes to it, and whether
//! its window had drawn by then. It reads each key with the keymap the screen
//! sent, as any app does, logs the key combination and the text it makes, and
//! repaints in a new colour.

use std::fs::File;
use std::os::unix::fs::FileExt;

use serde_json::json;
use wayland_client::protocol::wl_keyboard;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use xkbcommon::xkb;

use crate::App;

/// The keymap the screen sent last, and the keyboard's state in it.
pub struct Keyboard {
    context: xkb::Context,
    keymap: Option<(xkb::Keymap, xkb::State)>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            keymap: None,
        }
    }
}

/// Modifiers, in the order a key combination names them.
const MODIFIERS: [(&str, &str); 4] = [
    (xkb::MOD_NAME_CTRL, "ctrl"),
    (xkb::MOD_NAME_ALT, "alt"),
    (xkb::MOD_NAME_SHIFT, "shift"),
    (xkb::MOD_NAME_LOGO, "super"),
];

impl Keyboard {
    fn set_keymap(&mut self, file: File, size: u32) -> Result<(), String> {
        let mut text = vec![0; size as usize];
        file.read_exact_at(&mut text, 0)
            .map_err(|e| format!("can't read the keymap: {e}"))?;
        let text = String::from_utf8_lossy(&text)
            .trim_end_matches('\0')
            .to_owned();
        let keymap = xkb::Keymap::new_from_string(
            &self.context,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or("the screen sent a keymap that doesn't compile")?;
        let state = xkb::State::new(&keymap);
        self.keymap = Some((keymap, state));
        Ok(())
    }

    /// A pressed key as a combination, like `ctrl+shift+t`, and the text it
    /// types; `None` for a modifier on its own.
    fn read(&self, key: u32) -> Option<(String, String)> {
        let (keymap, state) = self.keymap.as_ref()?;
        let code = xkb::Keycode::new(key + 8);
        let layout = state.key_get_layout(code);
        let sym = *keymap.key_get_syms_by_level(code, layout, 0).first()?;
        if sym.is_modifier_key() {
            return None;
        }
        let mut combo: Vec<String> = MODIFIERS
            .iter()
            .filter(|(name, _)| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE))
            .map(|(_, word)| (*word).to_owned())
            .collect();
        combo.push(xkb::keysym_get_name(sym));
        let text = match state.key_get_one_sym(code).raw() {
            xkb::keysyms::KEY_Return | xkb::keysyms::KEY_KP_Enter => "\n".to_owned(),
            _ => state.key_get_utf8(code),
        };
        Some((combo.join("+"), text))
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                if let Err(e) = app.keyboard.set_keymap(File::from(fd), size) {
                    app.note(&json!({"event": "error", "error": e}));
                }
            }
            wl_keyboard::Event::Enter { .. } => {
                let drawn = app.drawn;
                app.note(&json!({"event": "keyboard", "drawn": drawn}));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some((_, state)) = &mut app.keyboard.keymap {
                    state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                if let Some((combo, text)) = app.keyboard.read(key) {
                    app.note(&json!({"event": "key", "key": combo, "text": text}));
                    app.input_seen();
                }
            }
            _ => {}
        }
    }
}
