//! `type`: type text into the app on the agent's screen.

use ports::{Action, BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Type;

impl Tool for Type {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "What to type. \\n presses Enter, \\t Tab."},
            },
            "required": ["text"],
        });
        ToolSpec {
            name: "type",
            description: "Type text into the app on your screen, as key presses, in order.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let text = args::string(&args, "text")?;
            if let Some(c) = text
                .chars()
                .find(|c| c.is_control() && !matches!(c, '\n' | '\t'))
            {
                return Err(CallError::InvalidArguments(format!(
                    "`text` has the control character {c:?}; only \\n and \\t can be typed"
                )));
            }
            let chars = text.chars().count();
            Ok(match screen.act(Action::Type(text)).await {
                Ok(()) => ToolOutput::text(format!("Typed {chars} characters.")),
                Err(e) => ToolOutput::error(format!("Couldn't type: {e}")),
            })
        })
    }
}
