//! The live view: the screen as a window on the user's desktop, so they can
//! watch the agent. The screen is a client of the desktop's compositor for
//! this, in the same event loop as everything else.
//!
//! Watch-only by construction: the view never binds `wl_seat`, so the user's
//! pointer and keys have no way to reach the agent's apps.
//!
//! The screen is drawn only when the desktop asks for a frame (so not while
//! the window is hidden) and it has changed since the frame the view last
//! showed.

use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;

use domain::view::APP_ID;
use rustix::fs::{MemfdFlags, memfd_create};
use smithay::backend::allocator::Fourcc;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::state::{HEIGHT, State, WIDTH};

/// One frame's bytes: the whole screen, four bytes a pixel.
const FRAME_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;
/// The shared memory: two frames.
const MEMORY_BYTES: usize = 2 * FRAME_BYTES;

/// The view's window and the connection it lives on.
pub struct View {
    connection: Connection,
    qh: QueueHandle<State>,
    surface: wl_surface::WlSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    /// Two frames' worth of shared memory, so one can be drawn while the
    /// desktop still reads the other.
    memory: File,
    buffers: [wl_buffer::WlBuffer; 2],
    /// Which buffers the desktop still holds.
    busy: [bool; 2],
    /// The desktop asked for a frame and hasn't had one since.
    wanted: bool,
    /// The screen's `frame` the view last showed.
    shown: Option<u64>,
}

impl State {
    /// Opens the view as a window called `title`, unless it is open.
    pub fn open_view(&mut self, title: &str) -> Result<(), String> {
        if self.view.is_some() {
            return Ok(());
        }
        let desktop = self
            .desktop
            .as_ref()
            .ok_or("there is no desktop to show the view on")?;
        let failed = |e: &dyn std::fmt::Display| {
            format!("can't open a window on {}: {e}", desktop.display())
        };
        let stream = UnixStream::connect(desktop).map_err(|e| failed(&e))?;
        let connection = Connection::from_socket(stream).map_err(|e| failed(&e))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&connection).map_err(|e| failed(&e))?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor =
            globals.bind(&qh, 4..=6, ()).map_err(|e| failed(&e))?;
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| failed(&e))?;
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).map_err(|e| failed(&e))?;

        let memory = File::from(
            memfd_create("hypr-computer-view", MemfdFlags::CLOEXEC).map_err(|e| failed(&e))?,
        );
        memory
            .set_len(MEMORY_BYTES as u64)
            .map_err(|e| failed(&e))?;
        let pool = shm.create_pool(memory.as_fd(), MEMORY_BYTES as i32, &qh, ());
        let buffers = [0, 1].map(|i| {
            pool.create_buffer(
                (i * FRAME_BYTES) as i32,
                WIDTH,
                HEIGHT,
                WIDTH * 4,
                wl_shm::Format::Xrgb8888,
                &qh,
                i,
            )
        });
        pool.destroy();

        let surface = compositor.create_surface(&qh, ());
        let toplevel = wm.get_xdg_surface(&surface, &qh, ()).get_toplevel(&qh, ());
        toplevel.set_title(title.to_owned());
        toplevel.set_app_id(APP_ID.to_owned());
        surface.commit();

        let fd = connection
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|e| failed(&e))?;
        self.loop_handle
            .insert_source(
                Generic::new(fd, Interest::READ, Mode::Level),
                move |_, _, state| {
                    let broke = match queue.prepare_read().map(|guard| guard.read()) {
                        Some(Err(WaylandError::Io(e))) if e.kind() != io::ErrorKind::WouldBlock => {
                            Some(e.to_string())
                        }
                        Some(Err(e @ WaylandError::Protocol(_))) => Some(e.to_string()),
                        _ => queue.dispatch_pending(state).err().map(|e| e.to_string()),
                    };
                    match broke {
                        Some(why) => {
                            state.close_view(&why);
                            Ok(PostAction::Remove)
                        }
                        None => Ok(PostAction::Continue),
                    }
                },
            )
            .map_err(|e| failed(&e))?;
        self.view = Some(View {
            connection,
            qh,
            surface,
            _toplevel: toplevel,
            memory,
            buffers,
            busy: [false; 2],
            wanted: false,
            shown: None,
        });
        self.flush_view();
        Ok(())
    }

    /// Gives the desktop the screen as it is now, if it asked for a frame
    /// and the screen changed since the last one.
    pub fn draw_view(&mut self) {
        if !self.view.as_ref().is_some_and(|v| v.wanted) {
            return;
        }
        if let Err(why) = self.render() {
            eprintln!("hypr-computer-screen: the live view can't draw: {why}");
            return;
        }
        let frame = self.frame;
        let Some(view) = &self.view else { return };
        let (true, Some(free)) = (view.shown != Some(frame), view.busy.iter().position(|b| !b))
        else {
            return;
        };
        // Argb8888 is what the desktop calls Xrgb8888: B, G, R, then a byte
        // it ignores.
        let pixels = match self.pixels(Fourcc::Argb8888) {
            Ok(pixels) => pixels,
            Err(why) => {
                eprintln!("hypr-computer-screen: the live view can't draw: {why}");
                return;
            }
        };
        let Some(view) = &mut self.view else { return };
        if let Err(e) = view
            .memory
            .write_all_at(&pixels, (free * FRAME_BYTES) as u64)
        {
            eprintln!("hypr-computer-screen: the live view can't draw: {e}");
            return;
        }
        view.busy[free] = true;
        view.wanted = false;
        view.shown = Some(frame);
        view.surface.attach(Some(&view.buffers[free]), 0, 0);
        view.surface.damage_buffer(0, 0, WIDTH, HEIGHT);
        view.surface.frame(&view.qh, ());
        view.surface.commit();
    }

    /// Sends the desktop what the view asked of it.
    pub fn flush_view(&mut self) {
        if let Some(view) = &self.view
            && let Err(e) = view.connection.flush()
        {
            self.close_view(&e.to_string());
        }
    }

    /// Forgets the view: its connection broke, so the desktop dropped it.
    fn close_view(&mut self, why: &str) {
        if self.view.take().is_some() {
            eprintln!("hypr-computer-screen: the live view closed: {why}");
        }
    }

    /// The desktop wants a frame.
    fn view_wanted(&mut self) {
        if let Some(view) = &mut self.view {
            view.wanted = true;
        }
        self.draw_view();
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
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            // The first frame maps the window.
            if state.view.as_ref().is_some_and(|v| v.shown.is_none()) {
                state.view_wanted();
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.view_wanted();
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some(busy) = state.view.as_mut().and_then(|v| v.busy.get_mut(*index))
        {
            *busy = false;
            state.draw_view();
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
