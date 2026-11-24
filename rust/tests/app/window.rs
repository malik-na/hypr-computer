//! The app's one window, and painting it.

use std::error::Error;
use std::io::Write;
use std::os::fd::AsFd;

use wayland_client::globals::{GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::App;

pub struct Window {
    pub surface: wl_surface::WlSurface,
    shm: wl_shm::WlShm,
    xdg: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    /// The size the screen asked for, once it did.
    size: Option<(i32, i32)>,
    configured: bool,
}

impl Window {
    pub fn new(globals: &GlobalList, qh: &QueueHandle<App>) -> Result<Window, Box<dyn Error>> {
        let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ())?;
        let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ())?;
        let wm: xdg_wm_base::XdgWmBase = globals.bind(qh, 1..=6, ())?;
        let surface = compositor.create_surface(qh, ());
        let xdg = wm.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg.get_toplevel(qh, ());
        toplevel.set_title("test app".to_owned());
        surface.commit();
        Ok(Window {
            surface,
            shm,
            xdg,
            _toplevel: toplevel,
            size: None,
            configured: false,
        })
    }

    pub fn configured(&self) -> bool {
        self.configured
    }

    /// Paints the window's top half `top` and its bottom half `bottom`.
    pub fn paint(
        &self,
        qh: &QueueHandle<App>,
        top: [u8; 3],
        bottom: [u8; 3],
    ) -> Result<(), Box<dyn Error>> {
        let (width, height) = self.size.ok_or("the screen gave no size")?;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let [r, g, b] = if y < height / 2 { top } else { bottom };
            for _ in 0..width {
                pixels.extend([b, g, r, 0xff]); // XRGB8888, little-endian
            }
        }
        let mut file = tempfile::tempfile()?;
        file.write_all(&pixels)?;
        let pool = self
            .shm
            .create_pool(file.as_fd(), i32::try_from(pixels.len())?, qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Xrgb8888,
            qh,
            (),
        );
        pool.destroy();
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
        Ok(())
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
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

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            app.window.xdg.ack_configure(serial);
            app.window.configured = true;
            app.dirty = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event
            && width > 0
            && height > 0
        {
            app.window.size = Some((width, height));
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

delegate_noop!(App: wl_compositor::WlCompositor);
delegate_noop!(App: ignore wl_surface::WlSurface);
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: wl_shm_pool::WlShmPool);
