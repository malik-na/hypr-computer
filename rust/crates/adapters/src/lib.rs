//! Real implementations of the traits in `ports`: the system clock and the
//! user's desktop; notifications in a later change.
#![forbid(unsafe_code)]

mod clock;
mod desktop;
mod hyprland;

pub use clock::SystemClock;
pub use desktop::desktop;
