//! The user's desktop as a Wayland compositor, for the live views to open
//! on: a tiny Smithay compositor run on a thread of the test.
//!
//! It behaves as Hyprland does towards a window, as far as a view can see:
//! - every window is tiled, 1280x800 (the size of one tile of the monitor);
//! - a window maps on its first buffer, and the fake Hyprland then decides its
//!   workspace and focus by its rules;
//! - a window on a workspace that isn't shown gets no frame callbacks and is
//!   told it is suspended, until its workspace is shown again;
//! - a window asking to be activated (xdg-activation) is reported to the fake.
//!
//! The test can read what a window shows, and send it the user's pointer and
//! keys.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::desktop::Window;
use smithay::input::keyboard::{FilterResult, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopSignal, PostAction, channel};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, ListeningSocket, Resource};
use smithay::utils::{Rectangle, SERIAL_COUNTER, Serial, Transform};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
    with_states,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use smithay::{
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_viewporter,
    delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
};

use super::hyprland::World;

/// The size Hyprland gives each window here.
pub const TILE: (i32, i32) = (1280, 800);
const FRAME: Duration = Duration::from_millis(16);

/// A window on the host, as the test sees it.
#[derive(Clone, Debug)]
pub struct HostWindow {
    /// Its Hyprland address, once it mapped.
    pub address: Option<String>,
    pub title: String,
    pub app_id: String,
    /// The pid of the client that opened it.
    pub pid: Option<i32>,
    /// Told it is suspended (its workspace isn't shown).
    pub suspended: bool,
}

/// What the window shows, as 8-bit RGB.
#[derive(Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Picture {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

enum Command {
    Windows(mpsc::Sender<Vec<HostWindow>>),
    Capture(String, mpsc::Sender<Result<Picture, String>>),
    UserInput(String, mpsc::Sender<Result<(), String>>),
    Stop,
}

pub struct Host {
    commands: channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    socket: PathBuf,
}

