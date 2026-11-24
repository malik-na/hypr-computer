//! hc-view: THROWAWAY live view of one hc-screen. A normal window on the host desktop.
//!
//!   hc-view --name hc-0        (WAYLAND_DISPLAY is the HOST's display)
//!
//! Watch-only by construction: this client never binds wl_seat, so the host's pointer and
//! keyboard have no path to the agent's apps. It pulls a frame from the screen each time the
//! host asks it to draw (frame callback), so when the window is hidden it simply stops asking
//! and the screen carries on unaffected.

use std::{
    io::{BufRead, BufReader, Write},
    os::fd::{AsFd, FromRawFd, OwnedFd},
    os::unix::net::UnixStream,
};

use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::{
    wp::viewporter::client::{wp_viewport, wp_viewporter},
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};

const W: i32 = 1280;
const H: i32 = 800;
const FRAME_BYTES: usize = (W * H * 4) as usize;

struct App {
    running: bool,
    configured: bool,
    size: (i32, i32),
    resized: bool,
    surface: wl_surface::WlSurface,
    viewport: wp_viewport::WpViewport,
    buffers: [wl_buffer::WlBuffer; 2],
    busy: [bool; 2],
    pool: memmap2::MmapMut,
    frame: memmap2::Mmap,
    ctl_out: UnixStream,
    ctl_in: BufReader<UnixStream>,
    last_seq: u64,
    drawn: u64,
}

impl App {
    /// Ask the screen for its current frame; returns its sequence number.
    fn pull(&mut self) -> Option<u64> {
        self.ctl_out.write_all(b"frame\n").ok()?;
        let mut line = String::new();
        if self.ctl_in.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&line).ok()?;
        v.get("seq")?.as_u64()
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(seq) = self.pull() else {
            self.running = false;
            return;
        };
        if seq != self.last_seq || self.resized {
            if let Some(i) = self.busy.iter().position(|b| !b) {
                let dst = &mut self.pool[i * FRAME_BYTES..(i + 1) * FRAME_BYTES];
                // screen gives R,G,B,A bytes; wl_shm XRGB8888 is B,G,R,X in memory
                for (d, s) in dst.chunks_exact_mut(4).zip(self.frame.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
                self.busy[i] = true;
                self.surface.attach(Some(&self.buffers[i]), 0, 0);
                self.surface.damage_buffer(0, 0, W, H);
                self.viewport.set_destination(self.size.0, self.size.1);
                self.last_seq = seq;
                self.resized = false;
                self.drawn += 1;
            }
        }
        self.surface.frame(qh, ());
        self.surface.commit();
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
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(
        app: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            app.resized = true;
            if !app.configured {
                app.configured = true;
                app.draw(qh);
            }
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
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    app.size = (width, height);
                }
            }
            xdg_toplevel::Event::Close => app.running = false,
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            app.draw(qh);
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, usize> for App {
    fn event(
        app: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            app.busy[*index] = false;
        }
    }
}

delegate_noop!(App: wl_compositor::WlCompositor);
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: wl_shm_pool::WlShmPool);
delegate_noop!(App: ignore wl_surface::WlSurface);
delegate_noop!(App: wp_viewporter::WpViewporter);
delegate_noop!(App: wp_viewport::WpViewport);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut name = "hc-0".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--name" {
            name = args.next().expect("--name NAME");
        }
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")?;
    let ctl = UnixStream::connect(format!("{runtime}/{name}.ctl"))?;
    let frame_file = std::fs::File::open(format!("{runtime}/{name}.fb"))?;
    let frame = unsafe { memmap2::Mmap::map(&frame_file)? };

    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ())?;
    let viewporter: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;

    // Two shm buffers in one pool, so we never draw into the one the host is showing.
    let fd = unsafe { libc::memfd_create(c"hc-view".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let file = std::fs::File::from(fd);
    file.set_len(2 * FRAME_BYTES as u64)?;
    let pool_map = unsafe { memmap2::MmapMut::map_mut(&file)? };
    let pool = shm.create_pool(file.as_fd(), 2 * FRAME_BYTES as i32, &qh, ());
    let buffers = [0usize, 1].map(|i| {
        pool.create_buffer((i * FRAME_BYTES) as i32, W, H, W * 4, wl_shm::Format::Xrgb8888, &qh, i)
    });

    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title(format!("hc-view {name}"));
    toplevel.set_app_id("hc-view".into());
    surface.commit();

    let mut app = App {
        running: true,
        configured: false,
        size: (W, H),
        resized: true,
        surface,
        viewport,
        buffers,
        busy: [false; 2],
        pool: pool_map,
        frame,
        ctl_in: BufReader::new(ctl.try_clone()?),
        ctl_out: ctl,
        last_seq: 0,
        drawn: 0,
    };
    while app.running {
        queue.blocking_dispatch(&mut app)?;
    }
    eprintln!("hc-view: exiting after {} frames", app.drawn);
    Ok(())
}
