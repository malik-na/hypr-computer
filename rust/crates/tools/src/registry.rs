//! The registry of tools and the guard every tool call goes through.

use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use ports::{BoxFuture, Png, Screen};
use serde_json::{Map, Value};

use crate::{click, drag, key, launch_app, r#move, open_url, screenshot, scroll, r#type};

/// Every tool, one line each: `&screenshot::Screenshot,`. A tool's file is
/// `src/<name>.rs` and its `mod` line is in `lib.rs`.
const TOOLS: &[&dyn Tool] = &[
    &screenshot::Screenshot,
    &launch_app::LaunchApp,
    &click::Click,
    &r#move::Move,
    &r#type::Type,
    &key::Key,
    &scroll::Scroll,
    &drag::Drag,
    &open_url::OpenUrl,
];

/// One MCP tool. Private to this crate: callers go through [`Registry::call`].
pub(crate) trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// Runs the tool on the caller's screen. Only the guard calls this, once
    /// the session has let the call through.
    fn call<'a>(
        &'a self,
        screen: &'a dyn Screen,
        args: Map<String, Value>,
    ) -> BoxFuture<'a, Result<ToolOutput, CallError>>;
    /// The call's arguments as the action log keeps them: anything that can
    /// hold a secret is replaced by its length. Pure.
    fn logged(&self, args: &Map<String, Value>) -> Map<String, Value> {
        args.clone()
    }
}

/// What `tools/list` says about a tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// A JSON Schema object for the tool's arguments.
    pub input_schema: Map<String, Value>,
}

/// Lets a session's tool calls through to its screen, and keeps its action
/// log. The daemon implements it with the session's lifecycle; the guard asks
/// it before every call and tells it how every call went.
pub trait Admit: Send + Sync {
    /// The session's screen, if a tool may use it now; otherwise why not, in
    /// words for the agent.
    fn admit(&self) -> BoxFuture<'_, Result<Arc<dyn Screen>, String>>;
    /// [`Admit::admit`], saying whether a refusal is the user's stop.
    fn admit_call(&self) -> BoxFuture<'_, Result<Arc<dyn Screen>, Refusal>> {
        Box::pin(async move {
            self.admit().await.map_err(|why| Refusal {
                why,
                stopped: false,
            })
        })
    }
    /// A call is over: it goes in the session's action log.
    fn log(&self, call: &Call<'_>) {
        let _ = call;
    }
}

/// Why a session's call wasn't let through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// In words for the agent.
    pub why: String,
    /// The user stopped the agent.
    pub stopped: bool,
}

/// One tool call, as the action log keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Call<'a> {
    /// When it came in.
    pub time: SystemTime,
    /// The tool's name, as the agent gave it.
    pub tool: &'a str,
    /// Its arguments as [`Tool::logged`] keeps them; `None` for a tool that
    /// doesn't exist, whose arguments nothing vouches for.
    pub arguments: Option<Map<String, Value>>,
    pub result: Outcome,
    /// The picture the agent got, if the call gave it one.
    pub picture: Option<Png>,
}

/// How a call ended, as the action log says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// The tool failed, or the call was refused for a reason other than a stop.
    Error,
    /// Refused, or ended, by the user's stop.
    Stopped,
    /// An unknown tool, or bad arguments.
    Invalid,
}

impl Outcome {
    pub fn word(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Stopped => "stopped",
            Outcome::Invalid => "invalid",
        }
    }
}

/// One piece of a tool's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image(Png),
}

/// A tool's result. `is_error` results go back to the agent as a failed tool
/// call it can read and act on, not as a protocol error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: Vec<Content>,
    pub is_error: bool,
}

impl ToolOutput {
    /// A call that worked, and what the agent should know about it.
    pub fn text(what: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(what.into())],
            is_error: false,
        }
    }

    /// A failed call, and what the agent should know about it.
    pub fn error(why: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(why.into())],
            is_error: true,
        }
    }
}

/// Why a call couldn't be made at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    UnknownTool(String),
    InvalidArguments(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::UnknownTool(name) => write!(f, "unknown tool: {name}"),
            CallError::InvalidArguments(why) => write!(f, "invalid arguments: {why}"),
        }
    }
}

impl std::error::Error for CallError {}

/// The tools an agent session can call.
pub struct Registry {
    tools: &'static [&'static dyn Tool],
}

impl Registry {
    /// Every tool in [`TOOLS`].
    pub fn builtin() -> Self {
        Self { tools: TOOLS }
    }

