//! The probe: a tiny Wayland app that fills the agent's screen, records every
//! pointer and keyboard event it receives, and repaints after each one, so a
//! test can see both what arrived and that the screen changed.
//!
//! Each repaint fills the window with a new background colour and draws a
//! `MARK`-coloured square wherever a button was pressed.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use xkbcommon::xkb;

use super::{TIMEOUT, wait_until};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// The colour of a click mark; no background is ever this colour.
pub const MARK: [u8; 3] = [0xff, 0xff, 0x00];
/// Half the side of a click mark.
const MARK_RADIUS: i32 = 4;

/// The background colour of the `n`th picture; consecutive ones differ.
pub fn background(n: u64) -> [u8; 3] {
    let c = |k: u64| (40 + (n * k) % 150) as u8;
    [c(53), c(97), c(29)]
}

/// Modifiers in effect when a key went down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

/// What the probe received, in order. Positions are in the window, which
/// fills the screen, so they are screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    PointerEnter {
        x: f64,
        y: f64,
    },
    PointerLeave,
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        pressed: bool,
        x: f64,
        y: f64,
        time: u32,
    },
    Axis {
        horizontal: bool,
        value: f64,
        x: f64,
        y: f64,
    },
    /// A wheel movement in 1/120ths of a notch (`axis_discrete` × 120 or
    /// `axis_value120`).
    Wheel {
        horizontal: bool,
        value120: i32,
    },
    KeyboardEnter,
    KeyboardLeave,
    Key {
        code: u32,
        sym: u32,
        name: String,
        utf8: String,
        pressed: bool,
        mods: Mods,
    },
    /// Text committed through text-input-v3, if the screen offers it.
    Text(String),
    /// The connection to the screen broke.
    Disconnected(String),
}

/// The probe; it stays on the screen until dropped.
pub struct Probe {
    log: Arc<Mutex<Vec<Event>>>,
    stop: Arc<AtomicBool>,
    stream: UnixStream,
    thread: Option<JoinHandle<()>>,
    pub width: i32,
    pub height: i32,
}

impl Probe {
    /// Opens a window on the screen at `socket`, paints it, and returns once
    /// the screen has the picture.
    pub fn start(socket: &Path) -> Probe {
        let stream = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("can't connect to {}: {e}", socket.display()));
        let ours = stream.try_clone().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let thread = {
            let (log, stop) = (log.clone(), stop.clone());
            thread::spawn(move || run(stream, log, stop, tx))
        };
        let (width, height) = rx
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|e| panic!("the probe never got on the screen: {e}"));
        Probe {
            log,
            stop,
            stream: ours,
            thread: Some(thread),
            width,
            height,
        }
    }

    pub fn events(&self) -> Vec<Event> {
        self.log.lock().unwrap().clone()
    }

    /// Where the event log is now; pass it to the `_since` functions.
    pub fn mark(&self) -> usize {
        self.log.lock().unwrap().len()
    }

    pub fn since(&self, mark: usize) -> Vec<Event> {
        self.log.lock().unwrap()[mark..].to_vec()
    }

    /// Waits until the events since `mark` satisfy `cond`.
    pub fn wait_for(&self, mark: usize, cond: impl Fn(&[Event]) -> bool) -> bool {
        wait_until(TIMEOUT, || cond(&self.log.lock().unwrap()[mark..]))
    }

    /// Waits a little and returns everything since `mark`: for checking that
    /// something did *not* arrive.
    pub fn settle(&self, mark: usize) -> Vec<Event> {
        thread::sleep(Duration::from_millis(300));
        self.since(mark)
    }

    /// Waits for the screen to give the probe keyboard focus.
    pub fn wait_keyboard_focus(&self) {
        let ok = self.wait_for(0, |e| {
            e.iter()
                .rev()
                .find(|e| matches!(e, Event::KeyboardEnter | Event::KeyboardLeave))
                == Some(&Event::KeyboardEnter)
        });
        assert!(
            ok,
            "the app on the screen never got keyboard focus: {:?}",
            self.events()
        );
    }

    /// Button presses since `mark`: (button, x, y).
    pub fn presses_since(&self, mark: usize) -> Vec<(u32, f64, f64)> {
        presses(&self.since(mark))
    }

    /// The text that key presses (and text-input) produced since `mark`.
    pub fn typed_since(&self, mark: usize) -> String {
        typed(&self.since(mark))
    }

    /// Waits until at least `expected`'s length in chars has been typed
    /// since `mark`, and returns what was.
    pub fn wait_typed(&self, mark: usize, expected: &str) -> String {
        let n = expected.chars().count();
        self.wait_for(mark, |e| typed(e).chars().count() >= n);
        // Anything extra would arrive with the rest.
        thread::sleep(Duration::from_millis(100));
        self.typed_since(mark)
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn presses(events: &[Event]) -> Vec<(u32, f64, f64)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Button {
                button,
                pressed: true,
                x,
                y,
                ..
            } => Some((*button, *x, *y)),
            _ => None,
        })
        .collect()
}

