//! The live view: the screen as a window on the user's desktop, so they can
//! watch the agent. The screen is a client of the desktop's compositor for
//! this, in the same event loop as everything else.
//!
//! Watch-only until the user takes over: the view takes the desktop's seat,
//! but the user's pointer and keys reach the agent's apps only through the
//! daemon's take-over, in `user.rs`.
//!
//! The window is black, at whatever size the desktop gives it, with the
//! screen in a subsurface scaled to fit, its shape kept. The screen is drawn
//! only when the desktop asks for a frame (so not while the window is
//! hidden) and it has changed since the frame the view last showed.

use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use domain::view::APP_ID;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{MemfdFlags, memfd_create};
use rustix::io::Errno;
use smithay::backend::allocator::Fourcc;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::banner::{Banner, COLOUR};
use crate::state::{HEIGHT, State, WIDTH};
use crate::user::ViewSeat;

/// One frame's bytes: the whole screen, four bytes a pixel.
const FRAME_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;
/// The shared memory: two frames, one black pixel, and one of the banner's
/// colour.
const MEMORY_BYTES: usize = 2 * FRAME_BYTES + 8;

/// How long the desktop may take to answer as the view opens. The screen
/// serves nothing else meanwhile, and the daemon ends a session whose screen
/// doesn't answer it within 10 s: a desktop that hangs must cost only the view.
const ANSWER_TIME: Duration = Duration::from_secs(2);

/// The view's window and the connection it lives on.
pub struct View {
    /// Tells this view from one opened after it.
    id: u64,
    connection: Connection,
    qh: QueueHandle<State>,
    /// The window itself: one black pixel, stretched to its size.
    window: Scaled,
    /// The screen, inside the window.
    picture: Scaled,
    placement: wl_subsurface::WlSubsurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    /// The window's size, as the desktop last asked; 0 for "any".
    size: (i32, i32),
    /// Two frames and a black pixel's worth of shared memory, so a frame can
    /// be drawn while the desktop still reads the other.
    memory: File,
    buffers: [wl_buffer::WlBuffer; 2],
    black: wl_buffer::WlBuffer,
    /// Which buffers the desktop still holds.
    busy: [bool; 2],
    /// The desktop asked for a frame and hasn't had one since.
    wanted: bool,
    /// The screen's `frame` the view last showed, and the buffer it is in.
    shown: Option<u64>,
    last: Option<usize>,
    /// Where the screen is in the window, and its size there.
    pub fit: ((i32, i32), (i32, i32)),
    /// The desktop's pointer and keyboard, for taking over.
    pub seat: ViewSeat,
    pub banner: Banner,
}