impl Host {
    /// Listens at `socket`; reports windows to the fake Hyprland's `world`.
    pub fn start(socket: &Path, world: Arc<Mutex<World>>) -> Self {
        let (commands, from_test) = channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let path = socket.to_owned();
        let thread = thread::spawn(move || run(path, world, from_test, ready_tx));
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the host compositor didn't start")
            .unwrap_or_else(|e| panic!("the host compositor didn't start: {e}"));
        Host {
            commands,
            thread: Some(thread),
            socket: socket.to_owned(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn ask<T>(&self, make: impl FnOnce(mpsc::Sender<T>) -> Command) -> T {
        let (tx, rx) = mpsc::channel();
        self.commands
            .send(make(tx))
            .expect("the host compositor stopped");
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the host compositor didn't answer")
    }

    /// Every window open on the host.
    pub fn windows(&self) -> Vec<HostWindow> {
        self.ask(Command::Windows)
    }

    /// What the window at `address` shows now.
    pub fn capture(&self, address: &str) -> Result<Picture, String> {
        self.ask(|tx| Command::Capture(address.to_owned(), tx))
    }

    /// The user moves the pointer over the window, clicks, scrolls, and types
    /// into it.
    pub fn user_input(&self, address: &str) -> Result<(), String> {
        self.ask(|tx| Command::UserInput(address.to_owned(), tx))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Win {
    window: Window,
    toplevel: ToplevelSurface,
    address: Option<String>,
    pid: Option<i32>,
    suspended: bool,
}

struct State {
    display: DisplayHandle,
    signal: LoopSignal,
    started: Instant,
    world: Arc<Mutex<World>>,
    windows: Vec<Win>,
    output: Output,
    renderer: PixmanRenderer,
    seat: Seat<State>,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<State>,
    activation_state: XdgActivationState,
    _output_manager_state: OutputManagerState,
    _decoration_state: XdgDecorationState,
    _viewporter_state: ViewporterState,
}

#[derive(Default)]
struct ClientState {
    compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

fn run(
    socket: PathBuf,
    world: Arc<Mutex<World>>,
    from_test: channel::Channel<Command>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let mut event_loop: EventLoop<State> = match EventLoop::try_new() {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let mut display: Display<State> = Display::new().unwrap();
    let dh = display.handle();
    let output = Output::new(
        "FAKE-1".to_owned(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "fake".to_owned(),
            model: "monitor".to_owned(),
        },
    );
    let mode = Mode {
        size: TILE.into(),
        refresh: 60_000,
    };
    let _global = output.create_global::<State>(&dh);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    seat.add_pointer();
    seat.add_keyboard(XkbConfig::default(), 200, 25).unwrap();
    let mut state = State {
        compositor_state: CompositorState::new::<State>(&dh),
        xdg_shell_state: XdgShellState::new::<State>(&dh),
        shm_state: ShmState::new::<State>(&dh, vec![]),
        activation_state: XdgActivationState::new::<State>(&dh),
        _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(&dh),
        _decoration_state: XdgDecorationState::new::<State>(&dh),
        _viewporter_state: ViewporterState::new::<State>(&dh),
        seat_state,
        seat,
        display: dh,
        signal: event_loop.get_signal(),
        started: Instant::now(),
        world,
        windows: Vec::new(),
        output,
        renderer: PixmanRenderer::new().unwrap(),
    };
    let handle = event_loop.handle();
    let listening = match ListeningSocket::bind_absolute(socket.clone()) {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(format!("can't listen on {}: {e}", socket.display())));
            return;
        }
    };
    handle
        .insert_source(
            Generic::new(
                listening,
                Interest::READ,
                smithay::reexports::calloop::Mode::Level,
            ),
            |_, listening, state| {
                while let Some(stream) = listening.accept()? {
                    state
                        .display
                        .insert_client(stream, Arc::new(ClientState::default()))?;
                }
                Ok(PostAction::Continue)
            },
        )
        .unwrap();
    let fd = display.backend().poll_fd().try_clone_to_owned().unwrap();
    handle
        .insert_source(
            Generic::new(fd, Interest::READ, smithay::reexports::calloop::Mode::Level),
            move |_, _, state| {
                display.dispatch_clients(state)?;
                Ok(PostAction::Continue)
            },
        )
        .unwrap();
    handle
        .insert_source(from_test, |event, _, state| match event {
            channel::Event::Msg(command) => state.command(command),
            channel::Event::Closed => state.signal.stop(),
        })
        .unwrap();
    handle
        .insert_source(Timer::from_duration(FRAME), |_, _, state| {
            state.tick();
            TimeoutAction::ToDuration(FRAME)
        })
        .unwrap();
    let _ = ready.send(Ok(()));
    let _ = event_loop.run(None, &mut state, |state| {
        let _ = state.display.flush_clients();
    });
}

impl State {
    fn command(&mut self, command: Command) {
        match command {
            Command::Windows(reply) => {
                let list = self
                    .windows
                    .iter()
                    .map(|w| {
                        let (title, app_id) = names(&w.toplevel);
                        HostWindow {
                            address: w.address.clone(),
                            title,
                            app_id,
                            pid: w.pid,
                            suspended: w.suspended,
                        }
                    })
                    .collect();
                let _ = reply.send(list);
            }
            Command::Capture(address, reply) => {
                let _ = reply.send(self.capture(&address));
            }
            Command::UserInput(address, reply) => {
                let _ = reply.send(self.user_input(&address));
            }
            Command::Stop => self.signal.stop(),
        }
    }

    fn win(&self, address: &str) -> Result<&Win, String> {
        self.windows
            .iter()
            .find(|w| w.address.as_deref() == Some(address))
            .ok_or_else(|| format!("no window {address} on the host"))
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// Frame callbacks for shown windows; suspends and resumes windows as
    /// their workspace is hidden and shown.
    fn tick(&mut self) {
        let elapsed = self.started.elapsed();
        let visible: Vec<bool> = {
            let world = self.world.lock().unwrap();
            self.windows
                .iter()
                .map(|w| w.address.as_deref().is_none_or(|a| world.is_visible(a)))
                .collect()
        };
        for (w, visible) in self.windows.iter_mut().zip(visible) {
            if w.suspended == visible && w.toplevel.is_initial_configure_sent() {
                w.suspended = !visible;
                w.toplevel.with_pending_state(|s| {
                    if visible {
                        s.states.unset(xdg_toplevel::State::Suspended);
                    } else {
                        s.states.set(xdg_toplevel::State::Suspended);
                    }
                });
                w.toplevel.send_pending_configure();
            }
            if visible {
                let output = self.output.clone();
                w.window
                    .send_frame(&self.output, elapsed, Some(Duration::ZERO), |_, _| {
                        Some(output.clone())
                    });
            }
        }
    }

    fn capture(&mut self, address: &str) -> Result<Picture, String> {
        let surface = self.win(address)?.toplevel.wl_surface().clone();
        let geometry = self.win(address)?.window.geometry();
        let (w, h) = (geometry.size.w, geometry.size.h);
        if w <= 0 || h <= 0 {
            return Err(format!("{address} has drawn nothing yet"));
        }
        let elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> =
            render_elements_from_surface_tree(
                &mut self.renderer,
                &surface,
                (-geometry.loc.x, -geometry.loc.y),
                1.0,
                1.0,
                Kind::Unspecified,
            );
        let mut target = self
            .renderer
            .create_buffer(Fourcc::Argb8888, (w, h).into())
            .map_err(|e| e.to_string())?;
        let mut framebuffer = self.renderer.bind(&mut target).map_err(|e| e.to_string())?;
        let mut damage = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
        damage
            .render_output(
                &mut self.renderer,
                &mut framebuffer,
                0,
                &elements,
                [0.0, 0.0, 0.0, 1.0],
            )
            .map_err(|e| format!("{e:?}"))?;
        let mapping = self
            .renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size((w, h).into()),
                Fourcc::Abgr8888,
            )
            .map_err(|e| e.to_string())?;
        let rgba = self
            .renderer
            .map_texture(&mapping)
            .map_err(|e| e.to_string())?;
        let rgb = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|[r, g, b, _]| [*r, *g, *b])
            .collect();
        Ok(Picture {
            width: w as u32,
            height: h as u32,
            rgb,
        })
    }

    fn user_input(&mut self, address: &str) -> Result<(), String> {
        let surface = self.win(address)?.toplevel.wl_surface().clone();
        let size = self.win(address)?.window.geometry().size;
        let (w, h) = (f64::from(size.w.max(1)), f64::from(size.h.max(1)));
        let pointer = self.seat.get_pointer().ok_or("no pointer")?;
        let keyboard = self.seat.get_keyboard().ok_or("no keyboard")?;
        let focus = Some((surface.clone(), (0.0, 0.0).into()));
        for (fx, fy) in [(0.1, 0.1), (0.5, 0.5), (0.9, 0.8), (0.3, 0.7), (0.5, 0.5)] {
            let event = MotionEvent {
                location: (fx * w, fy * h).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: self.time(),
            };
            pointer.motion(self, focus.clone(), &event);
            pointer.frame(self);
        }
        for button in [0x110, 0x111, 0x110] {
            for pressed in [ButtonState::Pressed, ButtonState::Released] {
                let event = ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: self.time(),
                    button,
                    state: pressed,
                };
                pointer.button(self, &event);
                pointer.frame(self);
            }
        }
        let scroll = AxisFrame::new(self.time())
            .source(AxisSource::Wheel)
            .value(Axis::Vertical, 15.0)
            .v120(Axis::Vertical, 120);
        pointer.axis(self, scroll);
        pointer.frame(self);
        keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        // a, Return, Escape twice (evdev codes + 8).
        for code in [38u32, 36, 9, 9] {
            for pressed in [KeyState::Pressed, KeyState::Released] {
                let time = self.time();
                keyboard.input::<(), _>(
                    self,
                    code.into(),
                    pressed,
                    SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
        }
        keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        let event = MotionEvent {
            location: (-10.0, -10.0).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: self.time(),
        };
        pointer.motion(self, None, &event);
        pointer.frame(self);
        Ok(())
    }

    fn window_for(&self, surface: &WlSurface) -> Option<usize> {
        self.windows
            .iter()
            .position(|w| w.toplevel.wl_surface() == surface)
    }

    /// A window maps on its first buffer: Hyprland's rules place it.
    fn map_if_ready(&mut self, index: usize) {
        let w = &self.windows[index];
        if w.address.is_some() {
            return;
        }
        let has_buffer =
            with_renderer_surface_state(w.toplevel.wl_surface(), |s| s.buffer().is_some())
                .unwrap_or(false);
        if !has_buffer {
            return;
        }
        let (title, app_id) = names(&w.toplevel);
        let pid = w.pid.unwrap_or(-1);
        let address = self.world.lock().unwrap().open_window(pid, &app_id, &title);
        self.windows[index].address = Some(address);
    }
}

fn names(toplevel: &ToplevelSurface) -> (String, String) {
    with_states(toplevel.wl_surface(), |states| {
        let data = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();
        (
            data.title.clone().unwrap_or_default(),
            data.app_id.clone().unwrap_or_default(),
        )
    })
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        static NONE: LazyLock<CompositorClientState> = LazyLock::new(Default::default);
        match client.get_data::<ClientState>() {
            Some(data) => &data.compositor,
            None => &NONE,
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(i) = self.window_for(&root) {
                self.windows[i].window.on_commit();
            }
        }
        if let Some(i) = self.window_for(surface) {
            if !self.windows[i].toplevel.is_initial_configure_sent() {
                self.windows[i].toplevel.send_configure();
            } else {
                self.map_if_ready(i);
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

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| {
            s.size = Some(TILE.into());
            for state in [
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ] {
                s.states.set(state);
            }
        });
        let pid = toplevel
            .wl_surface()
            .client()
            .and_then(|c| c.get_credentials(&self.display).ok())
            .map(|c| c.pid);
        self.windows.push(Win {
            window: Window::new_wayland_window(toplevel.clone()),
            toplevel,
            address: None,
            pid,
            suspended: false,
        });
    }

    fn toplevel_destroyed(&mut self, toplevel: ToplevelSurface) {
        if let Some(i) = self.windows.iter().position(|w| w.toplevel == toplevel) {
            let w = self.windows.remove(i);
            if let Some(address) = w.address {
                self.world.lock().unwrap().close_window(&address);
            }
        }
    }

    fn title_changed(&mut self, toplevel: ToplevelSurface) {
        if let Some(w) = self.windows.iter().find(|w| w.toplevel == toplevel)
            && let Some(address) = &w.address
        {
            let (title, _) = names(&toplevel);
            self.world.lock().unwrap().retitle(address, &title);
        }
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ServerSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.new_decoration(toplevel.clone());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ServerSide);
    }
}

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        _token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if let Some(i) = self.window_for(&surface)
            && let Some(address) = self.windows[i].address.clone()
        {
            self.world.lock().unwrap().activate(&address);
        }
    }
}

impl OutputHandler for State {}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);
delegate_xdg_activation!(State);
delegate_viewporter!(State);
delegate_output!(State);
delegate_seat!(State);
