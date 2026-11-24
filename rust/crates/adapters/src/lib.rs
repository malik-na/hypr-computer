//! Real implementations of the traits in `ports`: the system clock now; the
//! screen, notifications and the host compositor in later changes.
#![forbid(unsafe_code)]

mod clock;

pub use clock::SystemClock;