pub fn typed(events: &[Event]) -> String {
    let mut text = String::new();
    for e in events {
        match e {
            Event::Key {
                name,
                utf8,
                pressed: true,
                mods,
                ..
            } if !mods.ctrl && !mods.alt && !mods.logo => match name.as_str() {
                "Return" | "KP_Enter" => text.push('\n'),
                "Tab" => text.push('\t'),
                _ if utf8.chars().all(|c| !c.is_control()) => text.push_str(utf8),
                _ => {}
            },
            Event::Text(t) => text.push_str(t),
            _ => {}
        }
    }
    text
}

/// The keysym that `name` (e.g. `Page_Down`, `F5`, `l`) stands for.
pub fn keysym(name: &str) -> u32 {
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS).raw();
    assert_ne!(sym, 0, "{name} isn't a keysym");
    sym
}

/// Whether `name` is the keysym of a modifier key.
pub fn is_modifier(name: &str) -> bool {
    [
        "Control_",
        "Shift_",
        "Alt_",
        "Meta_",
        "Super_",
        "Hyper_",
        "ISO_Level3",
    ]
    .iter()
    .any(|m| name.starts_with(m))
}

/// Key presses, in order: (keysym, keysym name, modifiers in effect).
pub fn key_presses(events: &[Event]) -> Vec<(u32, String, Mods)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key {
                sym,
                name,
                pressed: true,
                mods,
                ..
            } => Some((*sym, name.clone(), *mods)),
            _ => None,
        })
        .collect()
}

/// Presses of keys other than modifiers.
pub fn non_modifier_presses(events: &[Event]) -> Vec<(u32, String, Mods)> {
    key_presses(events)
        .into_iter()
        .filter(|(_, name, _)| !is_modifier(name))
        .collect()
}

/// Whether every key that went down in `events` came back up.
pub fn all_keys_released(events: &[Event]) -> bool {
    let mut down: Vec<u32> = Vec::new();
    for e in events {
        if let Event::Key { code, pressed, .. } = e {
            if *pressed {
                down.push(*code);
            } else {
                down.retain(|c| c != code);
            }
        }
    }
    down.is_empty()
}

struct State {
    log: Arc<Mutex<Vec<Event>>>,
    configured: Option<u32>,
    size: (i32, i32),
    surface: Option<wl_surface::WlSurface>,
    buffers: Vec<wl_buffer::WlBuffer>,
    busy: [bool; 2],
    file: Option<File>,
    pictures: u64,
    dirty: bool,
    marks: Vec<(i32, i32)>,
    pos: (f64, f64),
    pointer_frames: bool,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    xkb_context: xkb::Context,
    xkb_state: Option<xkb::State>,
    text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pending_text: Option<String>,
}

impl State {
    fn record(&mut self, event: Event) {
        self.log.lock().unwrap().push(event);
    }

    fn frame_bytes(&self) -> usize {
        (self.size.0 * self.size.1 * 4) as usize
    }

