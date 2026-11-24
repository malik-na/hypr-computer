//! The keyboard's keymap, the US layout, and where each key symbol is on it.
//! Apps read keys through the keymap the seat sends them; this is the same
//! keymap, compiled once more so the screen can look keys up in it.

use smithay::input::keyboard::xkb::{self, Keycode, Keysym};

/// The US layout, from xkbcommon's own files: what the `evdev` rules give
/// for model `pc105` and layout `us`, whatever the environment says.
const US_LAYOUT: &str = r#"xkb_keymap {
    xkb_keycodes { include "evdev+aliases(qwerty)" };
    xkb_types { include "complete" };
    xkb_compat { include "complete" };
    xkb_symbols { include "pc+us+inet(evdev)" };
};
"#;

/// A key that types a symbol, and whether it needs Shift for it.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub code: Keycode,
    pub shift: bool,
}

pub struct Keymap {
    us: xkb::Keymap,
    /// The left Shift key.
    pub shift: Keycode,
}

impl Keymap {
    pub fn us() -> Result<Keymap, String> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let us = compile(&context, US_LAYOUT)?;
        let shift = find(&us, Keysym::from(xkb::keysyms::KEY_Shift_L))
            .ok_or("the US keymap has no Shift key")?
            .code;
        Ok(Keymap { us, shift })
    }

    /// The text of the keymap the seat should have.
    pub fn source(&self) -> String {
        US_LAYOUT.to_owned()
    }

    /// The key that types `sym`.
    pub fn find(&self, sym: Keysym) -> Option<Key> {
        find(&self.us, sym)
    }
}

fn compile(context: &xkb::Context, text: &str) -> Result<xkb::Keymap, String> {
    xkb::Keymap::new_from_string(
        context,
        text.to_owned(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .ok_or_else(|| "can't compile the keymap: are xkeyboard-config's files installed?".to_owned())
}

/// The key that types `sym` in `keymap`: one without Shift if there is one.
fn find(keymap: &xkb::Keymap, sym: Keysym) -> Option<Key> {
    let codes = keymap.min_keycode().raw()..=keymap.max_keycode().raw();
    [false, true].into_iter().find_map(|shift| {
        codes.clone().map(Keycode::new).find_map(|code| {
            let syms = keymap.key_get_syms_by_level(code, 0, u32::from(shift));
            (syms == [sym]).then_some(Key { code, shift })
        })
    })
}
