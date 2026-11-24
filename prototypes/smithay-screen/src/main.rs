//! hc-screen: THROWAWAY prototype of "the agent's screen".
//!
//! A headless Wayland compositor (Smithay). Apps connect to it as their Wayland display.
//! There is no host window, no input device and no screencopy client: the controlling
//! process injects input and reads pixels over a line protocol on a unix socket.
//!
//!   hc-screen --name hc-0 --renderer gles|pixman [--pixman-dmabuf] [--render-node PATH]
//!
//! Sockets, all in $XDG_RUNTIME_DIR:
//!   <name>        the Wayland display for the agent's apps (WAYLAND_DISPLAY=<name>)
//!   <name>.ctl    control socket, one command per line, one JSON reply per line
//!   <name>.fb     the last frame handed to a live view (RGBA, 1280x800), see `frame`

mod control;

use std::{
    os::unix::net::UnixListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use serde_json::json;
use smithay::{
    backend::{
        allocator::{dmabuf::Dmabuf, gbm::GbmDevice, Fourcc},
        drm::DrmNode,
        egl::{EGLContext, EGLDisplay},
        input::{Axis, AxisSource, ButtonState, KeyState},
        renderer::{
            damage::OutputDamageTracker,
            element::surface::WaylandSurfaceRenderElement,
            gles::{GlesRenderer, GlesTexture},
            pixman::PixmanRenderer,
            utils::on_commit_buffer_handler,
            buffer_type, Bind, BufferType, ExportMem, ImportDma, Offscreen,
        },
    },
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, space::render_output, PopupKeyboardGrab,
        PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Space, Window,
        WindowSurfaceType,
    },
    input::{
        keyboard::{FilterResult, XkbConfig},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, Focus, MotionEvent},
        Seat, SeatHandler, SeatState,
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            channel,
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, LoopSignal, Mode as CalloopMode, PostAction,
        },
        pixman,
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
            Client, Display, DisplayHandle, Resource,
        },
    },
    utils::{Logical, Point, Rectangle, Serial, Transform, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, BufferAssignment, CompositorClientState,
            CompositorHandler, CompositorState, SurfaceAttributes,
        },
        dmabuf::{
            DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            decoration::{XdgDecorationHandler, XdgDecorationState},
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
};

use control::{Cmd, Reply, Req};

pub const W: i32 = 1280;
pub const H: i32 = 800;

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Trace to stderr when HC_DEBUG is set.
fn debug(args: std::fmt::Arguments<'_>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HC_DEBUG").is_some()) {
        eprintln!("hc-screen: {args}");
    }
}

enum Backend {
    Gles {
        renderer: GlesRenderer,
        target: GlesTexture,
    },
    Pixman {
        renderer: PixmanRenderer,
        target: pixman::Image<'static, 'static>,
    },
}

struct State {
    start: Instant,
    dh: DisplayHandle,
    loop_signal: LoopSignal,

    space: Space<Window>,
    output: Output,
    /// Every toplevel, oldest first. Only `shown` is mapped: one app full-screen at a time.
    windows: Vec<Window>,
    shown: Option<Window>,
    popups: PopupManager,
    seat: Seat<Self>,

    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    dmabuf: Option<(DmabufState, DmabufGlobal)>,
    _output_manager_state: OutputManagerState,
    _xdg_decoration_state: XdgDecorationState,
    _viewporter_state: ViewporterState,
    _single_pixel_state: SinglePixelBufferState,
    /// Buffers committed by clients, by kind: [shm, dmabuf, other].
    commits: [u64; 3],

    backend: Backend,
    tracker: OutputDamageTracker,
    /// Something was committed since the last render.
    dirty: bool,
    renders: u64,
    /// Bumped by every render that changed pixels.
    render_seq: u64,
    /// `render_seq` of the frame currently in the shared frame file.
    fb_seq: u64,
    fb: memmap2::MmapMut,
    pointer_pos: Point<f64, Logical>,
}

