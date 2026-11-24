//! Real implementations of the traits in `ports`: the system clock and the
//! user's desktop, and its notification service.
#![forbid(unsafe_code)]

mod clock;
mod desktop;
mod hyprland;
mod notifications;

pub use clock::SystemClock;
pub use desktop::desktop;
pub use notifications::{notifications, session_bus};
