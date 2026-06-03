//! Locate the cover/poster image of a QuickTime `.mov` or MP4 video: the
//! iTunes-style cover art at `moov` → `udta` → `meta` → `ilst` → `covr` →
//! `data`. The `data` atom prefixes the image with a 4-byte well-known type
//! indicator (13 = JPEG, 14 = PNG) and a 4-byte locale; the remaining bytes are
//! the image.
//!
//! Videos without a cover atom (the common case for raw camera captures) yield
//! `None` — the GUI keeps the kind icon for those. Full video-frame extraction
//! is out of scope for B1.

use super::bmff::{find_box, BoxHeader};
use super::{be_u32, ThumbRegion};

/// `data`-atom well-known type for a JPEG payload.
const TYPE_JPEG: u32 = 13;
/// `data`-atom well-known type for a PNG payload.
const TYPE_PNG: u32 = 14;
/// `data` atom prefix: type indicator (4) + locale (4) before the image bytes.
const DATA_PREFIX: usize = 8;

/// Find the cover-art image span in a QuickTime/MP4 video's leading bytes.
pub fn cover_atom(header: &[u8]) -> Option<ThumbRegion> {
    let moov = find_box(header, 0, header.len(), b"moov")?;
    let udta = find_box(header, moov.content_start, moov.content_end, b"udta")?;
    let meta = find_box(header, udta.content_start, udta.content_end, b"meta")?;
    let ilst = find_ilst(header, &meta)?;
    let covr = find_box(header, ilst.content_start, ilst.content_end, b"covr")?;
    let data = find_box(header, covr.content_start, covr.content_end, b"data")?;
    cover_region(header, &data)
}

/// Find the `ilst` box under a `meta` box, tolerating both layouts: MP4's
/// `meta` is a FullBox (children start 4 bytes in), QuickTime's is a plain box.
fn find_ilst(buf: &[u8], meta: &BoxHeader) -> Option<BoxHeader> {
    find_box(buf, meta.content_start + 4, meta.content_end, b"ilst")
        .or_else(|| find_box(buf, meta.content_start, meta.content_end, b"ilst"))
}

/// Resolve a `data` atom to the span of its image bytes, accepting only the
/// JPEG and PNG well-known types (B1 re-encodes those; other payloads are not
/// decodable without extra codecs).
fn cover_region(buf: &[u8], data: &BoxHeader) -> Option<ThumbRegion> {
    // Low 24 bits of the first word are the well-known type; the top byte is a
    // version that is always 0 here.
    let type_code = be_u32(buf, data.content_start)? & 0x00FF_FFFF;
    if type_code != TYPE_JPEG && type_code != TYPE_PNG {
        return None;
    }
    let payload_start = data.content_start + DATA_PREFIX;
    // True length comes from the box end, which may sit past the read window —
    // the facade issues a second read for the tail when so.
    let len = data.box_end.checked_sub(payload_start)? as u64;
    if len == 0 {
        return None;
    }
    Some(ThumbRegion {
        offset: payload_start as u64,
        len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (content.len() + 8) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(&size.to_be_bytes());
        b.extend_from_slice(kind);
        b.extend_from_slice(content);
        b
    }

    /// Build `moov` → `udta` → `meta`(fullbox) → `ilst` → `covr` → `data`
    /// carrying `image` as a JPEG-typed cover atom.
    fn mov_with_cover(image: &[u8]) -> Vec<u8> {
        let mut data_content = Vec::new();
        data_content.extend_from_slice(&TYPE_JPEG.to_be_bytes()); // type indicator
        data_content.extend_from_slice(&0u32.to_be_bytes()); // locale
        data_content.extend_from_slice(image);
        let data = boxed(b"data", &data_content);
        let covr = boxed(b"covr", &data);
        let ilst = boxed(b"ilst", &covr);
        let mut meta_content = vec![0, 0, 0, 0]; // fullbox version/flags
        meta_content.extend_from_slice(&ilst);
        let meta = boxed(b"meta", &meta_content);
        let udta = boxed(b"udta", &meta);
        let moov = boxed(b"moov", &udta);

        let mut file = boxed(b"ftyp", b"qt  \0\0\0\0qt  ");
        file.extend_from_slice(&moov);
        file
    }

    #[test]
    fn locates_jpeg_cover_payload() {
        let image = b"\xFF\xD8\xFF\xE0 cover jpeg";
        let file = mov_with_cover(image);
        let region = cover_atom(&file).expect("region");
        let got = &file[region.offset as usize..(region.offset + region.len) as usize];
        assert_eq!(got, image);
    }

    #[test]
    fn returns_none_without_cover() {
        let file = boxed(b"ftyp", b"qt  \0\0\0\0qt  ");
        assert_eq!(cover_atom(&file), None);
    }
}
