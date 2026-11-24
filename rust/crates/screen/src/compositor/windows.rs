//! The window policy: one app on the screen at a time, at the screen's full
//! size, with no decorations. A new window is shown at once; when the shown
//! one closes, the newest of the others comes back.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use serde_json::json;
use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};

use crate::state::{HEIGHT, State, WIDTH};

impl State {
    pub fn add_window(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.size = Some((WIDTH, HEIGHT).into());
            for s in [
                xdg_toplevel::State::Maximized,
                xdg_toplevel::State::TiledLeft,
                xdg_toplevel::State::TiledRight,
                xdg_toplevel::State::TiledTop,
                xdg_toplevel::State::TiledBottom,
            ] {
                state.states.set(s);
            }
        });
        let window = Window::new_wayland_window(toplevel);
        self.windows.push(window.clone());
        self.show(window);
    }

    pub fn remove_window(&mut self, toplevel: &ToplevelSurface) {
        let Some(index) = self
            .windows
            .iter()
            .position(|w| w.toplevel() == Some(toplevel))
        else {
            return;
        };
        let window = self.windows.remove(index);
        self.space.unmap_elem(&window);
        if self.shown.as_ref() == Some(&window) {
            self.shown = None;
            match self.windows.last().cloned() {
                Some(next) => self.show(next),
                None => self.focus_shown(),
            }
        }
        self.dirty = true;
    }

    /// Counts `surface`'s window once it has a picture.
    pub fn note_drawn(&mut self, surface: &WlSurface) {
        struct Drawn;
        let has_picture = with_renderer_surface_state(surface, |s| s.buffer().is_some());
        if has_picture == Some(true)
            && self
                .window_for(surface)
                .is_some_and(|w| w.user_data().insert_if_missing(|| Drawn))
        {
            self.windows_drawn += 1;
        }
    }

    /// Every window's app id and title, oldest first, as JSON.
    pub fn window_list(&self) -> Vec<u8> {
        let list: Vec<_> = self
            .windows
            .iter()
            .filter_map(Window::toplevel)
            .map(|toplevel| {
                let (app_id, title) = with_states(toplevel.wl_surface(), |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok())
                        .map(|data| (data.app_id.clone(), data.title.clone()))
                        .unwrap_or_default()
                });
                json!({"app_id": app_id.unwrap_or_default(), "title": title.unwrap_or_default()})
            })
            .collect();
        serde_json::to_vec(&list).unwrap_or_default()
    }

    /// The window whose main surface is `surface`.
    pub fn window_for(&self, surface: &WlSurface) -> Option<&Window> {
        self.windows
            .iter()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
    }

    fn show(&mut self, window: Window) {
        if let Some(old) = self.shown.take()
            && old != window
        {
            self.space.unmap_elem(&old);
            set_activated(&old, false);
        }
        self.space.map_element(window.clone(), (0, 0), true);
        set_activated(&window, true);
        self.shown = Some(window);
        self.focus_shown();
        self.dirty = true;
    }
}

/// Tells the app whether its window is the one on the screen.
fn set_activated(window: &Window, activated: bool) {
    window.set_activated(activated);
    if let Some(toplevel) = window.toplevel()
        && toplevel.is_initial_configure_sent()
    {
        toplevel.send_pending_configure();
    }
}
