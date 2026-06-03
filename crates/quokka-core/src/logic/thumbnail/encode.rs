//! Downscale a located thumbnail candidate and re-encode it to JPEG.
//!
//! The candidate bytes come from an embedded thumbnail (an EXIF/`covr` JPEG, a
//! HEIF thumbnail item). B1 only re-encodes what the pure-Rust `image` codecs
//! decode — JPEG and PNG. A candidate that needs HEVC (a HEIF `hvc1` thumbnail
//! item) fails to decode here and returns `None`; that file falls back to its
//! kind icon until the Phase B2 decoder lands. This keeps the core free of any
//! native `libheif` build dependency, so it cross-compiles to macOS and
//! Windows alike.

use image::{DynamicImage, ImageFormat};

/// A re-encoded thumbnail: JPEG bytes plus the dimensions they were encoded at.
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode `candidate`, downscale it to fit within `max_dim` on its longest edge
/// (never upscaling), and re-encode as JPEG. Returns `None` when the bytes
/// aren't a JPEG/PNG the pure-Rust codecs decode, or when encoding fails — a
/// tolerant skip, never an error.
pub fn to_jpeg_thumbnail(candidate: &[u8], max_dim: u32) -> Option<Encoded> {
    let decoded = image::load_from_memory(candidate).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return None;
    }
    let scaled = downscale(decoded, max_dim);
    let (out_width, out_height) = (scaled.width(), scaled.height());

    let mut bytes = Vec::new();
    scaled
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .ok()?;
    Some(Encoded {
        bytes,
        width: out_width,
        height: out_height,
    })
}

/// Fit `img` within `max_dim` on its longest edge, preserving aspect ratio.
/// Images already within bounds are returned unchanged — we never upscale a
/// small embedded thumbnail into a blurry larger one.
fn downscale(img: DynamicImage, max_dim: u32) -> DynamicImage {
    let max_dim = max_dim.max(1);
    if img.width().max(img.height()) <= max_dim {
        return img;
    }
    // `resize` preserves aspect ratio, fitting inside the (max_dim, max_dim)
    // box. `Triangle` is a good speed/quality trade for grid-sized thumbnails.
    img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
}

/// Encode a solid `width`×`height` RGB image as JPEG bytes — a decodable
/// candidate for the re-encode path. Shared across the thumbnail tests (here
/// and the `app::thumbnail` facade tests), which all need a real JPEG the
/// pure-Rust codec can decode.
#[cfg(test)]
pub(crate) fn solid_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        width,
        height,
        image::Rgb([10, 20, 30]),
    ));
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .expect("encode fixture");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscales_large_candidate_to_max_dim() {
        let encoded = to_jpeg_thumbnail(&solid_jpeg(800, 400), 256).expect("encoded");
        assert_eq!(encoded.width, 256); // longest edge clamped
        assert_eq!(encoded.height, 128); // aspect ratio preserved
                                         // Output is a valid JPEG.
        assert_eq!(&encoded.bytes[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn keeps_small_candidate_dimensions() {
        let encoded = to_jpeg_thumbnail(&solid_jpeg(100, 80), 256).expect("encoded");
        assert_eq!((encoded.width, encoded.height), (100, 80));
    }

    #[test]
    fn returns_none_for_undecodable_bytes() {
        assert!(to_jpeg_thumbnail(b"not an image", 256).is_none());
        assert!(to_jpeg_thumbnail(&[], 256).is_none());
    }
}
