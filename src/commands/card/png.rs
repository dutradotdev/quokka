//! SVG → 1080×1080 PNG rasterisation via `resvg`/`tiny-skia`.
//!
//! JetBrains Mono is bundled with the binary and registered into `usvg`'s
//! font database before parsing. `resvg` does not consult system fonts on
//! macOS reliably, so without the embedded fonts every glyph would render
//! as a missing-glyph box on a clean machine.

use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use resvg::tiny_skia;
use resvg::usvg;

/// Square canvas the renderer emits.
pub const CANVAS_PX: u32 = 1080;

const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
const FONT_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");

/// Shared font database — built once per process. The system-font scan
/// alone takes 50-100ms on macOS; doing it on every card would dominate
/// the wall-clock cost of batch renders (chaos suite, menu loop). Wrapped
/// in `Arc` so each `usvg::Options` clones a pointer, not the data.
fn font_database() -> Arc<usvg::fontdb::Database> {
    static DB: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut fontdb = usvg::fontdb::Database::new();
        fontdb.load_font_data(FONT_REGULAR.to_vec());
        fontdb.load_font_data(FONT_MEDIUM.to_vec());
        // Pick up system fonts so CJK, Arabic, and emoji glyphs that
        // JetBrains Mono doesn't ship cover render instead of falling
        // through as tofu. On macOS this means San Francisco + Apple
        // Color Emoji; on other OSes whatever is installed.
        fontdb.load_system_fonts();
        Arc::new(fontdb)
    });
    DB.clone()
}

/// Parse the SVG, rasterise to PNG bytes. The resulting buffer is exactly
/// what gets written to disk.
pub fn svg_to_png(svg: &str) -> Result<Vec<u8>> {
    let opts = usvg::Options {
        fontdb: font_database(),
        ..Default::default()
    };

    let tree = usvg::Tree::from_str(svg, &opts).context("parse generated SVG")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(CANVAS_PX, CANVAS_PX).context("allocate 1080x1080 pixmap")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().context("encode PNG")
}

/// Extract the (width, height) baked into a PNG IHDR. Used by tests to
/// assert the output dimensions without pulling in a full PNG decoder.
pub fn read_png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // PNG = 8-byte signature + chunks. First chunk is always IHDR, with
    // width (u32 BE) at offset 16 and height (u32 BE) at offset 20.
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid SVG to prove the pipeline runs end-to-end without a
    /// device. The real card SVG is exercised via the integration test.
    const MINIMAL: &str = r##"<?xml version="1.0"?>
        <svg xmlns="http://www.w3.org/2000/svg" width="1080" height="1080">
            <rect width="1080" height="1080" fill="#1A1916"/>
        </svg>"##;

    #[test]
    fn renders_minimal_svg_to_a_1080_png() {
        let bytes = svg_to_png(MINIMAL).expect("render");
        assert!(
            bytes.len() > 100,
            "PNG suspiciously small ({})",
            bytes.len()
        );
        let (w, h) = read_png_dimensions(&bytes).expect("valid PNG signature + IHDR");
        assert_eq!((w, h), (CANVAS_PX, CANVAS_PX));
    }

    #[test]
    fn read_png_dimensions_rejects_non_png_input() {
        assert!(read_png_dimensions(b"not a png").is_none());
        assert!(read_png_dimensions(&[]).is_none());
    }
}
