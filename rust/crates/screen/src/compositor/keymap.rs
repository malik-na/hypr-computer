//! The keyboard's keymap, and where each key symbol is on it. It is the US
//! layout, plus, while the agent types characters a US keyboard doesn't
//! have, keys added for them on keycodes the US layout leaves unused. Apps
//! read keys through the keymap the seat sends them, so they read those
//! characters as typed, with no input method.

use smithay::input::keyboard::xkb::{self, Keycode, Keysym};

/// The US layout, from xkbcommon's own files: what the `evdev` rules give
/// for model `pc105` and layout `us`, whatever the environment says. Its
/// symbols section is left open, for the added keys.
const US_LAYOUT: &str = r#"xkb_keycodes { include "evdev+aliases(qwerty)" };
    xkb_types { include "complete" };
    xkb_compat { include "complete" };
    xkb_symbols { include "pc+us+inet(evdev)""#;

/// Keycodes above this can confuse apps that still think in X11 terms.
const LAST_X11_KEYCODE: u32 = 255;

/// A key that types a symbol, and whether it needs Shift for it.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub code: Keycode,
    pub shift: bool,
}

pub struct Keymap {
    context: xkb::Context,
    us: xkb::Keymap,
    /// The keymap the seat has: the US layout plus `added`.
    current: xkb::Keymap,
    added: Vec<Keysym>,
    /// The names of the keys the US layout leaves unused, for `added`.
    spare: Vec<String>,
    /// The left Shift key.
    pub shift: Keycode,
}

impl Keymap {
    pub fn us() -> Result<Keymap, String> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let us = compile(&context, &source(&[], &[]))?;
        let spare = (us.min_keycode().raw()..=us.max_keycode().raw().min(LAST_X11_KEYCODE))
            .map(Keycode::new)
            .filter(|code| us.num_layouts_for_key(*code) == 0)
            .filter_map(|code| us.key_get_name(code).map(str::to_owned))
            .collect();
        let shift = find(&us, Keysym::from(xkb::keysyms::KEY_Shift_L))
            .ok_or("the US keymap has no Shift key")?
            .code;
        Ok(Keymap {
            context,
            current: us.clone(),
            us,
            added: Vec::new(),
            spare,
            shift,
        })
    }

    /// The text of the keymap the seat should have now.
    pub fn source(&self) -> String {
        source(&self.added, &self.spare)
    }

    /// The key that types `sym` in the keymap the seat has now.
    pub fn find(&self, sym: Keysym) -> Option<Key> {
        find(&self.current, sym)
    }

    pub fn on_us_keyboard(&self, sym: Keysym) -> bool {
        find(&self.us, sym).is_some()
    }

    /// How many symbols a keymap can add to the US layout.
    pub fn room(&self) -> usize {
        self.spare.len()
    }

    /// Makes `syms`, none of them on a US keyboard and no more than
    /// [`Keymap::room`], typable. `true` if the keymap changed, and the seat
    /// needs [`Keymap::source`].
    pub fn add(&mut self, syms: &[Keysym]) -> Result<bool, String> {
        if syms.iter().all(|sym| self.added.contains(sym)) {
            return Ok(false);
        }
        let text = source(syms, &self.spare);
        self.current = compile(&self.context, &text)?;
        self.added = syms.to_vec();
        Ok(true)
    }
}

/// The US layout, with `added` on the `spare` keys.
fn source(added: &[Keysym], spare: &[String]) -> String {
    let keys: String = added
        .iter()
        .zip(spare)
        .map(|(sym, key)| {
            format!(
                "\n        key <{key}> {{ [ {} ] }};",
                xkb::keysym_get_name(*sym)
            )
        })
        .collect();
    format!("xkb_keymap {{\n    {US_LAYOUT}{keys}\n    }};\n}};\n")
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
