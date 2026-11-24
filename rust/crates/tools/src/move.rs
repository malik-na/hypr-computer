//! `move`: move the agent's pointer without clicking.

use ports::{Action, BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Move;

impl Tool for Move {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "description": "Pixels from the left of your screen."},
                "y": {"type": "integer", "description": "Pixels from the top of your screen."},
            },
            "required": ["x", "y"],
        });
        ToolSpec {
            name: "move",
            description: "Move the pointer to (x, y) on your screen without clicking, e.g. \
                          to hover over something.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let to = args::point(&args, "x", "y")?;
            Ok(match screen.act(Action::Move(to)).await {
                Ok(()) => ToolOutput::text(format!("Moved the pointer to ({}, {}).", to.x, to.y)),
                Err(e) => ToolOutput::error(format!("Couldn't move the pointer: {e}")),
            })
        })
    }
}