/// A surface whose picture is scaled to the size it is given.
struct Scaled {
    surface: wl_surface::WlSurface,
    viewport: wp_viewport::WpViewport,
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
        answers(&connection).map_err(|e| failed(&e))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&connection).map_err(|e| failed(&e))?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor =
            globals.bind(&qh, 4..=6, ()).map_err(|e| failed(&e))?;
        let subcompositor: wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, 1..=1, ()).map_err(|e| failed(&e))?;
        let viewporter: wp_viewporter::WpViewporter =
            globals.bind(&qh, 1..=1, ()).map_err(|e| failed(&e))?;
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| failed(&e))?;
        let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).map_err(|e| failed(&e))?;
        // Without a seat the view is only ever watched.
        let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=5, ()).ok();

        let memory = File::from(
            memfd_create("hypr-computer-view", MemfdFlags::CLOEXEC).map_err(|e| failed(&e))?,
        );
        // A new memfd is all zeros: the black pixel is already there.
        memory
            .set_len(MEMORY_BYTES as u64)
            .map_err(|e| failed(&e))?;
        memory
            .write_all_at(&COLOUR, (2 * FRAME_BYTES + 4) as u64)
            .map_err(|e| failed(&e))?;
        let pool = shm.create_pool(memory.as_fd(), MEMORY_BYTES as i32, &qh, ());
        let black = pool.create_buffer(
            (2 * FRAME_BYTES) as i32,
            1,
            1,
            4,
            wl_shm::Format::Xrgb8888,
            &qh,
            2,
        );
        let colour = pool.create_buffer(
            (2 * FRAME_BYTES + 4) as i32,
            1,
            1,
            4,
            wl_shm::Format::Xrgb8888,
            &qh,
            3,
        );
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

        let scaled = || {
            let surface = compositor.create_surface(&qh, ());
            let viewport = viewporter.get_viewport(&surface, &qh, ());
            Scaled { surface, viewport }
        };
        let (window, picture) = (scaled(), scaled());
        let placement = subcompositor.get_subsurface(&picture.surface, &window.surface, &qh, ());
        // Frames go up as they are drawn, not with the window's own commits.
        placement.set_desync();
        let toplevel = wm
            .get_xdg_surface(&window.surface, &qh, ())
            .get_toplevel(&qh, ());
        toplevel.set_title(title.to_owned());
        toplevel.set_app_id(APP_ID.to_owned());
        let mut banner = Banner::new(
            &compositor,
            &subcompositor,
            &viewporter,
            &window.surface,
            colour,
            &qh,
        );
        banner.show(self.user.has_screen);
        window.surface.commit();

        let fd = connection
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|e| failed(&e))?;
        self.views_opened += 1;
        let id = self.views_opened;
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
                        // Its view is closed: dropping this lets the connection go,
                        // and the desktop takes the window away.
                        None if state.view.as_ref().map(|v| v.id) != Some(id) => {
                            Ok(PostAction::Remove)
                        }
                        None => Ok(PostAction::Continue),
                    }
                },
            )
            .map_err(|e| failed(&e))?;
        self.view = Some(View {
            id,
            connection,
            qh,
            window,
            picture,
            placement,
            _toplevel: toplevel,
            size: (0, 0),
            memory,
            buffers,
            black,
            busy: [false; 2],
            wanted: false,
            shown: None,
            last: None,
            fit: fit(WIDTH, HEIGHT),
            seat: ViewSeat::new(seat),
            banner,
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
        view.last = Some(free);
        let picture = &view.picture.surface;
        picture.attach(Some(&view.buffers[free]), 0, 0);
        picture.damage_buffer(0, 0, WIDTH, HEIGHT);
        picture.frame(&view.qh, ());
        picture.commit();
    }

    /// Sends the desktop what the view asked of it. What doesn't fit in the
    /// socket now goes on the next try.
    pub fn flush_view(&mut self) {
        let Some(view) = &self.view else { return };
        match view.connection.flush() {
            Err(WaylandError::Io(e)) if e.kind() != io::ErrorKind::WouldBlock => {
                self.close_view(&e.to_string());
            }
            Err(e @ WaylandError::Protocol(_)) => self.close_view(&e.to_string()),
            _ => {}
        }
    }

    /// Forgets the view: its connection broke, or the user closed it.
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

impl View {
    /// Whether `surface` is the picture of the screen.
    pub fn is_picture(&self, surface: &wl_surface::WlSurface) -> bool {
        self.picture.surface == *surface
    }

    /// Lays the window out at the size the desktop asked for: black, with
    /// the screen as large as fits in the middle.
    fn lay_out(&mut self) {
        let (width, height) = match self.size {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => (WIDTH, HEIGHT),
        };
        self.fit = fit(width, height);
        self.banner.lay_out(self.fit);
        let ((x, y), (fit_width, fit_height)) = self.fit;
        self.picture.viewport.set_destination(fit_width, fit_height);
        // Hyprland (0.57.2) applies a new viewport size only with a buffer,
        // so the frame on show goes up again with it.
        if let Some(last) = self.last
            && let (Some(buffer), Some(busy)) = (self.buffers.get(last), self.busy.get_mut(last))
        {
            *busy = true;
            self.picture.surface.attach(Some(buffer), 0, 0);
            self.picture.surface.damage_buffer(0, 0, WIDTH, HEIGHT);
        }
        self.picture.surface.commit();
        self.placement.set_position(x, y);
        self.window.viewport.set_destination(width, height);
        self.window.surface.attach(Some(&self.black), 0, 0);
        self.window.surface.damage_buffer(0, 0, 1, 1);
        self.window.surface.commit();
    }
}

/// Where the screen goes in a window `width` by `height`: as large as fits,
/// its shape kept, in the middle. Its offset, then its size.
fn fit(width: i32, height: i32) -> ((i32, i32), (i32, i32)) {
    let scale = f64::min(
        f64::from(width) / f64::from(WIDTH),
        f64::from(height) / f64::from(HEIGHT),
    );
    let fit_width = ((f64::from(WIDTH) * scale).round() as i32).clamp(1, width);
    let fit_height = ((f64::from(HEIGHT) * scale).round() as i32).clamp(1, height);
    (
        ((width - fit_width) / 2, (height - fit_height) / 2),
        (fit_width, fit_height),
    )
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
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        let Some(view) = &mut state.view else { return };
        xdg.ack_configure(serial);
        view.lay_out();
        // The first frame comes with the first layout.
        if view.shown.is_none() {
            state.view_wanted();
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
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if let Some(view) = &mut state.view {
                    view.size = (width, height);
                }
            }
            // The user closed it; `hypr-computer watch` opens it again.
            xdg_toplevel::Event::Close => state.close_view("the user closed it"),
            _ => {}
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
        // Index 2 is the black pixel and 3 the banner's, which are never redrawn.
        if let wl_buffer::Event::Release = event
            && let Some(busy) = state.view.as_mut().and_then(|v| v.busy.get_mut(*index))
        {
            *busy = false;
            state.draw_view();
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: wl_subcompositor::WlSubcompositor);
delegate_noop!(State: wl_subsurface::WlSubsurface);
delegate_noop!(State: wp_viewporter::WpViewporter);
delegate_noop!(State: wp_viewport::WpViewport);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: wl_region::WlRegion);

/// Waits, at most [`ANSWER_TIME`], for the desktop at `connection` to answer
/// a first request.
fn answers(connection: &Connection) -> Result<(), String> {
    let mut queue = connection.new_event_queue::<Answered>();
    let _sync = connection.display().sync(&queue.handle(), ());
    let mut answered = Answered(false);
    let end = Instant::now() + ANSWER_TIME;
    while !answered.0 {
        connection.flush().map_err(|e| e.to_string())?;
        if let Some(guard) = queue.prepare_read() {
            let left = end.saturating_duration_since(Instant::now());
            let timeout = Timespec::try_from(left).map_err(|e| e.to_string())?;
            let ready = {
                let fd = guard.connection_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                poll(&mut fds, Some(&timeout))
            };
            match ready {
                Ok(0) => {
                    return Err(format!(
                        "the desktop didn't answer within {}s",
                        ANSWER_TIME.as_secs()
                    ));
                }
                Err(Errno::INTR) => continue,
                Err(e) => return Err(e.to_string()),
                Ok(_) => match guard.read() {
                    Err(WaylandError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.to_string()),
                    Ok(_) => {}
                },
            }
        }
        queue
            .dispatch_pending(&mut answered)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether the desktop has answered.
struct Answered(bool);

impl Dispatch<wl_callback::WlCallback, ()> for Answered {
    fn event(
        answered: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            answered.0 = true;
        }
    }
}
