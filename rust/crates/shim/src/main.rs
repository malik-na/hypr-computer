//! hypr-computer's shim: the program an agent starts as its MCP server.
//!
//! It joins its stdin and stdout to the daemon's socket and does nothing
//! else; the daemon speaks MCP. It exits as soon as either side closes, so
//! the daemon sees the agent leave at once, however the agent died.
#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

use domain::paths;

/// Which side closed first.
enum Closed {
    /// The agent closed our stdin (or stopped reading our stdout).
    Agent,
    /// The daemon closed the connection.
    Daemon,
}

fn main() -> ExitCode {
    let socket = match paths::daemon_socket(std::env::var_os("XDG_RUNTIME_DIR").as_deref()) {
        Ok(p) => p,
        Err(e) => return fail(&e.to_string()),
    };
    let stream = match UnixStream::connect(&socket) {
        Ok(s) => s,
        Err(e) => {
            return fail(&format!(
                "can't reach the daemon at {} ({e}); start it with `hypr-computer-daemon &`, then restart your agent",
                socket.display()
            ));
        }
    };
    let (to_agent, to_daemon) = match stream.try_clone() {
        Ok(clone) => (stream, clone),
        Err(e) => return fail(&format!("can't use the daemon's socket: {e}")),
    };

    let (done, closed) = mpsc::channel();
    let done_up = done.clone();
    thread::spawn(move || {
        let _ = pump(io::stdin().lock(), &to_daemon);
        // Report before anything closes the socket, so the other thread can't
        // mistake our own close for the daemon's. Exiting closes the socket.
        let _ = done_up.send(Closed::Agent);
    });
    thread::spawn(move || {
        let who = match pump(&to_agent, io::stdout().lock()) {
            Ok(()) => Closed::Daemon,
            Err(Broke::Writing) => Closed::Agent,
            Err(Broke::Reading) => Closed::Daemon,
        };
        let _ = done.send(who);
    });

    // Exit on the first side to close; the other thread dies with the process.
    match closed.recv() {
        Ok(Closed::Agent) => ExitCode::SUCCESS,
        Ok(Closed::Daemon) | Err(_) => fail("the daemon closed the connection"),
    }
}

enum Broke {
    Reading,
    Writing,
}

/// Copies `from` to `to` until `from` ends, flushing every chunk so no
/// message waits in a buffer.
fn pump(mut from: impl Read, mut to: impl Write) -> Result<(), Broke> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Broke::Reading),
        };
        let chunk = buf.get(..n).ok_or(Broke::Reading)?;
        to.write_all(chunk).map_err(|_| Broke::Writing)?;
        to.flush().map_err(|_| Broke::Writing)?;
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("hypr-computer: {message}");
    ExitCode::FAILURE
}
