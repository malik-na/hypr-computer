//! The control channel: one command per line on a unix socket, one JSON reply per line.
//!
//!   move X Y            pointer to X,Y (screen pixels)
//!   click [left|right|middle]
//!   down BTN / up BTN
//!   scroll DY [DX]      wheel steps, positive = down / right
//!   key COMBO           e.g. `key Return`, `key ctrl+l`, `key shift+Tab`
//!   type TEXT           US-ASCII text; `\n` is Enter, `\t` is Tab
//!   shot PATH|-         write a PNG (or `-`: capture only); replies timings and a pixel hash
//!   frame               copy the current frame into the shared frame file (used by hc-view)
//!   list / focus N / close N / pos / stats / quit
//!
//! Each connection gets a thread; commands are executed on the compositor's event loop.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use smithay::reexports::calloop::channel::Sender;

use crate::{H, W};

pub enum Cmd {
    Move(f64, f64),
    Button(u32, bool),
    Scroll(i32, i32),
    Key(u32, bool),
    Shot,
    Frame,
    List,
    Focus(usize),
    Close(usize),
    Pos,
    Stats,
    Quit,
}

pub enum Reply {
    Json(Value),
    Pixels { data: Vec<u8>, capture_us: u64 },
}

pub struct Req {
    pub cmd: Cmd,
    pub reply: mpsc::Sender<Reply>,
}

pub fn serve(listener: UnixListener, tx: Sender<Req>) {
    for stream in listener.incoming().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || client(stream, tx));
    }
}

fn client(stream: UnixStream, tx: Sender<Req>) {
    let Ok(mut out) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let reply = exec(&line, &tx);
        if writeln!(out, "{reply}").is_err() {
            break;
        }
    }
}

fn call(tx: &Sender<Req>, cmd: Cmd) -> Reply {
    let (rtx, rrx) = mpsc::channel();
    if tx.send(Req { cmd, reply: rtx }).is_err() {
        return Reply::Json(json!({ "ok": false, "error": "compositor gone" }));
    }
    rrx.recv()
        .unwrap_or_else(|_| Reply::Json(json!({ "ok": false, "error": "compositor gone" })))
}

fn simple(tx: &Sender<Req>, cmd: Cmd) -> Value {
    match call(tx, cmd) {
        Reply::Json(v) => v,
        Reply::Pixels { .. } => json!({ "ok": false, "error": "unexpected pixels" }),
    }
}

fn err(msg: &str) -> Value {
    json!({ "ok": false, "error": msg })
}

const PAUSE: Duration = Duration::from_millis(8);

