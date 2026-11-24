//! The registry of tools and the guard every tool call goes through.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use domain::SessionId;
use serde_json::{Map, Value};

/// Every tool, one line each: `&screenshot::Screenshot,`. A tool's file is
/// `src/<name>.rs` and its `mod` line is in `lib.rs`.
const TOOLS: &[&dyn Tool] = &[];

/// A boxed future, so tools can be stored side by side as `dyn Tool`.
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One MCP tool. Private to this crate: callers go through [`Registry::call`].
pub(crate) trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn call<'a>(
        &'a self,
        ctx: &'a CallContext,
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

/// Who is calling.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub session: SessionId,
}

/// One piece of a tool's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
}

/// A tool's result. `is_error` results go back to the agent as a failed tool
/// call it can read and act on, not as a protocol error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: Vec<Content>,
    pub is_error: bool,
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
    /// belong in this function, not in the tools.
    pub async fn call(
        &self,
        ctx: &CallContext,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<ToolOutput, CallError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .ok_or_else(|| CallError::UnknownTool(name.to_owned()))?;
        tool.call(ctx, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::Sessions;
    use std::collections::HashSet;
    use std::time::Instant;

    fn ctx() -> CallContext {
        CallContext {
            session: Sessions::new().open(Instant::now()),
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
            ctx: &'a CallContext,
            args: Map<String, Value>,
        ) -> BoxFuture<'a, Result<ToolOutput, CallError>> {
            Box::pin(async move {
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CallError::InvalidArguments("`text` is required".into()))?;
                Ok(ToolOutput {
                    content: vec![Content::Text(format!("{}: {text}", ctx.session))],
                    is_error: false,
                })
            })
        }
    }

    #[test]
    fn the_builtin_registry_is_empty_for_now() {
        assert_eq!(Registry::builtin().list(), Vec::new());
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
            .call(&ctx(), "nope", Map::new())
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
        let out = reg.call(&ctx(), "echo", args).await.unwrap();
        assert_eq!(out.content, vec![Content::Text("session 1: hi".into())]);
        assert_eq!(
            reg.call(&ctx(), "echo", Map::new()).await,
            Err(CallError::InvalidArguments("`text` is required".into()))
        );
    }
}
