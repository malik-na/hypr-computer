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
//! keys: clicking a window gives it the keyboard, as Hyprland does, and keys
//! go to the window with the keyboard. The host has the user's clipboard:
//! the test can copy to it, as the user does in one of their own apps, and
//! read what a paste on the desktop would get.

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
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
use smithay::desktop::{Window, WindowSurfaceType};
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
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_focus, set_data_device_selection,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
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
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_viewporter, delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
};

use super::hyprland::World;

/// The size Hyprland gives each window here.
pub const TILE: (i32, i32) = (1280, 800);
const FRAME: Duration = Duration::from_millis(16);
/// The text types the user's clipboard offers, and asks a window for.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// A pointer button, as evdev numbers it.
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

/// The evdev code of a key the tests press, by its xkb name.
fn key_code(name: &str) -> Result<u32, String> {
    let code = match name {
        "Escape" => 1,
        "Return" => 28,
        "Tab" => 15,
        "space" => 57,
        "BackSpace" => 14,
        "ctrl" | "Control_L" => 29,
        "shift" | "Shift_L" => 42,
        "alt" | "Alt_L" => 56,
        "a" => 30,
        "b" => 48,
        "c" => 46,
        "d" => 32,
        "e" => 18,
        "h" => 35,
        "i" => 23,
        "k" => 37,
        "o" => 24,
        "q" => 16,
        "v" => 47,
        "x" => 45,
        "z" => 44,
        "1" => 2,
        "2" => 3,
        "3" => 4,
        _ => return Err(format!("the host doesn't know the key {name:?}")),
    };
    Ok(code)
}

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
    UserInput(String, bool, mpsc::Sender<Result<(), String>>),
    Click(String, (f64, f64), u32, mpsc::Sender<Result<(), String>>),
    Hover(String, (f64, f64), mpsc::Sender<Result<(), String>>),
    Keys(String, Vec<String>, mpsc::Sender<Result<(), String>>),
    Copy(String, mpsc::Sender<()>),
    Paste(mpsc::Sender<Result<Pasted, String>>),
    Stop,
}

