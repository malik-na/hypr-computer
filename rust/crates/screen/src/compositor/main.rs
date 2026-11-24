//! hypr-computer-screen: one agent's screen.
//!
//! A headless Wayland compositor built on Smithay, rendering in software. The
//! agent's apps connect to the Wayland socket given as the first argument;
//! the daemon reads the screen and drives its pointer and keyboard over stdin
//! and stdout (see `screen::protocol`). There is no input device, so nothing
//! of the user's reaches it. Its only window on the user's desktop, whose
//! Wayland socket is the second argument, is its live view, opened when the
//! daemon asks. It exits when its stdin closes.
#![forbid(unsafe_code)]

mod activation;
mod capture;
mod clipboard;
mod handlers;
mod keyboard;
mod keymap;
mod pointer;
mod reading;
mod requests;
mod seat;
mod state;
mod view;
mod windows;

use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use screen::protocol::READY;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction, channel};
use smithay::reexports::wayland_server::{Display, ListeningSocket};

use crate::state::{ClientState, State};

/// How often apps are told to draw their next frame, at most.
const FRAME: Duration = Duration::from_millis(16);

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(socket), desktop, None) = (args.next(), args.next(), args.next()) else {
        eprintln!(
            "usage: hypr-computer-screen <path of the Wayland socket to create> \
             [<path of the user's desktop's Wayland socket>]"
        );
        return ExitCode::from(2);
    };
    match run(PathBuf::from(socket), desktop.map(PathBuf::from)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hypr-computer-screen: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(socket: PathBuf, desktop: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    if !socket.is_absolute() {
        return Err(format!("the socket path must be absolute, not {}", socket.display()).into());
    }
    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let mut display: Display<State> = Display::new()?;
    let handle = event_loop.handle();
    let mut state = State::new(
        display.handle(),
        event_loop.get_signal(),
        handle.clone(),
        desktop,
    )?;

    // Apps connecting.
    let listening = ListeningSocket::bind_absolute(socket.clone())
        .map_err(|e| format!("can't listen on {}: {e}", socket.display()))?;
    handle.insert_source(
        Generic::new(listening, Interest::READ, Mode::Level),
        |_, listening, state| {
            while let Some(stream) = listening.accept()? {
                state
                    .display
                    .insert_client(stream, Arc::new(ClientState::default()))?;
            }
            Ok(PostAction::Continue)
        },
    )?;

    // Apps' requests. The display is owned by this callback, so dispatching
    // to the state needs no shared borrow.
    let requests = display.backend().poll_fd().try_clone_to_owned()?;
    handle.insert_source(
        Generic::new(requests, Interest::READ, Mode::Level),
        move |_, _, state| {
            display.dispatch_clients(state)?;
            Ok(PostAction::Continue)
        },
    )?;

    // The daemon's requests, read on a thread of their own. When stdin
    // closes, the reader ends, the channel closes, and so does the screen.
    let (to_loop, from_stdin) = channel::channel();
    thread::spawn(move || requests::read(io::stdin().lock(), &to_loop));
    handle.insert_source(from_stdin, |event, _, state| match event {
        channel::Event::Msg(request) => {
            if requests::answer(state, request, &mut io::stdout().lock()).is_err() {
                state.stop();
            }
        }
        channel::Event::Closed => state.stop(),
    })?;

    handle.insert_source(Timer::from_duration(FRAME), |_, _, state| {
        state.frame_tick();
        TimeoutAction::ToDuration(FRAME)
    })?;

    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{READY}")?;
    stdout.flush()?;
    drop(stdout);

    event_loop.run(None, &mut state, |state| {
        let _ = state.display.flush_clients();
        state.flush_view();
    })?;
    Ok(())
}
