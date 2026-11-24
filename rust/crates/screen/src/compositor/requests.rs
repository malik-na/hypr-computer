//! The daemon's requests: read from stdin, answered on stdout, in the format
//! in `screen::protocol`.

use std::io::{self, BufRead, Write};

use screen::protocol::{Reply, Request};
use smithay::reexports::calloop::channel::Sender;

use crate::state::State;

/// A request, or the line that wasn't one.
pub type Incoming = Result<Request, String>;

/// Passes each line of `input` to the event loop until it ends.
pub fn read(input: impl BufRead, to_loop: &Sender<Incoming>) {
    for line in input.lines() {
        let Ok(line) = line else { return };
        let request = Request::parse(&line).ok_or(line);
        if to_loop.send(request).is_err() {
            return;
        }
    }
}

/// Answers one request. `Err` only if the answer can't be written: the
/// daemon is gone.
pub fn answer(state: &mut State, request: Incoming, out: &mut impl Write) -> io::Result<()> {
    let reply = match request {
        Ok(Request::Screenshot) => state.screenshot(),
        Err(line) => Err(format!("unknown request {line:?}")),
    };
    match reply {
        Ok(png) => {
            out.write_all(Reply::Png(png.len()).line().as_bytes())?;
            out.write_all(&png)?;
        }
        Err(why) => out.write_all(Reply::Error(why).line().as_bytes())?,
    }
    out.flush()
}
