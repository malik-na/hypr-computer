//! `scroll`: turn the mouse wheel over a point on the agent's screen.

use ports::{Action, BoxFuture, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Scroll;

/// Each direction, and one notch that way as (down, right).
const DIRECTIONS: [(&str, (i32, i32)); 4] = [
    ("down", (1, 0)),
    ("up", (-1, 0)),
    ("right", (0, 1)),
    ("left", (0, -1)),
];

impl Tool for Scroll {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "description": "Pixels from the left of your screen."},
                "y": {"type": "integer", "description": "Pixels from the top of your screen."},
                "direction": {"type": "string", "enum": ["down", "up", "left", "right"], "description": "Default down."},
                "amount": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Wheel notches. Default 3."},
            },
            "required": ["x", "y"],
        });
        ToolSpec {
            name: "scroll",
            description: "Scroll with the mouse wheel over (x, y) on your screen.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let at = args::point(&args, "x", "y")?;
            let names = DIRECTIONS.map(|(name, _)| name);
            let direction = args::choice(&args, "direction", "down", &names)?;
            let amount = args::count(&args, "amount", 3, (1, 50))?;
            let (down, right) = DIRECTIONS
                .iter()
                .find(|(name, _)| *name == direction)
                .map_or((1, 0), |(_, notch)| *notch);
            let (down, right) = (down * amount as i32, right * amount as i32);
            Ok(match screen.act(Action::Scroll { at, down, right }).await {
                Ok(()) => ToolOutput::text(format!(
                    "Scrolled {direction} {amount} notches at ({}, {}).",
                    at.x, at.y
                )),
                Err(e) => ToolOutput::error(format!("Couldn't scroll: {e}")),
            })
        })
    }
}
