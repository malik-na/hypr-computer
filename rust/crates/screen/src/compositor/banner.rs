//! The banner a live view shows while the user has the screen: a bright
//! frame along the picture's edges. It is drawn on the view only, in
//! surfaces of its own above the picture, so the agent's screenshots never
//! show it, and it takes no input: the user's pointer goes through it.

use wayland_client::QueueHandle;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};

use crate::state::State;

/// How thick the frame is, in the view's pixels.
const EDGE: i32 = 6;

/// The frame's colour, in the bytes of one Xrgb8888 pixel: B, G, R, unused.
pub const COLOUR: [u8; 4] = [0x00, 0x8c, 0xff, 0xff];

/// One edge of the frame.
struct Strip {
    surface: wl_surface::WlSurface,
    viewport: wp_viewport::WpViewport,
    placement: wl_subsurface::WlSubsurface,
}

pub struct Banner {
    strips: [Strip; 4],
    /// One pixel of [`COLOUR`], stretched over each strip.
    colour: wl_buffer::WlBuffer,
    /// Where the picture is in the window, and its size.
    picture: ((i32, i32), (i32, i32)),
    shown: bool,
}

impl Banner {
    /// A banner over `window`'s subsurfaces made so far, hidden.
    pub fn new(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: &wl_subcompositor::WlSubcompositor,
        viewporter: &wp_viewporter::WpViewporter,
        window: &wl_surface::WlSurface,
        colour: wl_buffer::WlBuffer,
        qh: &QueueHandle<State>,
    ) -> Self {
        let strip = || {
            let surface = compositor.create_surface(qh, ());
            let viewport = viewporter.get_viewport(&surface, qh, ());
            let placement = subcompositor.get_subsurface(&surface, window, qh, ());
            placement.set_desync();
            let nothing = compositor.create_region(qh, ());
            surface.set_input_region(Some(&nothing));
            nothing.destroy();
            Strip {
                surface,
                viewport,
                placement,
            }
        };
        Self {
            strips: [strip(), strip(), strip(), strip()],
            colour,
            picture: ((0, 0), (1, 1)),
            shown: false,
        }
    }

    /// Follows the picture to `picture`, its place and size in the window.
    /// The strips move with the window's next commit.
    pub fn lay_out(&mut self, picture: ((i32, i32), (i32, i32))) {
        self.picture = picture;
        let ((x, y), (w, h)) = picture;
        let edge = EDGE.min(w / 2).min(h / 2).max(1);
        let rects = [
            (x, y, w, edge),
            (x, y + h - edge, w, edge),
            (x, y + edge, edge, (h - 2 * edge).max(1)),
            (x + w - edge, y + edge, edge, (h - 2 * edge).max(1)),
        ];
        for (strip, (x, y, w, h)) in self.strips.iter().zip(rects) {
            strip.placement.set_position(x, y);
            strip.viewport.set_destination(w, h);
        }
        self.draw();
    }

    pub fn show(&mut self, shown: bool) {
        if self.shown != shown {
            self.shown = shown;
            self.draw();
        }
    }

    fn draw(&self) {
        for strip in &self.strips {
            if self.shown {
                strip.surface.attach(Some(&self.colour), 0, 0);
                strip.surface.damage_buffer(0, 0, 1, 1);
            } else {
                strip.surface.attach(None, 0, 0);
            }
            strip.surface.commit();
        }
    }
}

impl State {
    /// Shows or hides the view's banner, if there is a view.
    pub fn show_banner(&mut self, shown: bool) {
        if let Some(view) = &mut self.view {
            view.banner.show(shown);
        }
        self.flush_view();
    }
}