/// Render the space into the offscreen target; optionally read the pixels back (RGBA, top-down).
/// A macro and not a generic fn: the two renderers share no convenient common bound.
macro_rules! render_into {
    ($renderer:expr, $target:expr, $rty:ty, $state:expr, $want_pixels:expr) => {{
        (|| -> Result<(bool, Option<Vec<u8>>), String> {
            let renderer = $renderer;
            let mut fb = renderer.bind($target).map_err(|e| format!("bind: {e:?}"))?;
            let damaged = render_output::<_, WaylandSurfaceRenderElement<$rty>, _, _>(
                $state.0,
                renderer,
                &mut fb,
                1.0,
                1, // one persistent buffer: its age is always 1
                [$state.1],
                &[],
                $state.2,
                [0.1, 0.1, 0.1, 1.0],
            )
            .map_err(|e| format!("render: {e:?}"))?
            .damage
            .is_some();
            let pixels = if $want_pixels {
                let region = Rectangle::from_size((W, H).into());
                let mapping = renderer
                    .copy_framebuffer(&fb, region, Fourcc::Abgr8888)
                    .map_err(|e| format!("copy_framebuffer: {e:?}"))?;
                // Note: GlesMapping::flipped() says true, but an offscreen texture rendered with
                // Transform::Normal reads back top-down already. Flipping here was wrong.
                let bytes = renderer
                    .map_texture(&mapping)
                    .map_err(|e| format!("map_texture: {e:?}"))?;
                Some(bytes[..(W * H * 4) as usize].to_vec())
            } else {
                None
            };
            Ok((damaged, pixels))
        })()
    }};
}

