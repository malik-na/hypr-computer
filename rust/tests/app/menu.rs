//! A menu the app opens on a right click: a popup next to the pointer that
//! asks for the keyboard and pointer (an `xdg_popup` grab), as app menus do.

use serde_json::json;
use wayland_client::protocol::wl_surface;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface};

use crate::App;
use crate::window::paint;

const SIZE: (i32, i32) = (200, 100);
const COLOUR: [u8; 3] = [0xf0, 0xf0, 0xf0];

pub struct Menu {
    pub surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    popup: xdg_popup::XdgPopup,
}

/// Tells the menu's `xdg_surface` from the window's.
pub struct MenuSurface;

impl App {
    /// Opens the menu where the pointer is, asking for a grab with the
    /// `serial` of the click that opened it.
    pub fn open_menu(&mut self, qh: &QueueHandle<App>, serial: u32) {
        let (Some(seat), None) = (&self.wl_seat, &self.menu) else {
            return;
        };
        let window = &self.window;
        let surface = window.compositor.create_surface(qh, ());
        let xdg = window.wm.get_xdg_surface(&surface, qh, MenuSurface);
        let positioner = window.wm.create_positioner(qh, ());
        let (x, y) = self.pointer.at();
        positioner.set_size(SIZE.0, SIZE.1);
        positioner.set_anchor_rect(x as i32, y as i32, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::BottomRight);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        let parent = window.xdg.as_ref().map(|(parent, _)| parent);
        let popup = xdg.get_popup(parent, &positioner, qh, ());
        positioner.destroy();
        popup.grab(seat, serial);
        surface.commit();
        self.menu = Some(Menu {
            surface,
            xdg,
            popup,
        });
        self.note(&json!({"event": "menu", "state": "open"}));
    }

    fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            menu.popup.destroy();
            menu.xdg.destroy();
            menu.surface.destroy();
            self.note(&json!({"event": "menu", "state": "closed"}));
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, MenuSurface> for App {
    fn event(
        app: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &MenuSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && let Some(menu) = &app.menu
        {
            xdg.ack_configure(serial);
            let painted = paint(&app.window.shm, &menu.surface, SIZE, COLOUR, COLOUR, qh);
            if let Err(e) = painted {
                app.note(&json!({"event": "error", "error": e.to_string()}));
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            app.close_menu();
        }
    }
}

delegate_noop!(App: xdg_positioner::XdgPositioner);
