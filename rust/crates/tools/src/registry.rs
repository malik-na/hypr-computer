//! The registry of tools and the guard every tool call goes through.

use std::fmt;
use std::sync::Arc;

use ports::{BoxFuture, Png, Screen};
use serde_json::{Map, Value};

use crate::screenshot;

/// Every tool, one line each: `&screenshot::Screenshot,`. A tool's file is
/// `src/<name>.rs` and its `mod` line is in `lib.rs`.
const TOOLS: &[&dyn Tool] = &[&screenshot::Screenshot];

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
}

/// What `tools/list` says about a tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// A JSON Schema object for the tool's arguments.
    pub input_schema: Map<String, Value>,
}

/// Lets a session's tool calls through to its screen. The daemon implements
/// it with the session's lifecycle; the guard asks it before every call.
pub trait Admit: Send + Sync {
    /// The session's screen, if a tool may use it now; otherwise why not, in
    /// words for the agent.
    fn admit(&self) -> BoxFuture<'_, Result<Arc<dyn Screen>, String>>;
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
    pub async fn call(
        &self,
        session: &dyn Admit,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<ToolOutput, CallError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .ok_or_else(|| CallError::UnknownTool(name.to_owned()))?;
        match session.admit().await {
            Ok(screen) => tool.call(screen.as_ref(), args).await,
            Err(why) => Ok(ToolOutput::error(why)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ports::ScreenError;
    use std::collections::HashSet;

    /// A session whose screen is ready, or isn't.
    struct Ready(bool);

    struct Blank;

    impl Screen for Blank {
        fn screenshot(&self) -> BoxFuture<'_, Result<Png, ScreenError>> {
            Box::pin(async { Ok(Png(Vec::new())) })
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