impl State {
    fn now(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn render(&mut self, want_pixels: bool) -> Result<Option<Vec<u8>>, String> {
        let parts = (&self.output, &self.space, &mut self.tracker);
        let (damaged, pixels) = match &mut self.backend {
            Backend::Gles { renderer, target } => {
                render_into!(renderer, target, GlesRenderer, parts, want_pixels)
            }
            Backend::Pixman { renderer, target } => {
                render_into!(renderer, target, PixmanRenderer, parts, want_pixels)
            }
        }?;
        self.dirty = false;
        self.renders += 1;
        if damaged {
            self.render_seq += 1;
        }
        // Clients draw their next frame when they get this; only the shown app gets it.
        if let Some(window) = &self.shown {
            let output = &self.output;
            window.send_frame(output, self.start.elapsed(), Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        Ok(pixels)
    }

    fn tick(&mut self) {
        if STOP.load(Ordering::SeqCst) {
            self.loop_signal.stop();
            return;
        }
        if self.dirty {
            if let Err(e) = self.render(false) {
                eprintln!("hc-screen: render failed: {e}");
            }
            self.space.refresh();
            self.popups.cleanup();
        }
    }

    fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    fn window_for(&self, surface: &WlSurface) -> Option<&Window> {
        self.windows
            .iter()
            .find(|w| w.toplevel().unwrap().wl_surface() == surface)
    }

    /// Make `window` the one app on the screen.
    fn show(&mut self, window: Window) {
        if let Some(old) = self.shown.take() {
            if old != window {
                self.space.unmap_elem(&old);
                old.set_activated(false);
                if old.toplevel().unwrap().is_initial_configure_sent() {
                    old.toplevel().unwrap().send_pending_configure();
                }
            }
        }
        self.space.map_element(window.clone(), (0, 0), true);
        let toplevel = window.toplevel().unwrap();
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        let surface = toplevel.wl_surface().clone();
        self.shown = Some(window);
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        self.move_pointer(self.pointer_pos);
        self.dirty = true;
    }

    fn move_pointer(&mut self, pos: Point<f64, Logical>) {
        self.pointer_pos = pos;
        let under = self.surface_under(pos);
        let pointer = self.seat.get_pointer().unwrap();
        let event = MotionEvent {
            location: pos,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.now(),
        };
        pointer.motion(self, under, &event);
        pointer.frame(self);
    }

    fn describe_windows(&self) -> serde_json::Value {
        let list: Vec<_> = self
            .windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let surface = w.toplevel().unwrap().wl_surface();
                let (app_id, title) = with_states(surface, |states| {
                    let attrs = states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    (attrs.app_id.clone(), attrs.title.clone())
                });
                let geo = w.geometry();
                json!({
                    "n": i,
                    "shown": self.shown.as_ref() == Some(w),
                    "app_id": app_id,
                    "title": title,
                    "size": [geo.size.w, geo.size.h],
                })
            })
            .collect();
        json!({ "ok": true, "windows": list, "popups": self.popup_count() })
    }

    fn popup_count(&self) -> usize {
        self.shown
            .as_ref()
            .map(|w| PopupManager::popups_for_surface(w.toplevel().unwrap().wl_surface()).count())
            .unwrap_or(0)
    }

    fn handle(&mut self, cmd: Cmd) -> Reply {
        let ok = || Reply::Json(json!({ "ok": true }));
        match cmd {
            Cmd::Move(x, y) => {
                let pos = Point::from((x.clamp(0.0, W as f64 - 1.0), y.clamp(0.0, H as f64 - 1.0)));
                self.move_pointer(pos);
                ok()
            }
            Cmd::Button(button, pressed) => {
                let pointer = self.seat.get_pointer().unwrap();
                let event = ButtonEvent {
                    button,
                    state: if pressed { ButtonState::Pressed } else { ButtonState::Released },
                    serial: SERIAL_COUNTER.next_serial(),
                    time: self.now(),
                };
                pointer.button(self, &event);
                pointer.frame(self);
                ok()
            }
            Cmd::Scroll(dx, dy) => {
                let mut frame = AxisFrame::new(self.now()).source(AxisSource::Wheel);
                if dx != 0 {
                    frame = frame
                        .value(Axis::Horizontal, dx as f64 * 15.0)
                        .v120(Axis::Horizontal, dx * 120);
                }
                if dy != 0 {
                    frame = frame
                        .value(Axis::Vertical, dy as f64 * 15.0)
                        .v120(Axis::Vertical, dy * 120);
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
                ok()
            }
            Cmd::Key(evdev, pressed) => {
                let keyboard = self.seat.get_keyboard().unwrap();
                let time = self.now();
                keyboard.input::<(), _>(
                    self,
                    (evdev + 8).into(),
                    if pressed { KeyState::Pressed } else { KeyState::Released },
                    SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
                ok()
            }
            Cmd::Shot => {
                let t = Instant::now();
                match self.render(true) {
                    Ok(Some(data)) => Reply::Pixels {
                        data,
                        capture_us: t.elapsed().as_micros() as u64,
                    },
                    Ok(None) => Reply::Json(json!({ "ok": false, "error": "no pixels" })),
                    Err(e) => Reply::Json(json!({ "ok": false, "error": e })),
                }
            }
            Cmd::Frame => {
                // A live view asks for the current frame. Read back only when it changed.
                let mut error = None;
                if self.dirty || self.render_seq != self.fb_seq || self.fb_seq == 0 {
                    match self.render(true) {
                        Ok(Some(data)) => {
                            self.fb[..data.len()].copy_from_slice(&data);
                            self.fb_seq = self.render_seq.max(1);
                            self.render_seq = self.fb_seq;
                        }
                        Ok(None) => {}
                        Err(e) => error = Some(e),
                    }
                }
                Reply::Json(json!({ "ok": error.is_none(), "seq": self.fb_seq, "error": error }))
            }
            Cmd::List => Reply::Json(self.describe_windows()),
            Cmd::Focus(n) => match self.windows.get(n).cloned() {
                Some(w) => {
                    self.show(w);
                    ok()
                }
                None => Reply::Json(json!({ "ok": false, "error": "no such window" })),
            },
            Cmd::Close(n) => match self.windows.get(n) {
                Some(w) => {
                    w.toplevel().unwrap().send_close();
                    ok()
                }
                None => Reply::Json(json!({ "ok": false, "error": "no such window" })),
            },
            Cmd::Pos => Reply::Json(json!({
                "ok": true, "x": self.pointer_pos.x, "y": self.pointer_pos.y,
            })),
            Cmd::Stats => Reply::Json(json!({
                "ok": true,
                "renders": self.renders,
                "render_seq": self.render_seq,
                "uptime_ms": self.start.elapsed().as_millis() as u64,
                "renderer": match self.backend { Backend::Gles { .. } => "gles", Backend::Pixman { .. } => "pixman" },
                "dmabuf": self.dmabuf.is_some(),
                "commits_shm": self.commits[0],
                "commits_dmabuf": self.commits[1],
                "commits_other": self.commits[2],
            })),
            Cmd::Quit => {
                self.loop_signal.stop();
                ok()
            }
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for(&root) else {
            return;
        };
        let window_loc = self.space.element_location(window).unwrap_or_default();
        // The target rectangle is the whole screen, relative to the popup's parent.
        let mut target = Rectangle::from_size((W, H).into());
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Smithay handlers
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }
    fn commit(&mut self, surface: &WlSurface) {
        let kind = with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            match &attrs.current().buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer_type(buffer)),
                _ => None,
            }
        });
        match kind {
            Some(Some(BufferType::Shm)) => self.commits[0] += 1,
            Some(Some(BufferType::Dma)) => self.commits[1] += 1,
            Some(_) => self.commits[2] += 1,
            None => {}
        }
        on_commit_buffer_handler::<Self>(surface);
        self.dirty = true;
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for(&root) {
                window.on_commit();
            }
        }
        if let Some(window) = self.window_for(surface) {
            let toplevel = window.toplevel().unwrap();
            if !toplevel.is_initial_configure_sent() {
                toplevel.send_configure();
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                let _ = xdg.send_configure();
            }
        }
    }
}
impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_compositor!(State);
delegate_shm!(State);

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Every toplevel is the size of the whole screen, no decorations, no gaps.
        surface.with_pending_state(|state| {
            state.size = Some((W, H).into());
            for s in [
                xdg_toplevel::State::Maximized,
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
                xdg_toplevel::State::Activated,
            ] {
                state.states.set(s);
            }
        });
        let window = Window::new_wayland_window(surface);
        self.windows.push(window.clone());
        self.show(window);
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(index) = self
            .windows
            .iter()
            .position(|w| w.toplevel().unwrap().wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let window = self.windows.remove(index);
        self.space.unmap_elem(&window);
        if self.shown.as_ref() == Some(&window) {
            self.shown = None;
            if let Some(next) = self.windows.last().cloned() {
                self.show(next);
            }
        }
        self.dirty = true;
    }
    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        debug(format_args!("new popup, geometry {:?}", surface.with_pending_state(|s| s.geometry)));
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }
    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        debug(format_args!("popup destroyed"));
        self.dirty = true;
    }
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<Self> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let mut grab = match self.popups.grab_popup(root, kind, &seat, serial) {
            Ok(grab) => grab,
            Err(e) => {
                debug(format_args!("popup grab refused: {e:?}"));
                return;
            }
        };
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                debug(format_args!("popup grab dropped: keyboard grabbed by something else"));
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                debug(format_args!("popup grab dropped: pointer grabbed by something else"));
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }
}
delegate_xdg_shell!(State);

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}
delegate_xdg_decoration!(State);

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.dh;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}
delegate_seat!(State);

