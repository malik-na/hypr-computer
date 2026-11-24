//! hc-test-app: a tiny Wayland app for the black-box tests to put on an
//! agent's screen. It opens one window, paints its top half one colour and its
//! bottom half another, so a screenshot shows whose app it is and which way
//! up, and writes what happens to it to the file given with `--log`, one JSON
//! object per line.
//!
//! `hc-test-app [--top RRGGBB] [--bottom RRGGBB] [--log PATH]`
#![forbid(unsafe_code)]

mod log;
mod pointer;
mod seat;
mod window;

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::{self, ExitCode};

use serde_json::{Value, json};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, QueueHandle};

use crate::log::Log;
use crate::pointer::Pointer;
use crate::window::Window;

/// Everything the app holds; the Wayland events of every module land here.
pub struct App {
    pub log: Log,
    pub window: Window,
    /// The colours to paint, top and bottom.
    pub colours: ([u8; 3], [u8; 3]),
    /// Something changed what the window should show.
    pub dirty: bool,
    /// How many times input has come.
    inputs: u32,
    /// What the screen's seat offers.
    pub seat: Vec<&'static str>,
    pub pointer: Pointer,
}

impl App {
    /// Logs `event`. A log that can't be written ends the app, so a test
    /// sees the failure instead of missing events.
    pub fn note(&mut self, event: &Value) {
        if let Err(e) = self.log.write(event) {
            eprintln!("hc-test-app: can't write the log: {e}");
            process::exit(1);
        }
    }

    /// Input came: the window turns a new colour.
    pub fn input_seen(&mut self) {
        self.inputs += 1;
        let n = self.inputs;
        let shade = |a: u32, b: u32| ((n * a + b) % 256) as u8;
        let colour = [shade(67, 30), shade(131, 60), shade(29, 90)];
        self.colours = (colour, colour);
        self.dirty = true;
    }

    fn paint(&mut self, qh: &QueueHandle<App>) -> Result<(), Box<dyn Error>> {
        self.dirty = false;
        let (top, bottom) = self.colours;
        self.window.paint(qh, top, bottom)?;
        Ok(())
    }
}

struct Args {
    top: [u8; 3],
    bottom: [u8; 3],
    log: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hc-test-app: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&connection)?;
    let qh = queue.handle();
    let mut app = App {
        log: Log::open(args.log.as_deref())?,
        window: Window::new(&globals, &qh)?,
        colours: (args.top, args.bottom),
        dirty: true,
        inputs: 0,
        seat: Vec::new(),
        pointer: Pointer::default(),
    };
    // Optional: with no seat, the app just shows its colours.
    let _seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 5..=9, ()).ok();
    while !app.window.configured() {
        queue.blocking_dispatch(&mut app)?;
    }
    app.paint(&qh)?;
    // Once the screen answers this, it has the picture.
    queue.roundtrip(&mut app)?;
    app.note(&json!({
        "event": "ready",
        "pid": process::id(),
        "display": env::var("DISPLAY").ok(),
        "seat": app.seat,
    }));
    loop {
        queue.blocking_dispatch(&mut app)?;
        if app.dirty {
            app.paint(&qh)?;
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        top: [0x80; 3],
        bottom: [0x40; 3],
        log: None,
    };
    let mut given = env::args().skip(1);
    while let Some(flag) = given.next() {
        let value = given
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--top" => args.top = colour(&value)?,
            "--bottom" => args.bottom = colour(&value)?,
            "--log" => args.log = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown argument {flag:?}")),
        }
    }
    Ok(args)
}

/// `RRGGBB` as bytes.
fn colour(hex: &str) -> Result<[u8; 3], String> {
    let byte = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|b| u8::from_str_radix(b, 16).ok())
            .ok_or_else(|| format!("not a colour: {hex:?}; use RRGGBB"))
    };
    if hex.len() != 6 {
        return Err(format!("not a colour: {hex:?}; use RRGGBB"));
    }
    Ok([byte(0)?, byte(2)?, byte(4)?])
}
