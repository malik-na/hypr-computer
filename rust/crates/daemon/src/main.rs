//! hypr-computer's daemon. One runs per login session; every connection to
//! its socket is one agent session speaking MCP. See docs/design.md.
#![forbid(unsafe_code)]

mod listen;
mod mcp;
mod owner;
mod serve;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;

use adapters::SystemClock;
use listenfd::ListenFd;
use tracing::{Level, info};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::prelude::*;

fn main() -> ExitCode {
    // Read (and clear) systemd's LISTEN_* variables while this is still the
    // only thread: clearing the environment isn't safe once others run.
    let mut fds = ListenFd::from_env();

    if let Some(arg) = std::env::args().nth(1) {
        eprintln!("hypr-computer-daemon: unexpected argument {arg:?}; it takes none");
        return ExitCode::from(2);
    }

    let filter = Targets::new()
        .with_default(Level::INFO)
        .with_target("rmcp", Level::WARN);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal())
                .with_target(false),
        )
        .with(filter)
        .init();

    let listening = match listen::listen(&mut fds, std::env::var_os("XDG_RUNTIME_DIR").as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!("{}", listening.describe());

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("hypr-computer-daemon: can't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve::run(listening, Arc::new(SystemClock))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}