fn exec(line: &str, tx: &Sender<Req>) -> Value {
    let line = line.trim_end_matches(['\r', '\n']);
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let nums: Vec<f64> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    match word {
        "move" if nums.len() == 2 => simple(tx, Cmd::Move(nums[0], nums[1])),
        "click" | "down" | "up" => {
            let button = match rest.trim() {
                "" | "left" => 0x110,
                "right" => 0x111,
                "middle" => 0x112,
                _ => return err("button: left|right|middle"),
            };
            match word {
                "down" => simple(tx, Cmd::Button(button, true)),
                "up" => simple(tx, Cmd::Button(button, false)),
                _ => {
                    simple(tx, Cmd::Button(button, true));
                    std::thread::sleep(PAUSE);
                    simple(tx, Cmd::Button(button, false))
                }
            }
        }
        "scroll" if !nums.is_empty() => {
            simple(tx, Cmd::Scroll(nums.get(1).copied().unwrap_or(0.0) as i32, nums[0] as i32))
        }
        "key" => {
            let mut codes = Vec::new();
            for part in rest.trim().split('+') {
                match key_by_name(part) {
                    Some(code) => codes.push(code),
                    None => return err("unknown key"),
                }
            }
            for &code in &codes {
                simple(tx, Cmd::Key(code, true));
                std::thread::sleep(Duration::from_millis(2));
            }
            for &code in codes.iter().rev() {
                simple(tx, Cmd::Key(code, false));
                std::thread::sleep(Duration::from_millis(2));
            }
            json!({ "ok": true })
        }
        "type" => {
            let text = rest.replace("\\n", "\n").replace("\\t", "\t");
            let mut skipped = 0;
            for ch in text.chars() {
                let Some((code, shift)) = key_for_char(ch) else {
                    skipped += 1;
                    continue;
                };
                if shift {
                    simple(tx, Cmd::Key(KEY_LEFTSHIFT, true));
                }
                simple(tx, Cmd::Key(code, true));
                simple(tx, Cmd::Key(code, false));
                if shift {
                    simple(tx, Cmd::Key(KEY_LEFTSHIFT, false));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            json!({ "ok": skipped == 0, "skipped": skipped })
        }
        "shot" => match call(tx, Cmd::Shot) {
            Reply::Json(v) => v,
            Reply::Pixels { data, capture_us } => {
                let hash = fast_hash(&data);
                let t = Instant::now();
                let path = rest.trim();
                if !path.is_empty() && path != "-" {
                    if let Err(e) = write_png(path, &data) {
                        return err(&format!("png: {e}"));
                    }
                }
                json!({
                    "ok": true,
                    "capture_ms": capture_us as f64 / 1000.0,
                    "encode_ms": t.elapsed().as_micros() as f64 / 1000.0,
                    "hash": format!("{hash:016x}"),
                })
            }
        },
        "frame" => simple(tx, Cmd::Frame),
        "list" => simple(tx, Cmd::List),
        "focus" if nums.len() == 1 => simple(tx, Cmd::Focus(nums[0] as usize)),
        "close" if nums.len() == 1 => simple(tx, Cmd::Close(nums[0] as usize)),
        "pos" => simple(tx, Cmd::Pos),
        "stats" => simple(tx, Cmd::Stats),
        "quit" => simple(tx, Cmd::Quit),
        _ => err("unknown command"),
    }
}

fn write_png(path: &str, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut rgb = Vec::with_capacity((W * H * 3) as usize);
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, W as u32, H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn fast_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for chunk in data.chunks_exact(8) {
        let v = u64::from_le_bytes(chunk.try_into().unwrap());
        h = (h ^ v).wrapping_mul(0x100000001b3).rotate_left(23);
    }
    h
}

// --- keys: evdev codes for a US layout -------------------------------------------------------

const KEY_LEFTSHIFT: u32 = 42;

fn key_by_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "ctrl" | "control" => 29,
        "shift" => KEY_LEFTSHIFT,
        "alt" => 56,
        "super" | "meta" => 125,
        "return" | "enter" => 28,
        "tab" => 15,
        "escape" | "esc" => 1,
        "backspace" => 14,
        "delete" => 111,
        "space" => 57,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "f5" => 63,
        "f11" => 87,
        "f12" => 88,
        _ => {
            let mut chars = name.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
            key_for_char(ch)?.0
        }
    })
}

fn key_for_char(ch: char) -> Option<(u32, bool)> {
    const ROWS: [(&str, &str, u32); 4] = [
        ("`1234567890-=", "~!@#$%^&*()_+", 0),
        ("qwertyuiop[]", "QWERTYUIOP{}", 16),
        ("asdfghjkl;'", "ASDFGHJKL:\"", 30),
        ("zxcvbnm,./", "ZXCVBNM<>?", 44),
    ];
    match ch {
        '\n' => return Some((28, false)),
        '\t' => return Some((15, false)),
        ' ' => return Some((57, false)),
        '\\' => return Some((43, false)),
        '|' => return Some((43, true)),
        _ => {}
    }
    for (plain, shifted, base) in ROWS {
        for (set, shift) in [(plain, false), (shifted, true)] {
            if let Some(i) = set.chars().position(|c| c == ch) {
                // The top row starts at grave (41), then 1..= are 2..13.
                let code = if base == 0 { if i == 0 { 41 } else { i as u32 + 1 } } else { base + i as u32 };
                return Some((code, shift));
            }
        }
    }
    None
}
