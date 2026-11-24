//! A screenshot as the agent receives it: a PNG in an MCP image block.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

/// A decoded screenshot, as 8-bit RGB.
#[derive(PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    rgb: Vec<u8>,
}

impl Image {
    /// Decodes the one image block of a `tools/call` result.
    pub fn from_result(result: &Value) -> Self {
        let content = result["content"].as_array().expect("content");
        assert_eq!(content.len(), 1, "one image and nothing else: {result}");
        let block = &content[0];
        assert_eq!(block["type"], "image", "{block}");
        assert_eq!(block["mimeType"], "image/png", "{block}");
        let png = STANDARD
            .decode(block["data"].as_str().expect("data"))
            .expect("base64");
        Self::from_png(&png)
    }

    pub fn from_png(png: &[u8]) -> Self {
        let mut reader = png::Decoder::new(png).read_info().expect("png");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("png frame");
        buf.truncate(info.buffer_size());
        let rgb = match info.color_type {
            png::ColorType::Rgb => buf,
            png::ColorType::Rgba => buf
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|[r, g, b, _]| [*r, *g, *b])
                .collect(),
            other => panic!("unexpected PNG colour type {other:?}"),
        };
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        Image {
            width: info.width,
            height: info.height,
            rgb,
        }
    }

    /// How many pixels differ from `other`'s, which must be the same size.
    pub fn pixels_unlike(&self, other: &Image) -> usize {
        assert_eq!(
            (self.width, self.height),
            (other.width, other.height),
            "the images differ in size"
        );
        let (ours, theirs) = (self.rgb.as_chunks::<3>().0, other.rgb.as_chunks::<3>().0);
        ours.iter().zip(theirs).filter(|(a, b)| a != b).count()
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}