    /// Paints the next picture into a free buffer, or later if none is free.
    fn repaint(&mut self) {
        let Some(i) = self.busy.iter().position(|b| !b) else {
            self.dirty = true;
            return;
        };
        self.dirty = false;
        let (w, h) = self.size;
        let [r, g, b] = background(self.pictures);
        let mut pixels: Vec<u8> = [b, g, r, 0xff].repeat((w * h) as usize);
        let [mr, mg, mb] = MARK;
        for &(mx, my) in &self.marks {
            for y in (my - MARK_RADIUS).max(0)..(my + MARK_RADIUS + 1).min(h) {
                for x in (mx - MARK_RADIUS).max(0)..(mx + MARK_RADIUS + 1).min(w) {
                    let o = ((y * w + x) * 4) as usize;
                    pixels[o..o + 4].copy_from_slice(&[mb, mg, mr, 0xff]);
                }
            }
        }
        let offset = (i * self.frame_bytes()) as u64;
        self.file
            .as_ref()
            .unwrap()
            .write_all_at(&pixels, offset)
            .unwrap();
        let surface = self.surface.as_ref().unwrap();
        surface.attach(Some(&self.buffers[i]), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
        self.busy[i] = true;
        self.pictures += 1;
    }

    fn mods(&self) -> Mods {
        let Some(s) = &self.xkb_state else {
            return Mods::default();
        };
        let on = |m: &str| s.mod_name_is_active(m, xkb::STATE_MODS_EFFECTIVE);
        Mods {
            ctrl: on(xkb::MOD_NAME_CTRL),
            shift: on(xkb::MOD_NAME_SHIFT),
            alt: on(xkb::MOD_NAME_ALT),
            logo: on(xkb::MOD_NAME_LOGO),
        }
    }
}

fn run(
    stream: UnixStream,
    log: Arc<Mutex<Vec<Event>>>,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<(i32, i32)>,
) {
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=9, ())
        .unwrap_or_else(|e| panic!("the agent's screen has no seat: {e}"));
    let mut state = State {
        log,
        configured: None,
        size: (0, 0),
        surface: None,
        buffers: Vec::new(),
        busy: [false; 2],
        file: None,
        pictures: 0,
        dirty: false,
        marks: Vec::new(),
        pos: (0.0, 0.0),
        pointer_frames: seat.version() >= 5,
        pointer: None,
        keyboard: None,
        xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
        xkb_state: None,
        text_input_manager: globals.bind(&qh, 1..=1, ()).ok(),
        text_input: None,
        pending_text: None,
    };
    if let Some(m) = &state.text_input_manager {
        state.text_input = Some(m.get_text_input(&seat, &qh, ()));
    }