    #[cfg(test)]
    fn with(tools: &'static [&'static dyn Tool]) -> Self {
        Self { tools }
    }

    /// The tools, in registry order, for `tools/list`.
    pub fn list(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    /// The guard: every tool call goes through here and nowhere else. Checks
    /// that apply to every tool (stop, take-over, limits, the action log)
    /// belong in this function, not in the tools. A call the session doesn't
    /// let through gets a failed result that says why, and the tool never runs.
    /// One stopped on the way gets what [`Screen::end_call`] says instead of
    /// its result; otherwise [`Screen::note`] is added to it. Every call,
    /// whatever became of it, goes in the session's action log before the
    /// agent has its answer; one whose agent went away before it ended, as an
    /// error.
    pub async fn call(
        &self,
        session: &dyn Admit,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<ToolOutput, CallError> {
        let tool = self.tools.iter().find(|t| t.spec().name == name);
        let mut record = Record {
            session,
            call: Call {
                time: SystemTime::now(),
                tool: name,
                arguments: tool.map(|t| t.logged(&args)),
                result: Outcome::Error,
                picture: None,
            },
        };
        let (answer, result) = match tool {
            Some(tool) => run(*tool, session, args).await,
            None => (
                Err(CallError::UnknownTool(name.to_owned())),
                Outcome::Invalid,
            ),
        };
        record.call.result = result;
        record.call.picture = match &answer {
            Ok(output) if result == Outcome::Ok => output.content.iter().find_map(|c| match c {
                Content::Image(png) => Some(png.clone()),
                Content::Text(_) => None,
            }),
            _ => None,
        };
        answer
    }
}

/// A call's record: logged when the call ends, or when it is dropped
/// unfinished.
struct Record<'a> {
    session: &'a dyn Admit,
    call: Call<'a>,
}

impl Drop for Record<'_> {
    fn drop(&mut self) {
        self.session.log(&self.call);
    }
}

/// Runs `tool` if `session` lets it, and says how that went.
async fn run(
    tool: &dyn Tool,
    session: &dyn Admit,
    args: Map<String, Value>,
) -> (Result<ToolOutput, CallError>, Outcome) {
    let screen = match session.admit_call().await {
        Ok(screen) => screen,
        Err(refused) => {
            let result = if refused.stopped {
                Outcome::Stopped
            } else {
                Outcome::Error
            };
            return (Ok(ToolOutput::error(refused.why)), result);
        }
    };
    let output = tool.call(screen.as_ref(), args).await;
    if let Some(why) = screen.end_call().await {
        return (Ok(ToolOutput::error(why)), Outcome::Stopped);
    }
    let output = output.map(|mut output| {
        output.content.extend(screen.note().map(Content::Text));
        output
    });
    let result = match &output {
        Ok(output) if output.is_error => Outcome::Error,
        Ok(_) => Outcome::Ok,
        Err(_) => Outcome::Invalid,
    };
    (output, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ports::{Action, AppCommand, Launched, ScreenError};
    use std::collections::HashSet;

    /// A session whose screen is ready, or isn't.
    struct Ready(bool);

    struct Blank;

    impl Screen for Blank {
        fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
            Box::pin(async { Ok(Png(Vec::new())) })
        }
        fn act(&self, _action: Action) -> BoxFuture<'_, Result<(), ScreenError>> {
            Box::pin(async { Err(ScreenError("blank".into())) })
        }
        fn launch(&self, _app: AppCommand) -> BoxFuture<'_, Result<Launched, ScreenError>> {
            Box::pin(async { Err(ScreenError("blank".into())) })
        }
    }

    impl Admit for Ready {
        fn admit(&self) -> BoxFuture<'_, Result<Arc<dyn Screen>, String>> {
            let ready = self.0;
            Box::pin(async move {
                if ready {
                    Ok(Arc::new(Blank) as Arc<dyn Screen>)
                } else {
                    Err("not yet".to_owned())
                }
            })
        }
    }

    struct Echo;

    impl Tool for Echo {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo",
                description: "Says its `text` argument back.",
                input_schema: Map::new(),
            }
        }
        fn call<'a>(
            &'a self,
            _screen: &'a dyn Screen,
            args: Map<String, Value>,
        ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
            Box::pin(async move {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CallError::InvalidArguments("`text` is required".into()))?;
                Ok(ToolOutput {
                    content: vec![Content::Text(text.to_owned())],
                    is_error: false,
                })
            })
        }
    }

    #[test]
    fn builtin_tool_names_are_unique() {
        let names: Vec<_> = Registry::builtin().list().iter().map(|t| t.name).collect();
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "two tools share a name: {names:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_tool_is_refused_by_the_guard() {
        let err = Registry::builtin()
            .call(&Ready(true), "nope", Map::new())
            .await
            .unwrap_err();
        assert_eq!(err, CallError::UnknownTool("nope".into()));
        assert_eq!(err.to_string(), "unknown tool: nope");
    }

    #[tokio::test]
    async fn the_guard_lists_and_runs_registered_tools() {
        static TOOLS: &[&dyn Tool] = &[&Echo];
        let reg = Registry::with(TOOLS);
        assert_eq!(reg.list(), vec![Echo.spec()]);
        let mut args = Map::new();
        args.insert("text".into(), Value::from("hi"));
        let out = reg.call(&Ready(true), "echo", args.clone()).await.unwrap();
        assert_eq!(out.content, vec![Content::Text("hi".into())]);
        assert_eq!(
            reg.call(&Ready(true), "echo", Map::new()).await,
            Err(CallError::InvalidArguments("`text` is required".into()))
        );
        assert_eq!(
            reg.call(&Ready(false), "echo", args).await,
            Ok(ToolOutput::error("not yet"))
        );
    }
}
