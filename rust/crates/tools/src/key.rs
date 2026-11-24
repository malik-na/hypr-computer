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
}
