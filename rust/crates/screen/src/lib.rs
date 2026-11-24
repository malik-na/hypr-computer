//! An agent's screen.
//!
//! Two halves of one thing: `hypr-computer-screen`, the compositor process
//! (in `src/compositor/`), and [`ScreenProcesses`], the daemon's handle on it,
//! which implements the `ports` screen traits. They talk over the process's
//! stdin and stdout, in the format in [`protocol`].
#![forbid(unsafe_code)]

mod launch;
mod process;
pub mod protocol;

pub use process::ScreenProcesses;

/// Every screen's size in pixels, width by height.
pub const SIZE: (u32, u32) = (1280, 800);
