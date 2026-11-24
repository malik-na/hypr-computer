//! A tiny Wayland app that paints four coloured quadrants (so a picture shows
//! whose app it is, and which way up and round), and records every input
//! event it gets. On a pointer or key event it turns magenta, so the input
//! also shows in a screenshot.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

pub const MAGENTA: [u8; 3] = [0xff, 0x00, 0xff];

/// Top left, top right, bottom left, bottom right.
pub type Quadrants = [[u8; 3]; 4];

pub struct Probe {
    events: Arc<Mutex<Vec<String>>>,
    has_seat: bool,
}

struct State {
    configured: Option<u32>,
    size: (i32, i32),
    events: Arc<Mutex<Vec<String>>>,
    surface: Option<wl_surface::WlSurface>,
    magenta: Option<wl_buffer::WlBuffer>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
}

impl Probe {
    /// Opens a window on the compositor at `socket`, paints it, and returns
    /// once the compositor has the picture.
    pub fn paint(socket: &Path, colours: Quadrants, app_id: &str) -> Probe {
        let stream = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("can't connect to {}: {e}", socket.display()));
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue): (_, EventQueue<State>) =
            registry_queue_init(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
        let has_seat = seat.is_some();

        let events = Arc::new(Mutex::new(Vec::new()));
        let surface = compositor.create_surface(&qh, ());
        let window = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = window.get_toplevel(&qh, ());
        toplevel.set_title("test app".to_owned());
        toplevel.set_app_id(app_id.to_owned());
        surface.commit();
        let mut state = State {
            configured: None,
            size: (0, 0),
            events: events.clone(),
            surface: None,
            magenta: None,
            pointer: None,
            keyboard: None,
        };
        while state.configured.is_none() {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        window.ack_configure(state.configured.unwrap());
        let (width, height) = match state.size {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => (1280, 800),
        };
        let quads = buffer(&shm, &qh, width, height, |x, y| {
            let right = usize::from(x >= width / 2);
            let bottom = usize::from(y >= height / 2);
            colours[bottom * 2 + right]
        });
        state.magenta = Some(buffer(&shm, &qh, width, height, |_, _| MAGENTA));
        surface.attach(Some(&quads), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        state.surface = Some(surface);
        queue.roundtrip(&mut state).unwrap();
        // Keeps the app alive and listening until its compositor goes away.
        thread::spawn(move || {
            let _keep = (connection, quads, toplevel, window, seat);
            while queue.blocking_dispatch(&mut state).is_ok() {}
        });
        Probe { events, has_seat }
    }

    /// Every input event the app got so far.
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// Whether the compositor offered the app a seat at all.
    pub fn has_seat(&self) -> bool {
        self.has_seat
    }
}

fn buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<State>,
    width: i32,
    height: i32,
    colour: impl Fn(i32, i32) -> [u8; 3],
) -> wl_buffer::WlBuffer {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = colour(x, y);
            pixels.extend([b, g, r, 0xff]); // XRGB8888, little-endian
        }
    }
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&pixels).unwrap();
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, qh, ());
    pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Xrgb8888,
        qh,
        (),
    )
}

impl State {
    fn record(&mut self, event: String, turns_magenta: bool) {
        self.events.lock().unwrap().push(event);
        if turns_magenta && let (Some(s), Some(b)) = (&self.surface, &self.magenta) {
            s.attach(Some(b), 0, 0);
            s.damage_buffer(0, 0, i32::MAX, i32::MAX);
            s.commit();
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
        let input = matches!(
            event,
            wl_pointer::Event::Motion { .. }
                | wl_pointer::Event::Button { .. }
                | wl_pointer::Event::Axis { .. }
                | wl_pointer::Event::AxisValue120 { .. }
                | wl_pointer::Event::AxisDiscrete { .. }
        );
        if !matches!(event, wl_pointer::Event::Frame) {
            state.record(format!("pointer {event:?}"), input);
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
            wl_keyboard::Event::Keymap { .. } | wl_keyboard::Event::RepeatInfo { .. } => {}
            wl_keyboard::Event::Key { key, .. } => state.record(format!("key {key}"), true),
            other => state.record(format!("keyboard {other:?}"), false),
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
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            if state.configured.is_some() {
                surface.ack_configure(serial);
            }
            state.configured.get_or_insert(serial);
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
        if let xdg_toplevel::Event::Configure { width, height, .. } = event
            && state.configured.is_none()
        {
            state.size = (width, height);
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
