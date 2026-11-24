//! The user taking over the screen from its live view (specs/TakeOver.tla).
//!
//! The view takes the desktop's seat: its pointer and its keyboard. Each
//! input the user gives in the view is reported to the daemon on the user
//! socket, and given to the screen only if the daemon answers `forward`, so
//! the daemon's order of events decides, not when a notice arrives here.
//!
//! Between `taken` and `given` the user has the screen: the banner shows,
//! and a request of the daemon's that comes meanwhile, but opening the view,
//! waits for the hand-back: it is the call that waited, and the user's input
//! forwarded before `given` must land, and reach the apps, before it runs.

use std::collections::VecDeque;
use std::error::Error;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use ports::{Point, UserInput, ViewNotice};
use screen::protocol::{Request, input_line, parse_notice};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::utils::SERIAL_COUNTER;
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::requests::{self, Incoming};
use crate::state::{HEIGHT, State, WIDTH};

/// An input of the user's, as given to the screen.
#[derive(Clone, Copy, Debug)]
enum Raw {
    Motion(Point),
    Button { code: u32, pressed: bool, at: Point },
    Axis { axis: Axis, value: f64, at: Point },
    Key { code: u32, pressed: bool },
}

impl Raw {
    fn kind(self) -> UserInput {
        match self {
            Raw::Button { pressed: true, .. } => UserInput::Click,
            Raw::Key { pressed: true, .. } => UserInput::Key { escape: false },
            _ => UserInput::Other,
        }
    }
}

/// The screen's side of taking over.
#[derive(Default)]
pub struct User {
    /// The daemon's end of the user socket, once it connected.
    link: Option<UnixStream>,
    /// What came on it and isn't a whole line yet.
    partial: Vec<u8>,
    /// Inputs reported and not yet answered, oldest first.
    undecided: VecDeque<Raw>,
    /// The user has the screen.
    pub has_screen: bool,
    /// The buttons and keys the user holds down on the screen.
    held_buttons: Vec<u32>,
    held_keys: Vec<u32>,
    /// The daemon's requests waiting for the hand-back.
    deferred: VecDeque<Incoming>,
    /// Where the user's pointer is on the screen.
    at: Point,
}

/// The view's hold on the desktop's seat.
#[derive(Default)]
pub struct ViewSeat {
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// The pointer is over the screen's picture, not the black around it.
    on_picture: bool,
}

impl ViewSeat {
    pub fn new(seat: Option<wl_seat::WlSeat>) -> Self {
        Self {
            seat,
            ..Self::default()
        }
    }
}

/// Listens on `path` for the daemon. It connects once; the file goes then.
pub fn listen(handle: &LoopHandle<'static, State>, path: &Path) -> Result<(), Box<dyn Error>> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let listener =
        UnixListener::bind(path).map_err(|e| format!("can't listen on {}: {e}", path.display()))?;
    let path = path.to_owned();
    handle.insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        move |_, listener, state| match listener.accept() {
            Ok((stream, _)) => {
                let _ = std::fs::remove_file(&path);
                state.link_user(stream);
                Ok(PostAction::Remove)
            }
            Err(e) => {
                eprintln!("hypr-computer-screen: accept on the user socket failed: {e}");
                Ok(PostAction::Continue)
            }
        },
    )?;
    Ok(())
}

