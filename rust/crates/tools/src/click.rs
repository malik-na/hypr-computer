//! `click`: click a point on the agent's screen.

use ports::{Action, BoxFuture, Button, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct Click;

impl Tool for Click {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "description": "Pixels from the left of your screen."},
                "y": {"type": "integer", "description": "Pixels from the top of your screen."},
                "button": {"type": "string", "enum": ["left", "right", "middle"], "description": "Default left."},
                "count": {"type": "integer", "minimum": 1, "maximum": 3, "description": "2 for a double click, 3 for a triple click. Default 1."},
            },
            "required": ["x", "y"],
        });
        ToolSpec {
            name: "click",
            description: "Click at (x, y) on your screen. Returns once the app has drawn its \
                          response, so your next screenshot shows it.",
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
            let names = Button::ALL.map(Button::name);
            let button = args::choice(&args, "button", "left", &names)?;
            let button = Button::from_name(button).unwrap_or(Button::Left);
            let count = args::count(&args, "count", 1, (1, 3))?;
            Ok(
                match screen.act(Action::Click { at, button, count }).await {
                    Ok(()) => ToolOutput::text(format!("Clicked at ({}, {}).", at.x, at.y)),
                    Err(e) => ToolOutput::error(format!("Couldn't click: {e}")),
                },
            )
        })
    }
}
