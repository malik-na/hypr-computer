//! An agent's screen.
//!
//! Two halves of one thing: `hypr-computer-screen`, the compositor process
//! (in `src/compositor/`), and [`ScreenProcesses`], the daemon's handle on it,
//! which implements the `ports` screen traits. They talk over the process's
//! stdin and stdout, in the format in [`protocol`].
#![forbid(unsafe_code)]

mod apps;
mod browser;
mod devtools;
mod launch;
mod process;
pub mod protocol;
mod settle;
mod strays;
mod user;

pub use apps::start_time;
pub use process::ScreenProcesses;

/// Every screen's size in pixels, width by height.
pub const SIZE: (u32, u32) = (1280, 800);

/// Why `point` isn't on the screen, if it isn't.
pub fn off_screen(point: ports::Point) -> Option<String> {
    let (width, height) = SIZE;
    (point.x >= width || point.y >= height).then(|| {
        format!(
            "({}, {}) is off your screen, which is {width}x{height}: x is 0 to {}, y is 0 to {}",
            point.x,
            point.y,
            width - 1,
            height - 1
        )
    })
}
