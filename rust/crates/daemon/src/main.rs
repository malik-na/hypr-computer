//! hypr-computer's daemon. One runs per login session; every connection to
//! its socket is one agent session speaking MCP. See docs/design.md.
#![forbid(unsafe_code)]

mod config;
mod control;
mod listen;
mod log;
mod mcp;
mod owner;
mod serve;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use adapters::SystemClock;
use config::Settings;
use domain::paths;
use listenfd::ListenFd;
use log::ActionLog;
use screen::ScreenProcesses;
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

    // rmcp's warnings quote requests and error replies, which hold what
    // agents type and the URLs they open: the journal mustn't.
    let filter = Targets::new()
        .with_default(Level::INFO)
        .with_target("rmcp", Level::ERROR);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal())
                .with_target(false),
        )
        .with(filter)
        .init();

    let program = match screen_program() {
        Ok(program) => program,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    let log = match action_log() {
        Ok(log) => Arc::new(log),
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
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
    // Before the socket exists: from then on, anyone can signal us.
    let _in_runtime = runtime.enter();
    let stop = match serve::watch_signals() {
        Ok(stop) => stop,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    let listening = match listen::listen(&mut fds, runtime_dir.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!("{}", listening.describe());
    let desktop = paths::desktop_socket(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        runtime_dir.as_deref(),
    );
    let screens = Arc::new(ScreenProcesses::new(
        program,
        listening.dir.clone(),
        desktop.clone(),
        std::env::var_os("HYPR_COMPUTER_BROWSER")
            .filter(|b| !b.is_empty())
            .map(PathBuf::from),
    ));
    let desktop = adapters::desktop(
        desktop.as_deref(),
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
    );

    match runtime.block_on(serve::run(
        listening,
        Arc::new(SystemClock),
        screens,
        stop,
        desktop,
        log,
    )) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hypr-computer-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The action log, as the user's settings say. Every action is logged, so
/// without one the daemon doesn't start.
fn action_log() -> Result<ActionLog, String> {
    let home = std::env::var_os("HOME");
    let settings = paths::config_file(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        home.as_deref(),
    )
    .map(|file| Settings::read(&file))
    .unwrap_or_default();
    let state = paths::state_dir(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
    )
    .ok_or("there is nowhere to keep the action log: neither HOME nor XDG_STATE_HOME is set")?;
    ActionLog::open(state.clone(), &settings)
        .map_err(|e| format!("can't make the action log in {}: {e}", state.display()))
}

/// The screen program: `$HYPR_COMPUTER_SCREEN` if set, otherwise
/// `hypr-computer-screen` next to this program. Without one, every agent
/// would connect only to be dropped, so the daemon doesn't start.
fn screen_program() -> Result<PathBuf, String> {
    let program = match std::env::var_os("HYPR_COMPUTER_SCREEN") {
        Some(program) => PathBuf::from(program),
        None => std::env::current_exe()
            .map_err(|e| format!("can't find my own program: {e}"))?
            .with_file_name("hypr-computer-screen"),
    };
    if !program.is_file() {
        return Err(format!(
            "there is no screen program at {}; install hypr-computer-screen next to \
             hypr-computer-daemon, or set HYPR_COMPUTER_SCREEN to its path",
            program.display()
        ));
    }
    Ok(program)
}
