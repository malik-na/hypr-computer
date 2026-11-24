//! Everything the screen holds, and how it is set up.

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use screen::SIZE;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::Offscreen;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::desktop::{PopupManager, Space, Window};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::pixman;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::utils::Transform;
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shm::ShmState;

use crate::keymap::Keymap;
use crate::reading::Reading;
use crate::seat::new_seat;
use crate::view::View;

/// The screen's size as Smithay wants it.
pub const WIDTH: i32 = SIZE.0 as i32;
pub const HEIGHT: i32 = SIZE.1 as i32;

pub struct State {
    pub display: DisplayHandle,
    loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, State>,
    started: Instant,
    /// The user's desktop's Wayland socket, if there is one.
    pub desktop: Option<PathBuf>,
    /// The live view, once the daemon has opened it.
    pub view: Option<View>,
    /// How many views were ever opened.
    pub views_opened: u64,

    pub space: Space<Window>,
    pub output: Output,
    /// Every app window, oldest first. Only `shown` is on the screen.
    pub windows: Vec<Window>,
    pub shown: Option<Window>,
    pub popups: PopupManager,
    /// An app drew something since apps were last told to draw again.
    pub dirty: bool,
    /// How many windows have drawn their first picture.
    pub windows_drawn: u64,
    /// How many times apps have drawn, and when they last did.
    pub commits: u64,
    pub last_commit: Instant,
    /// Whether apps have read the agent's input.
    pub reading: Reading,
    /// How many renders changed what is on the screen.
    pub frame: u64,

    pub renderer: PixmanRenderer,
    /// What the screen is rendered into; kept, so a render redraws only what changed.
    pub target: pixman::Image<'static, 'static>,
    pub damage: OutputDamageTracker,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<State>,
    /// The keyboard's keymap, to look keys up in.
    pub keymap: Keymap,
    _output_manager_state: OutputManagerState,
    _xdg_decoration_state: XdgDecorationState,
}

impl State {
    pub fn new(
        display: DisplayHandle,
        loop_signal: LoopSignal,
        loop_handle: LoopHandle<'static, State>,
        desktop: Option<PathBuf>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut renderer = PixmanRenderer::new()?;
        let target = renderer.create_buffer(Fourcc::Argb8888, (WIDTH, HEIGHT).into())?;

        let output = Output::new(
            "hypr-computer".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "hypr-computer".to_owned(),
                model: "agent screen".to_owned(),
            },
        );
        let mode = Mode {
            size: (WIDTH, HEIGHT).into(),
            refresh: 60_000,
        };
        let _global = output.create_global::<State>(&display);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));
        let damage = OutputDamageTracker::from_output(&output);
        let mut seat_state = SeatState::new();
        let seat = new_seat(&mut seat_state, &display)?;

        let mut state = State {
            compositor_state: CompositorState::new::<State>(&display),
            xdg_shell_state: XdgShellState::new::<State>(&display),
            shm_state: ShmState::new::<State>(&display, vec![]),
            seat_state,
            data_device_state: DataDeviceState::new::<State>(&display),
            seat,
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(&display),
            _xdg_decoration_state: XdgDecorationState::new::<State>(&display),
            display,
            loop_signal,
            loop_handle,
            started: Instant::now(),
            desktop,
            view: None,
            views_opened: 0,
            space,
            output,
            windows: Vec::new(),
            shown: None,
            popups: PopupManager::default(),
            dirty: true,
            windows_drawn: 0,
            commits: 0,
            last_commit: Instant::now(),
            reading: Reading::default(),
            frame: 0,
            renderer,
            target,
            damage,
            keymap: Keymap::us()?,
        };
        state.send_keymap()?;
        Ok(state)
    }

    pub fn stop(&self) {
        self.loop_signal.stop();
    }

    /// The time for an input event, in milliseconds.
    pub fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// Lets the shown app draw its next frame, if it drew since the last one.
    pub fn frame_tick(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(window) = &self.shown {
            let output = &self.output;
            window.send_frame(
                output,
                self.started.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }
        self.space.refresh();
        self.popups.cleanup();
        self.draw_view();
    }
}

/// What the screen keeps for each app connected to it.
#[derive(Default)]
pub struct ClientState {
    pub compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
