//! Reading a tool's arguments. Not a tool: the helpers the tools share. A
//! wrong argument is a [`CallError::InvalidArguments`] that says what to send.

use serde_json::{Map, Value};

use crate::registry::CallError;

fn invalid(why: String) -> CallError {
    CallError::InvalidArguments(why)
}

/// A required string.
pub fn string(args: &Map<String, Value>, name: &str) -> Result<String, CallError> {
    match args.get(name) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(other) => Err(invalid(format!(
            "`{name}` must be a non-empty string, not {other}"
        ))),
        None => Err(invalid(format!("`{name}` is required"))),
    }
}

/// An optional list of strings; none given is an empty list.
pub fn strings(args: &Map<String, Value>, name: &str) -> Result<Vec<String>, CallError> {
    let wrong = || invalid(format!("`{name}` must be a list of strings"));
    match args.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(wrong))
            .collect(),
        Some(_) => Err(wrong()),
    }
}
