//! Reading the screen: render what is on it, read the pixels back, and
//! encode them as a PNG.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, ExportMem};
use smithay::desktop::space::render_output;
use smithay::utils::Rectangle;

use crate::state::{HEIGHT, State, WIDTH};

/// What shows where no app has drawn.
const BACKGROUND: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

impl State {
    /// The screen as it is now, as a PNG.
    pub fn screenshot(&mut self) -> Result<Vec<u8>, String> {
        self.render()?;
        // Abgr8888 is R, G, B, A in memory.
        encode_png(&self.pixels(Fourcc::Abgr8888)?)
    }

    /// Draws what changed on the screen since the last render.
    pub fn render(&mut self) -> Result<(), String> {
        let State {
            renderer,
            target,
            output,
            space,
            damage,
            ..
        } = self;
        let mut framebuffer = renderer
            .bind(target)
            .map_err(|e| format!("can't render: {e}"))?;
        render_output::<_, WaylandSurfaceRenderElement<PixmanRenderer>, _, _>(
            output,
            renderer,
            &mut framebuffer,
            1.0,
            // The target is kept between renders, so only what changed is redrawn.
            1,
            [&*space],
            &[],
            damage,
            BACKGROUND,
        )
        .map_err(|e| format!("can't render: {e:?}"))?;
        Ok(())
    }

    /// The last render's pixels, top row first, in `format`.
    pub fn pixels(&mut self, format: Fourcc) -> Result<Vec<u8>, String> {
        let State {
            renderer, target, ..
        } = self;
        let framebuffer = renderer
            .bind(target)
            .map_err(|e| format!("can't read the screen: {e}"))?;
        let mapping = renderer
            .copy_framebuffer(
                &framebuffer,
                Rectangle::from_size((WIDTH, HEIGHT).into()),
                format,
            )
            .map_err(|e| format!("can't read the screen: {e}"))?;
        renderer
            .map_texture(&mapping)
            .map(<[u8]>::to_vec)
            .map_err(|e| format!("can't read the screen: {e}"))
    }
}

/// Encodes a whole screen of RGBA pixels as an RGB PNG: the screen is opaque,
/// so alpha would only add bytes.
fn encode_png(rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (width, height) = screen::SIZE;
    let pixels = (width * height) as usize;
    let rgb: Vec<u8> = rgba
        .get(..pixels * 4)
        .ok_or_else(|| {
            format!(
                "read {} bytes of the screen, not {}",
                rgba.len(),
                pixels * 4
            )
        })?
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, _]| [*r, *g, *b])
        .collect();
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("can't encode the screenshot: {e}"))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| format!("can't encode the screenshot: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("can't encode the screenshot: {e}"))?;
    Ok(png)
}
