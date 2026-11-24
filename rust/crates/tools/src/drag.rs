//! `drag`: press the left button at one point, move to another, release.

use ports::{Action, BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Drag;

impl Tool for Drag {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "x1": {"type": "integer", "description": "Where to press: pixels from the left."},
                "y1": {"type": "integer", "description": "Where to press: pixels from the top."},
                "x2": {"type": "integer", "description": "Where to release: pixels from the left."},
                "y2": {"type": "integer", "description": "Where to release: pixels from the top."},
            },
            "required": ["x1", "y1", "x2", "y2"],
        });
        ToolSpec {
            name: "drag",
            description: "Press the left button at (x1, y1) on your screen, move to (x2, y2) \
                          and release: to select text, move a slider, or drag something.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let from = args::point(&args, "x1", "y1")?;
            let to = args::point(&args, "x2", "y2")?;
            Ok(match screen.act(Action::Drag { from, to }).await {
                Ok(()) => ToolOutput::text(format!(
                    "Dragged from ({}, {}) to ({}, {}).",
                    from.x, from.y, to.x, to.y
                )),
                Err(e) => ToolOutput::error(format!("Couldn't drag: {e}")),
            })
        })
    }
}