impl SelectionHandler for State {
    type SelectionUserData = ();
}
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
delegate_data_device!(State);

impl OutputHandler for State {}
delegate_output!(State);
delegate_viewporter!(State);
delegate_single_pixel_buffer!(State);

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf.as_mut().unwrap().0
    }
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let imported = match &mut self.backend {
            Backend::Gles { renderer, .. } => renderer.import_dmabuf(&dmabuf, None).is_ok(),
            Backend::Pixman { renderer, .. } => renderer.import_dmabuf(&dmabuf, None).is_ok(),
        };
        if imported {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State);

// ---------------------------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------------------------

struct Args {
    name: String,
    renderer: String,
    render_node: String,
    pixman_dmabuf: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        name: "hc-0".into(),
        renderer: "gles".into(),
        render_node: "/dev/dri/renderD128".into(),
        pixman_dmabuf: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => args.name = it.next().expect("--name NAME"),
            "--renderer" => args.renderer = it.next().expect("--renderer gles|pixman"),
            "--render-node" => args.render_node = it.next().expect("--render-node PATH"),
            "--pixman-dmabuf" => args.pixman_dmabuf = true,
            other => panic!("unknown argument {other}"),
        }
    }
    args
}

fn init_gles(node: &str) -> Result<(GlesRenderer, GlesTexture), Box<dyn std::error::Error>> {
    let file = std::fs::OpenOptions::new().read(true).write(true).open(node)?;
    let gbm = GbmDevice::new(file)?;
    let display = unsafe { EGLDisplay::new(gbm)? };
    let context = EGLContext::new(&display)?;
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (W, H).into())?;
    Ok((renderer, target))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();
    let runtime = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;
    let dh = display.handle();

    // --- renderer ---
    let mut dmabuf_formats = None;
    let backend = match args.renderer.as_str() {
        "gles" => {
            let (renderer, target) = init_gles(&args.render_node)?;
            dmabuf_formats = Some(renderer.dmabuf_formats());
            Backend::Gles { renderer, target }
        }
        "pixman" => {
            let mut renderer = PixmanRenderer::new()?;
            let target: pixman::Image<'static, 'static> =
                renderer.create_buffer(Fourcc::Argb8888, (W, H).into())?;
            if args.pixman_dmabuf {
                // Clients may render with the GPU; we map their (linear) buffers and composite in software.
                dmabuf_formats = Some(renderer.dmabuf_formats());
            }
            Backend::Pixman { renderer, target }
        }
        other => panic!("unknown renderer {other}"),
    };

    // --- protocol globals ---
    let compositor_state = CompositorState::new::<State>(&dh);
    let xdg_shell_state = XdgShellState::new::<State>(&dh);
    let xdg_decoration_state = XdgDecorationState::new::<State>(&dh);
    let shm_state = ShmState::new::<State>(&dh, vec![]);
    let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
    let viewporter_state = ViewporterState::new::<State>(&dh);
    let data_device_state = DataDeviceState::new::<State>(&dh);
    let single_pixel_state = SinglePixelBufferState::new::<State>(&dh);
    let mut seat_state = SeatState::new();
    let mut seat: Seat<State> = seat_state.new_wl_seat(&dh, "agent");
    seat.add_keyboard(XkbConfig { layout: "us", ..XkbConfig::default() }, 400, 25)?;
    seat.add_pointer();

    let dmabuf = match dmabuf_formats {
        Some(formats) => {
            let node = DrmNode::from_path(&args.render_node)?;
            let feedback = DmabufFeedbackBuilder::new(node.dev_id(), formats).build()?;
            let mut state = DmabufState::new();
            let global = state.create_global_with_default_feedback::<State>(&dh, &feedback);
            Some((state, global))
        }
        None => None,
    };

    let output = Output::new(
        "AGENT".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "hypr-computer".into(),
            model: "prototype".into(),
        },
    );
    let mode = Mode { size: (W, H).into(), refresh: 60_000 };
    let _output_global = output.create_global::<State>(&dh);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(smithay::output::Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    let mut space = Space::default();
    space.map_output(&output, (0, 0));
    let tracker = OutputDamageTracker::from_output(&output);

    // --- the frame file a live view reads ---
    let fb_path = format!("{runtime}/{}.fb", args.name);
    let fb_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&fb_path)?;
    fb_file.set_len((W * H * 4) as u64)?;
    let fb = unsafe { memmap2::MmapMut::map_mut(&fb_file)? };

    // --- sockets ---
    let listening = ListeningSocketSource::with_name(&args.name)?;
    event_loop
        .handle()
        .insert_source(listening, |stream, _, state: &mut State| {
            let _ = state.dh.insert_client(stream, Arc::new(ClientState::default()));
        })?;
    event_loop.handle().insert_source(
        Generic::new(display, Interest::READ, CalloopMode::Level),
        |_, display, state| {
            unsafe { display.get_mut().dispatch_clients(state).unwrap() };
            Ok(PostAction::Continue)
        },
    )?;

    let ctl_path = format!("{runtime}/{}.ctl", args.name);
    let _ = std::fs::remove_file(&ctl_path);
    let listener = UnixListener::bind(&ctl_path)?;
    let (tx, rx) = channel::channel::<Req>();
    std::thread::spawn(move || control::serve(listener, tx));
    event_loop.handle().insert_source(rx, |event, _, state| {
        if let channel::Event::Msg(req) = event {
            let reply = state.handle(req.cmd);
            let _ = req.reply.send(reply);
        }
    })?;

    event_loop.handle().insert_source(
        Timer::from_duration(Duration::from_millis(16)),
        |_, _, state| {
            state.tick();
            TimeoutAction::ToDuration(Duration::from_millis(16))
        },
    )?;

    let mut state = State {
        start: Instant::now(),
        dh,
        loop_signal: event_loop.get_signal(),
        space,
        output,
        windows: Vec::new(),
        shown: None,
        popups: PopupManager::default(),
        seat,
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        data_device_state,
        dmabuf,
        _output_manager_state: output_manager_state,
        _xdg_decoration_state: xdg_decoration_state,
        _viewporter_state: viewporter_state,
        _single_pixel_state: single_pixel_state,
        commits: [0; 3],
        backend,
        tracker,
        dirty: true,
        renders: 0,
        render_seq: 0,
        fb_seq: 0,
        fb,
        pointer_pos: (W as f64 / 2.0, H as f64 / 2.0).into(),
    };

    println!(
        "{}",
        json!({ "ready": true, "wayland_display": args.name, "control": ctl_path, "frame_file": fb_path,
                "renderer": args.renderer, "dmabuf": state.dmabuf.is_some(), "pid": std::process::id() })
    );

    event_loop.run(None, &mut state, |state| {
        let _ = state.dh.flush_clients();
    })?;

    let _ = std::fs::remove_file(&ctl_path);
    let _ = std::fs::remove_file(&fb_path);
    Ok(())
}
