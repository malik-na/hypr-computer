//! hypr-computer's MCP tools.
//!
//! One file per tool in this directory, each declared below with `mod` and
//! listed once in `registry::TOOLS`. Nothing outside this crate can call a
//! tool directly: the only way in is [`Registry::call`], the one guard every
//! tool call passes through. `rust/tests/architecture.rs` fails if a file here
//! isn't registered.
#![forbid(unsafe_code)]

mod registry;

// One line per tool, in the same order as `registry::TOOLS`:
// mod screenshot;

pub use registry::{CallContext, CallError, Content, Registry, ToolOutput, ToolSpec};