    let surface = compositor.create_surface(&qh, ());
    let window = wm.get_xdg_surface(&surface, &qh, ());
    let toplevel = window.get_toplevel(&qh, ());
    toplevel.set_title("spec25 probe".to_owned());
    surface.commit();
    while state.configured.is_none() {
        queue.blocking_dispatch(&mut state).unwrap();
    }
    let (w, h) = state.size;
    assert!(w > 0 && h > 0, "the screen gave no size: {w}x{h}");
    let file = tempfile::tempfile().unwrap();
    file.set_len((w * h * 4 * 2) as u64).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4 * 2, &qh, ());
    state.buffers = (0..2)
        .map(|i| {
            pool.create_buffer(
                i * w * h * 4,
                w,
                h,
                w * 4,
                wl_shm::Format::Xrgb8888,
                &qh,
                i as usize,
            )
        })
        .collect();
    state.file = Some(file);
    state.surface = Some(surface);
    state.repaint();
    queue.roundtrip(&mut state).unwrap();
    let _ = ready.send((w, h));
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = queue.blocking_dispatch(&mut state) {
            if !stop.load(Ordering::SeqCst) {
                state.record(Event::Disconnected(e.to_string()));
            }
            break;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        window: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            window.ack_configure(serial);
            state.configured = Some(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The first size sticks: the buffers are made for it.
        if let xdg_toplevel::Event::Configure { width, height, .. } = event
            && state.configured.is_none()
        {
            state.size = (width, height);
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.busy[*i] = false;
            if state.dirty {
                state.repaint();
            }
        }
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
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
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
        use wl_pointer::Event as P;
        let (x, y) = state.pos;
        let input = match event {
            P::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                state.pos = (surface_x, surface_y);
                state.record(Event::PointerEnter {
                    x: surface_x,
                    y: surface_y,
                });
                true
            }
            P::Leave { .. } => {
                state.record(Event::PointerLeave);
                false
            }
            P::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pos = (surface_x, surface_y);
                state.record(Event::Motion {
                    x: surface_x,
                    y: surface_y,
                });
                true
            }
            P::Button {
                button,
                state: s,
                time,
                ..
            } => {
                let pressed = s == WEnum::Value(wl_pointer::ButtonState::Pressed);
                if pressed {
                    state.marks.push((x.floor() as i32, y.floor() as i32));
                }
                state.record(Event::Button {
                    button,
                    pressed,
                    x,
                    y,
                    time,
                });
                true
            }
            P::Axis { axis, value, .. } => {
                let horizontal = axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll);
                state.record(Event::Axis {
                    horizontal,
                    value,
                    x,
                    y,
                });
                true
            }
            P::AxisDiscrete { axis, discrete } => {
                let horizontal = axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll);
                state.record(Event::Wheel {
                    horizontal,
                    value120: discrete * 120,
                });
                true
            }
            P::AxisValue120 { axis, value120 } => {
                let horizontal = axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll);
                state.record(Event::Wheel {
                    horizontal,
                    value120,
                });
                true
            }
            P::Frame => {
                state.repaint();
                false
            }
            _ => false,
        };
        if input && !state.pointer_frames {
            state.repaint();
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
        use wl_keyboard::Event as K;
        match event {
            K::Keymap { format, fd, size } => {
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    return;
                }
                let file = File::from(fd);
                let mut buf = vec![0; size as usize];
                file.read_exact_at(&mut buf, 0).unwrap();
                while buf.last() == Some(&0) {
                    buf.pop();
                }
                let text = String::from_utf8_lossy(&buf).into_owned();
                let keymap = xkb::Keymap::new_from_string(
                    &state.xkb_context,
                    text,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                )
                .unwrap_or_else(|| panic!("the screen sent a keymap xkbcommon can't read"));
                state.xkb_state = Some(xkb::State::new(&keymap));
            }
            K::Enter { .. } => state.record(Event::KeyboardEnter),
            K::Leave { .. } => state.record(Event::KeyboardLeave),
            K::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(s) = &mut state.xkb_state {
                    s.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            K::Key { key, state: ks, .. } => {
                let pressed = match ks {
                    WEnum::Value(wl_keyboard::KeyState::Pressed) => true,
                    WEnum::Value(wl_keyboard::KeyState::Released) => false,
                    _ => return, // repeats
                };
                let code = xkb::Keycode::new(key + 8);
                let (sym, utf8) = match &state.xkb_state {
                    Some(s) => (s.key_get_one_sym(code), s.key_get_utf8(code)),
                    None => (xkb::Keysym::new(0), String::new()),
                };
                let mods = state.mods();
                state.record(Event::Key {
                    code: key,
                    sym: sym.raw(),
                    name: xkb::keysym_get_name(sym),
                    utf8,
                    pressed,
                    mods,
                });
                state.repaint();
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for State {
    fn event(
        state: &mut Self,
        ti: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_text_input_v3::Event as T;
        match event {
            T::Enter { .. } => {
                ti.enable();
                ti.commit();
            }
            T::CommitString { text } => state.pending_text = text,
            T::Done { .. } => {
                if let Some(text) = state.pending_text.take() {
                    state.record(Event::Text(text));
                    state.repaint();
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: zwp_text_input_manager_v3::ZwpTextInputManagerV3);
