//! `key`: press a key or key combination on the agent's screen.

use ports::{Action, BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Key;

impl Tool for Key {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "keys": {
                    "type": "string",
                    "description": "A key, or keys joined with +, held down in order and let go \
                                    of together: Return, Escape, Tab, BackSpace, Delete, Up, \
                                    Page_Down, F5, ctrl+l, ctrl+shift+t, alt+Left, super+Return.",
                },
            },
            "required": ["keys"],
        });
        ToolSpec {
            name: "key",
            description: "Press a key or key combination in the app on your screen.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let keys = args::string(&args, "keys")?;
            Ok(match screen.act(Action::Key(keys.clone())).await {
                Ok(()) => ToolOutput::text(format!("Pressed {keys}.")),
                Err(e) => ToolOutput::error(format!("Couldn't press {keys}: {e}")),
            })
        })
    }

    /// A key that may type a character may be one of a password, typed a
    /// key at a time: it is kept as one character. Shortcuts are kept whole.
    fn logged(&self, args: &Map<String, Value>) -> Map<String, Value> {
        let mut logged = args.clone();
        if let Some(keys) = logged.get_mut("keys") {
            match keys.as_str() {
                Some(combo) if may_type_a_character(combo) => *keys = json!({"characters": 1}),
                Some(_) => {}
                None => *keys = args::characters(keys),
            }
        }
        logged
    }
}

/// Whether pressing `combo` may type a character, as one key of a password
/// typed a key at a time would: a key that isn't a named non-character key
/// (`Return`, `F5`, `Left`), alone or with Shift (`z`, `shift+q`, `7`,
/// `exclam`, `KP_1`), and not a shortcut (`ctrl+l`, `alt+Left`).
fn may_type_a_character(combo: &str) -> bool {
    let (held, key) = match combo.strip_suffix("++") {
        Some(held) => (held, "+"),
        None if combo == "+" => ("", combo),
        None => combo.rsplit_once('+').unwrap_or(("", combo)),
    };
    let named = |name: &str, names: &[&str]| names.iter().any(|n| n.eq_ignore_ascii_case(name));
    let shortcut = held
        .split('+')
        .any(|modifier| named(modifier, SHORTCUT_MODIFIERS));
    let lower = key.to_ascii_lowercase();
    let function_key = lower
        .strip_prefix('f')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let no_character = named(key, NO_CHARACTER)
        || named(key, SHORTCUT_MODIFIERS)
        || function_key
        || lower.starts_with("xf86");
    !shortcut && !no_character
}

/// Modifiers that make a key press a shortcut rather than typing.
const SHORTCUT_MODIFIERS: &[&str] = &[
    "ctrl",
    "control",
    "control_l",
    "control_r",
    "alt",
    "alt_l",
    "alt_r",
    "super",
    "super_l",
    "super_r",
    "meta",
    "meta_l",
    "meta_r",
    "win",
    "cmd",
    "hyper_l",
    "hyper_r",
];

/// Keys that type no character (besides F1, F2 and so on, and XF86 keys).
const NO_CHARACTER: &[&str] = &[
    "return",
    "enter",
    "kp_enter",
    "tab",
    "iso_left_tab",
    "escape",
    "esc",
    "backspace",
    "delete",
    "del",
    "insert",
    "home",
    "end",
    "up",
    "down",
    "left",
    "right",
    "page_up",
    "page_down",
    "prior",
    "next",
    "pageup",
    "pgup",
    "pagedown",
    "pgdn",
    "print",
    "pause",
    "scroll_lock",
    "caps_lock",
    "num_lock",
    "menu",
    "shift",
    "shift_l",
    "shift_r",
];
