//! hypr-computer's state and the rules that change it.
//!
//! Pure: no I/O, no async, no clock reads, no dependency on another workspace
//! crate. Everything the rules need from the outside world comes in as an
//! argument. `rust/tests/architecture.rs` enforces this.
#![forbid(unsafe_code)]

pub mod lifecycle;
pub mod paths;
pub mod session;

pub use session::{EndReason, Gone, SessionId, Sessions, Stepped};
// probe