impl State {
    fn link_user(&mut self, stream: UnixStream) {
        let reader = match stream.try_clone() {
            Ok(reader) => reader,
            Err(e) => {
                eprintln!("hypr-computer-screen: the user socket broke: {e}");
                return;
            }
        };
        let added = self.loop_handle.insert_source(
            Generic::new(reader, Interest::READ, Mode::Level),
            |_, reader, state| {
                let mut buf = [0; 256];
                match (&**reader).read(&mut buf) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        Ok(PostAction::Continue)
                    }
                    Ok(0) | Err(_) => {
                        state.unlink_user();
                        Ok(PostAction::Remove)
                    }
                    Ok(n) => {
                        state.user.partial.extend_from_slice(&buf[..n]);
                        state.read_notices();
                        Ok(PostAction::Continue)
                    }
                }
            },
        );
        match added {
            Ok(_) => self.user.link = Some(stream),
            Err(e) => eprintln!("hypr-computer-screen: can't read the user socket: {e}"),
        }
    }

    /// The daemon is gone: the view is watch-only from now on.
    fn unlink_user(&mut self) {
        self.user.link = None;
        self.user.undecided.clear();
        self.notice(ViewNotice::Given);
    }

    fn read_notices(&mut self) {
        while let Some(end) = self.user.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.user.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]).into_owned();
            match parse_notice(&line) {
                Some(notice) => self.notice(notice),
                None => eprintln!("hypr-computer-screen: unknown notice {line:?}"),
            }
        }
    }

    fn notice(&mut self, notice: ViewNotice) {
        match notice {
            ViewNotice::Forward => {
                if let Some(raw) = self.user.undecided.pop_front() {
                    self.give_user(raw);
                }
            }
            ViewNotice::Drop => {
                self.user.undecided.pop_front();
            }
            ViewNotice::TakenOver => {
                self.user.has_screen = true;
                self.show_banner(true);
            }
            ViewNotice::Given => {
                self.user.has_screen = false;
                self.release_user();
                self.show_banner(false);
                self.answer_deferred();
            }
        }
    }

    /// The daemon's request, answered now, or once the user hands back.
    /// Opening the view never waits: the user may have closed it. (The
    /// daemon asks one thing at a time, so it comes with nothing deferred.)
    pub fn request(&mut self, request: Incoming) {
        let view = matches!(request, Ok(Request::View(_)));
        if !view && (self.user.has_screen || !self.user.deferred.is_empty()) {
            self.user.deferred.push_back(request);
            return;
        }
        self.answer(request);
    }

    fn answer(&mut self, request: Incoming) {
        if requests::answer(self, request, &mut std::io::stdout().lock()).is_err() {
            self.stop();
        }
    }

    fn answer_deferred(&mut self) {
        if !self.user.deferred.is_empty() {
            // The apps get the user's last input before the call goes on.
            let _ = self.display.flush_clients();
        }
        while !self.user.has_screen
            && let Some(request) = self.user.deferred.pop_front()
        {
            self.answer(request);
        }
    }

    /// The user gave `raw` in the view: asks the daemon.
    fn user_gave(&mut self, raw: Raw) {
        let kind = raw.kind();
        let Some(link) = &mut self.user.link else {
            return;
        };
        if link.write_all(input_line(kind).as_bytes()).is_err() {
            self.unlink_user();
            return;
        }
        self.user.undecided.push_back(raw);
    }

    /// Gives the screen an input the daemon let through. A release only if
    /// it gave the press.
    fn give_user(&mut self, raw: Raw) {
        let time = self.time();
        match raw {
            Raw::Motion(at) => {
                let _ = self.move_pointer(at);
            }
            Raw::Button { code, pressed, at } => {
                let _ = self.move_pointer(at);
                if pressed {
                    self.user.held_buttons.push(code);
                } else if !take(&mut self.user.held_buttons, code) {
                    return;
                }
                self.user_button(code, pressed);
            }
            Raw::Axis { axis, value, at } => {
                let _ = self.move_pointer(at);
                if let Some(pointer) = self.seat.get_pointer() {
                    let frame = AxisFrame::new(time)
                        .source(AxisSource::Wheel)
                        .value(axis, value);
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            Raw::Key { code, pressed } => {
                if pressed {
                    self.user.held_keys.push(code);
                } else if !take(&mut self.user.held_keys, code) {
                    return;
                }
                self.user_key(code, pressed);
            }
        }
    }

    fn user_button(&mut self, code: u32, pressed: bool) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let event = ButtonEvent {
            button: code,
            state,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.time(),
        };
        pointer.button(self, &event);
        pointer.frame(self);
    }

    fn user_key(&mut self, code: u32, pressed: bool) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        let time = self.time();
        keyboard.input::<(), _>(
            self,
            (code + 8).into(),
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |_, _, _| FilterResult::Forward,
        );
    }

    /// Lets go of everything the user holds down on the screen.
    fn release_user(&mut self) {
        for code in std::mem::take(&mut self.user.held_buttons) {
            self.user_button(code, false);
        }
        for code in std::mem::take(&mut self.user.held_keys) {
            self.user_key(code, false);
        }
    }

    /// The view lost the desktop's keyboard or pointer: the user lets go of
    /// what they held there, so the screen gets its release too.
    fn user_left(&mut self, keys: bool) {
        let held = if keys {
            self.user.held_keys.clone()
        } else {
            self.user.held_buttons.clone()
        };
        let at = self.user.at;
        for code in held {
            self.user_gave(if keys {
                Raw::Key {
                    code,
                    pressed: false,
                }
            } else {
                Raw::Button {
                    code,
                    pressed: false,
                    at,
                }
            });
        }
    }

    /// Where the view's point `(x, y)`, on the surface the pointer is on, is
    /// on the screen.
    fn on_screen(&self, x: f64, y: f64) -> Point {
        let Some(view) = &self.view else {
            return self.user.at;
        };
        let ((left, top), (width, height)) = view.fit;
        let (x, y) = if view.seat.on_picture {
            (x, y)
        } else {
            (x - f64::from(left), y - f64::from(top))
        };
        let scale = |v: f64, size: i32, of: i32| {
            let v = v * f64::from(of) / f64::from(size.max(1));
            v.clamp(0.0, f64::from(of - 1)) as u32
        };
        Point {
            x: scale(x, width, WIDTH),
            y: scale(y, height, HEIGHT),
        }
    }

    fn pointer_at(&mut self, x: f64, y: f64) {
        self.user.at = self.on_screen(x, y);
        self.user_gave(Raw::Motion(self.user.at));
    }
}

