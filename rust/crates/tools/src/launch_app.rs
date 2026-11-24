//! `launch_app`: start an installed app on the agent's screen.

use ports::{AppCommand, BoxFuture, Launched, Screen};
use serde_json::{Map, Value, json};

use crate::args;
use crate::registry::{CallError, Tool, ToolOutput, ToolSpec};

pub struct LaunchApp;

impl Tool for LaunchApp {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": {
                "app": {
                    "type": "string",
                    "description": "The program: a name on PATH, like foot or chromium, or a path.",
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Its arguments, one string each.",
                },
            },
            "required": ["app"],
        });
        ToolSpec {
            name: "launch_app",
            description: "Start an installed app on your screen. Returns once its window is \
                          there: it fills the screen, in front of any other. The app, and \
                          everything it starts, ends with your session.",
            input_schema: schema.as_object().cloned().unwrap_or_default(),
        }
    }

    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
        Box::pin(async move {
            let program = args::string(&args, "app")?;
            let app = AppCommand {
                program: program.clone(),
                args: args::strings(&args, "args")?,
            };
            Ok(match screen.launch(app).await {
                Ok(Launched::Window { pid }) => ToolOutput::text(format!(
                    "Started {program} (pid {pid}); its window is on your screen."
                )),
                Ok(Launched::NoWindow { pid, waited }) => ToolOutput::text(format!(
                    "Started {program} (pid {pid}), but it opened no window within {}s. \
                     Take a screenshot to see whether it has since.",
                    waited.as_secs()
                )),
                Ok(Launched::Exited {
                    status,
                    failed,
                    output,
                }) => {
                    let said = if output.is_empty() {
                        String::new()
                    } else {
                        format!(" It printed:\n{output}")
                    };
                    if failed {
                        ToolOutput::error(format!(
                            "{program} exited ({status}) before it opened a window.{said}"
                        ))
                    } else {
                        ToolOutput::text(format!(
                            "{program} ran and exited ({status}), and no window opened.{said}"
                        ))
                    }
                }
                Err(e) => ToolOutput::error(format!("Couldn't start {program}: {e}")),
            })
        })
    }
}
