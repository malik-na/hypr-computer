//! `screenshot`: what is on the agent's screen now.

use ports::{BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::registry::{CallError, Content, Tool, ToolOutput, ToolSpec};

pub struct Screenshot;

impl Tool for Screenshot {
    fn spec(&self) -> ToolSpec {
        let schema = json!({"type": "object", "properties": {}});
        ToolSpec {
            name: "screenshot",
            description: "Take a screenshot of your screen. Returns it as a 1280x800 PNG. \
                          The screen is yours alone: no one else's windows are on it.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        _args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            Ok(match screen.screenshot().await {
                Ok(png) => ToolOutput {
                    content: vec![Content::Image(png)],
                    is_error: false,
                },
                Err(e) => ToolOutput::error(format!("couldn't take a screenshot: {e}")),
            })
        })
    }
}
