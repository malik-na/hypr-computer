//! The Wayland protocols the screen speaks, as Smithay handlers: surfaces and
//! shared-memory buffers, xdg-shell windows and popups, server-side
//! decorations (that is, none drawn), and the output. The seat is in
//! `seat.rs`.

use std::sync::LazyLock;

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{PopupKind, find_popup_root_surface, get_popup_toplevel_coords};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface::WlSurface};
use smithay::utils::{Rectangle, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_output, delegate_shm, delegate_xdg_decoration,
    delegate_xdg_shell,
};

use crate::state::{ClientState, HEIGHT, State, WIDTH};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Every client is inserted with a ClientState, so the fallback is never used.
        static NONE: LazyLock<CompositorClientState> = LazyLock::new(Default::default);
        match client.get_data::<ClientState>() {
            Some(data) => &data.compositor,
            None => &NONE,
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
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
        if let Some(toplevel) = self.window_for(surface).and_then(|w| w.toplevel())
            && !toplevel.is_initial_configure_sent()
        {
            toplevel.send_configure();
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            // Fails only if the popup's parent is gone, and then so is the popup.
            let _ = popup.send_configure();
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

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_window(surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(&surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.keep_on_screen(&surface);
        // Fails only if the popup's parent is gone, and then so is the popup.
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.dirty = true;
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.keep_on_screen(&surface);
        surface.send_repositioned(token);
    }

    /// Popup grabs take input, and the screen has none yet.
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

impl State {
    /// Moves `popup` so it fits on the screen, as far as its positioner allows.
    fn keep_on_screen(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Some(window) = find_popup_root_surface(&kind)
            .ok()
            .and_then(|root| self.window_for(&root))
        else {
            return;
        };
        // The whole screen, relative to the popup's parent.
        let mut target = Rectangle::from_size((WIDTH, HEIGHT).into());
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= self.space.element_location(window).unwrap_or_default();
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
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

impl OutputHandler for State {}

delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);
delegate_output!(State);
