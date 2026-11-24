//! Reading a tool's arguments. Not a tool: the helpers the tools share. A
//! wrong argument is a [`CallError::InvalidArguments`] that says what to send.

use ports::Point;
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

/// A required coordinate: a whole number of pixels, 0 or more.
pub fn coordinate(args: &Map<String, Value>, name: &str) -> Result<u32, CallError> {
    let value = args
        .get(name)
        .ok_or_else(|| invalid(format!("`{name}` is required")))?;
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= f64::from(u32::MAX))
        .map(|n| n.round() as u32)
        .ok_or_else(|| {
            invalid(format!(
                "`{name}` must be a number of pixels, 0 or more, not {value}"
            ))
        })
}

/// The point at the coordinates `x` and `y` name.
pub fn point(args: &Map<String, Value>, x: &str, y: &str) -> Result<Point, CallError> {
    Ok(Point {
        x: coordinate(args, x)?,
        y: coordinate(args, y)?,
    })
}

/// An optional whole number from `min` to `max`.
pub fn count(
    args: &Map<String, Value>,
    name: &str,
    default: u32,
    (min, max): (u32, u32),
) -> Result<u32, CallError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| {
                invalid(format!(
                    "`{name}` must be a whole number from {min} to {max}, not {value}"
                ))
            }),
    }
}

/// One of `choices`, or `default` if not given.
pub fn choice<'a>(
    args: &Map<String, Value>,
    name: &str,
    default: &'a str,
    choices: &[&'a str],
) -> Result<&'a str, CallError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => choices
            .iter()
            .find(|c| value.as_str() == Some(**c))
            .copied()
            .ok_or_else(|| {
                invalid(format!(
                    "`{name}` must be one of {}, not {value}",
                    choices.join(", ")
                ))
            }),
    }
}
