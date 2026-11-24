//! A tiny Wayland app for the tests: one window, its top half one colour and
//! its bottom half another, so a screenshot shows whose app it is and which
//! way up.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The app; it stays on the screen until dropped.
pub struct App {
    _connection: Connection,
    _queue: EventQueue<State>,
}

#[derive(Default)]
struct State {
    /// The serial of the window's first configure, once it came.
    configured: Option<u32>,
    size: (i32, i32),
}

impl App {
    /// Opens a window on the screen at `socket`, paints it, and returns once
    /// the screen has the picture.
    pub fn paint(socket: &Path, top: [u8; 3], bottom: [u8; 3]) -> App {
        let stream = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("can't connect to {}: {e}", socket.display()));
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<State>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();

        let surface = compositor.create_surface(&qh, ());
        let window = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = window.get_toplevel(&qh, ());
        toplevel.set_title("test app".to_owned());
        surface.commit();
        let mut state = State::default();
        while state.configured.is_none() {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        window.ack_configure(state.configured.unwrap());

        let (width, height) = state.size;
        assert!(
            width > 0 && height > 0,
            "the screen gave no size: {width}x{height}"
        );
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let [r, g, b] = if y < height / 2 { top } else { bottom };
            for _ in 0..width {
                pixels.extend([b, g, r, 0xff]); // XRGB8888, little-endian
            }
        }
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&pixels).unwrap();
        let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Xrgb8888,
            &qh,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        // Once the screen answers this, it has handled the commit above.
        queue.roundtrip(&mut state).unwrap();
        App {
            _connection: connection,
            _queue: queue,
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
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
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
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.size = (width, height);
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