/// Removes one `code` from `held`; whether it was there.
fn take(held: &mut Vec<u32>, code: u32) -> bool {
    match held.iter().position(|c| *c == code) {
        Some(i) => {
            held.remove(i);
            true
        }
        None => false,
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        let Some(view) = &mut state.view else { return };
        let ours = &mut view.seat;
        if ours.seat.as_ref() != Some(seat) {
            return;
        }
        if caps.contains(wl_seat::Capability::Pointer) && ours.pointer.is_none() {
            ours.pointer = Some(seat.get_pointer(qh, ()));
        }
        if caps.contains(wl_seat::Capability::Keyboard) && ours.keyboard.is_none() {
            ours.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                let Some(view) = &mut state.view else { return };
                view.seat.on_picture = view.is_picture(&surface);
                state.pointer_at(surface_x, surface_y);
            }
            wl_pointer::Event::Leave { .. } => state.user_left(false),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.pointer_at(surface_x, surface_y),
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(pressed),
                ..
            } => {
                let at = state.user.at;
                state.user_gave(Raw::Button {
                    code: button,
                    pressed: pressed == wl_pointer::ButtonState::Pressed,
                    at,
                });
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                let axis = match axis {
                    wl_pointer::Axis::HorizontalScroll => Axis::Horizontal,
                    _ => Axis::Vertical,
                };
                let at = state.user.at;
                state.user_gave(Raw::Axis { axis, value, at });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(pressed),
                ..
            } => state.user_gave(Raw::Key {
                code: key,
                pressed: pressed == wl_keyboard::KeyState::Pressed,
            }),
            wl_keyboard::Event::Leave { .. } => state.user_left(true),
            _ => {}
        }
    }
}
