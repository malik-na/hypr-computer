//! What hypr-computer needs from the outside world, as traits.
//!
//! The daemon and the tools depend on these traits; `adapters` implements
//! them for real, and tests implement them with fakes. Add a trait here only
//! when code in this workspace uses it.
#![forbid(unsafe_code)]

use std::time::Instant;

/// Tells the time. Session lengths and, later, timeouts read the time only
/// through this, so tests can control it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}