/// What a paste on the desktop gets: the text the test copied, or a pipe
/// that the window owning the clipboard writes it into.
enum Pasted {
    Nothing,
    Text(String),
    From(std::io::PipeReader),
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
        self.ask(|tx| Command::UserInput(address.to_owned(), true, tx))
    }

    /// As [`Host::user_input`], but without clicking: what a user does to a
    /// view they only watch.
    pub fn user_input_without_clicking(&self, address: &str) -> Result<(), String> {
        self.ask(|tx| Command::UserInput(address.to_owned(), false, tx))
    }

    /// The user clicks `button` at `at` in the window (its own coordinates):
    /// the pointer moves there, the window gets the keyboard, as Hyprland's
    /// click to focus gives it, and the button goes down and up. The window
    /// must be on the workspace shown.
    pub fn click(&self, address: &str, at: (f64, f64), button: u32) -> Result<(), String> {
        self.ask(|tx| Command::Click(address.to_owned(), at, button, tx))
    }

    /// The user moves the pointer to `at` in the window, without clicking.
    pub fn hover(&self, address: &str, at: (f64, f64)) -> Result<(), String> {
        self.ask(|tx| Command::Hover(address.to_owned(), at, tx))
    }

    /// The user presses each of `keys` in turn: a key's xkb name, or a
    /// combination like `ctrl+v`. Keys go to the window with the keyboard,
    /// which must be the one at `address`.
    pub fn keys(&self, address: &str, keys: &[&str]) -> Result<(), String> {
        let keys = keys.iter().map(|k| (*k).to_owned()).collect();
        self.ask(|tx| Command::Keys(address.to_owned(), keys, tx))
    }

    /// The user copies `text` in one of their own apps.
    pub fn copy(&self, text: &str) {
        self.ask(|tx| Command::Copy(text.to_owned(), tx));
    }

    /// What a paste on the user's desktop gets now: `None` if the clipboard
    /// is empty, or its owner doesn't answer.
    pub fn paste(&self) -> Option<String> {
        let pasted = self
            .ask(Command::Paste)
            .unwrap_or_else(|e| panic!("can't paste on the desktop: {e}"));
        match pasted {
            Pasted::Nothing => None,
            Pasted::Text(text) => Some(text),
            Pasted::From(mut pipe) => {
                let (tx, rx) = mpsc::channel();
                thread::spawn(move || {
                    let mut text = String::new();
                    let read = pipe.read_to_string(&mut text);
                    let _ = tx.send(read.map(|_| text));
                });
                rx.recv_timeout(Duration::from_secs(5)).ok()?.ok()
            }
        }
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
    data_device_state: DataDeviceState,
    /// The text types of the clipboard a window set, if one did.
    clipboard_types: Vec<String>,
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
        data_device_state: DataDeviceState::new::<State>(&dh),
        clipboard_types: Vec::new(),
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
            Command::UserInput(address, clicks, reply) => {
                let _ = reply.send(self.user_input(&address, clicks));
            }
            Command::Click(address, at, button, reply) => {
                let _ = reply.send(self.click(&address, at, button));
            }
            Command::Hover(address, at, reply) => {
                let _ = reply.send(self.hover(&address, at));
            }
            Command::Keys(address, keys, reply) => {
                let _ = reply.send(self.press_keys(&address, &keys));
            }
            Command::Copy(text, reply) => {
                let types = TEXT_TYPES.iter().map(|t| (*t).to_owned()).collect();
                set_data_device_selection(&self.display, &self.seat, types, text);
                self.clipboard_types.clear();
                let _ = reply.send(());
            }
            Command::Paste(reply) => {
                let _ = reply.send(self.paste());
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

    fn user_input(&mut self, address: &str, clicks: bool) -> Result<(), String> {
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
        let buttons: &[u32] = if clicks { &[0x110, 0x111, 0x110] } else { &[] };
        for &button in buttons {
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

    /// Moves the pointer to `at` in the window, onto the surface there.
    fn pointer_to(&mut self, address: &str, at: (f64, f64)) -> Result<(), String> {
        let w = self.win(address)?;
        if !self.world.lock().unwrap().is_visible(address) {
            return Err(format!("{address} isn't on the workspace shown"));
        }
        let geometry = w.window.geometry();
        let point = (
            at.0 + f64::from(geometry.loc.x),
            at.1 + f64::from(geometry.loc.y),
        );
        let focus = w
            .window
            .surface_under(point, WindowSurfaceType::ALL)
            .map(|(surface, loc)| (surface, (loc - geometry.loc).to_f64()));
        if focus.is_none() {
            return Err(format!("nothing of {address} is at {at:?}"));
        }
        let pointer = self.seat.get_pointer().ok_or("no pointer")?;
        let event = MotionEvent {
            location: at.into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: self.time(),
        };
        pointer.motion(self, focus, &event);
        pointer.frame(self);
        Ok(())
    }

    fn hover(&mut self, address: &str, at: (f64, f64)) -> Result<(), String> {
        self.pointer_to(address, at)
    }

    fn click(&mut self, address: &str, at: (f64, f64), button: u32) -> Result<(), String> {
        self.pointer_to(address, at)?;
        if self.world.lock().unwrap().user_clicks(address) {
            let surface = self.win(address)?.toplevel.wl_surface().clone();
            let keyboard = self.seat.get_keyboard().ok_or("no keyboard")?;
            if keyboard.current_focus().as_ref() != Some(&surface) {
                keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
            }
        }
        let pointer = self.seat.get_pointer().ok_or("no pointer")?;
        for state in [ButtonState::Pressed, ButtonState::Released] {
            let event = ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: self.time(),
                button,
                state,
            };
            pointer.button(self, &event);
            pointer.frame(self);
        }
        Ok(())
    }

    fn press_keys(&mut self, address: &str, keys: &[String]) -> Result<(), String> {
        let surface = self.win(address)?.toplevel.wl_surface().clone();
        let keyboard = self.seat.get_keyboard().ok_or("no keyboard")?;
        if keyboard.current_focus().as_ref() != Some(&surface) {
            return Err(format!(
                "{address} doesn't have the keyboard: click it first"
            ));
        }
        for combo in keys {
            let codes = combo
                .split('+')
                .map(key_code)
                .collect::<Result<Vec<_>, _>>()?;
            let order = codes.iter().map(|c| (*c, KeyState::Pressed));
            let back = codes.iter().rev().map(|c| (*c, KeyState::Released));
            for (code, state) in order.chain(back) {
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
        }
        Ok(())
    }

    fn paste(&mut self) -> Result<Pasted, String> {
        if let Some(text) = current_data_device_selection_userdata(&self.seat) {
            return Ok(Pasted::Text(text.clone()));
        }
        let Some(mime) = TEXT_TYPES
            .iter()
            .find(|t| self.clipboard_types.iter().any(|c| c == *t))
        else {
            return Ok(Pasted::Nothing);
        };
        let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
        match request_data_device_client_selection(&self.seat, (*mime).to_owned(), writer.into()) {
            Ok(()) => {
                let _ = self.display.flush_clients();
                Ok(Pasted::From(reader))
            }
            Err(_) => Ok(Pasted::Nothing),
        }
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

    /// The window with the keyboard is the one that can paste, as on any
    /// desktop.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(Resource::client);
        set_data_device_focus(&self.display, seat, client);
    }
}

impl SelectionHandler for State {
    /// The text the test copied.
    type SelectionUserData = String;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _: Seat<Self>,
    ) {
        if ty == SelectionTarget::Clipboard {
            self.clipboard_types = source.map(|s| s.mime_types()).unwrap_or_default();
        }
    }

    fn send_selection(
        &mut self,
        _: SelectionTarget,
        _: String,
        fd: OwnedFd,
        _: Seat<Self>,
        text: &String,
    ) {
        let mut pipe = std::fs::File::from(fd);
        let _ = pipe.write_all(text.as_bytes());
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);
delegate_xdg_activation!(State);
delegate_viewporter!(State);
delegate_output!(State);
delegate_seat!(State);
delegate_data_device!(State);
