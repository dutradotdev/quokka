//! Embedded-thumbnail extraction (Phase B1 of the progressive-preview design).
//!
//! Given the leading bytes of a media file, locate a thumbnail the file
//! *already carries* — the EXIF thumbnail of a JPEG, the thumbnail item of a
//! HEIF/HEIC, or the cover/poster atom of a QuickTime/MP4 video — without
//! decoding the full image. The located bytes are then downscaled and
//! re-encoded to a small JPEG by [`encode`].
//!
//! This module is **pure**: every parser takes a `&[u8]` and returns an
//! [`Option<ThumbRegion>`], never touching the device. The async I/O (reading
//! the header window, then the located region) lives in the [`app`](crate::app)
//! facade, which drives this logic exactly as `analyze` drives the walk
//! heuristics. The parsers are deliberately tolerant — any malformed or
//! unexpected structure yields `None`, never a panic or an error (the executable
//! form of the project's tolerant-parsing rule), so one odd file never aborts a
//! batch.

mod bmff;
pub mod encode;
pub mod heif;
pub mod jpeg;
pub mod mov;

use serde::{Deserialize, Serialize};

/// How many leading bytes the facade reads to locate an embedded thumbnail.
/// Covers a JPEG's EXIF APP1 segment (capped at 64 KiB by the spec), a HEIF
/// `meta` box plus a small thumbnail item in `mdat`, and a front-loaded MOV
/// `moov`/`udta` cover atom. Files that store the thumbnail bytes past this
/// window still resolve: the parser returns the absolute region and the facade
/// issues a second bounded read for it.
pub const READ_WINDOW_BYTES: u64 = 256 * 1024;

/// Default longest-edge size, in pixels, of a produced thumbnail. The facade
/// takes `max_dim` as a parameter (the GUI may request grid vs. hover-zoom
/// sizes); this is the sensible grid default callers reach for.
pub const DEFAULT_MAX_DIM: u32 = 256;

/// How many thumbnails the streaming batch builds concurrently. Small on
/// purpose: a 200-item grid must not open 200 simultaneous device reads. Mirror
/// of the bounded fan-out the app-enrichment batch uses.
pub const FAN_OUT: usize = 6;

/// Encoded delivery format of a [`Thumbnail`]. JPEG only in v1 — the embedded
/// thumbnails B1 extracts are already JPEG/PNG and re-encode to JPEG, which
/// every webview renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThumbFormat {
    Jpeg,
}

/// A browser-renderable thumbnail for one media file: the source path (so batch
/// results can be keyed back to the grid cell), the decoded dimensions, the
/// format, and the encoded bytes. The GUI mirrors this in `bindings.ts` and
/// serves `bytes` as an `<img src>` (base64 data URL today; a `quokka-thumb://`
/// scheme later). Bytes ride the wire as a JSON array, matching the existing
/// `RenderedCard::png` contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub remote: String,
    pub width: u32,
    pub height: u32,
    pub format: ThumbFormat,
    pub bytes: Vec<u8>,
}

/// One streaming batch from the `thumbnails` facade, mirroring
/// [`BatchUpdate`](crate::device::BatchUpdate): `thumbnails` carries only the
/// results produced since the last update (files that yielded no embedded
/// thumbnail advance `done` without appearing here), and `done`/`total` are
/// cumulative so a grid can show "127 / 200" progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbBatch {
    pub thumbnails: Vec<Thumbnail>,
    pub done: usize,
    pub total: usize,
}

/// Per-batch callback for the `thumbnails` facade. Follows
/// [`BatchCallback`](crate::device::BatchCallback) verbatim (`Send + Sync`, a
/// closed channel just drops updates while work continues) so the GUI reuses
/// its existing enrichment channel wiring.
pub type ThumbCallback = Box<dyn Fn(ThumbBatch) + Send + Sync>;

/// Absolute byte span of an embedded thumbnail within the source file. Returned
/// by every parser so the facade resolves it uniformly: if the span sits inside
/// the header window already read it is sliced for free, otherwise a second
/// bounded read fetches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbRegion {
    pub offset: u64,
    pub len: u64,
}

/// Media containers B1 knows how to mine for an embedded thumbnail. Detected
/// from magic bytes first, falling back to the path extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Jpeg,
    Heif,
    /// ISO-BMFF video (QuickTime `.mov`, MP4 `.mp4`/`.m4v`) — mined for a cover
    /// atom.
    IsoVideo,
    Unknown,
}

/// Locate the embedded thumbnail in `header` (the leading [`READ_WINDOW_BYTES`]
/// of the file at `path`), returning its absolute byte span or `None` when the
/// container carries no embedded thumbnail B1 can use. The per-container parser
/// is selected by [`detect_container`], not by an open-coded branch ladder.
pub fn locate(header: &[u8], path: &str) -> Option<ThumbRegion> {
    match detect_container(header, path) {
        Container::Jpeg => jpeg::exif_thumbnail(header),
        Container::Heif => heif::thumbnail_item(header),
        Container::IsoVideo => mov::cover_atom(header),
        Container::Unknown => None,
    }
}

/// Detect the container from magic bytes, falling back to the path extension
/// when the magic is inconclusive (e.g. an ISO-BMFF brand we don't classify).
fn detect_container(header: &[u8], path: &str) -> Container {
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Container::Jpeg;
    }
    if let Some(brand) = iso_bmff_major_brand(header) {
        if HEIF_BRANDS.contains(&&brand[..]) {
            return Container::Heif;
        }
        if VIDEO_BRANDS.contains(&&brand[..]) {
            return Container::IsoVideo;
        }
    }
    container_from_extension(path)
}

/// The major-brand bytes of an ISO base-media file (`ftyp` box), if `header`
/// begins with one. Bytes 4..8 are the `ftyp` type tag; 8..12 are the brand.
fn iso_bmff_major_brand(header: &[u8]) -> Option<[u8; 4]> {
    if header.get(4..8)? != b"ftyp" {
        return None;
    }
    let brand = header.get(8..12)?;
    Some([brand[0], brand[1], brand[2], brand[3]])
}

/// HEIF/HEIC `ftyp` major brands. `mif1`/`msf1` are the generic HEIF brands iOS
/// also stamps; `heic`/`heix` are the HEVC-coded image brands.
const HEIF_BRANDS: &[&[u8]] = &[
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"mif1", b"msf1",
];

/// QuickTime / MP4 `ftyp` major brands worth scanning for a cover atom.
const VIDEO_BRANDS: &[&[u8]] = &[
    b"qt  ", b"mp42", b"mp41", b"isom", b"M4V ", b"M4A ", b"avc1",
];

/// Last-resort container guess from the file extension, for files whose magic
/// bytes we didn't classify (or that fell outside the read window).
fn container_from_extension(path: &str) -> Container {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Container::Jpeg,
        "heic" | "heif" => Container::Heif,
        "mov" | "mp4" | "m4v" => Container::IsoVideo,
        _ => Container::Unknown,
    }
}

/// Read a big-endian `u16` at `at`, bounds-checked. Shared by the ISO-BMFF and
/// JPEG-segment parsers, which are big-endian on the wire.
pub(crate) fn be_u16(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(buf.get(at..at + 2)?.try_into().ok()?))
}

/// Read a big-endian `u32` at `at`, bounds-checked.
pub(crate) fn be_u32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

/// Read a big-endian `u64` at `at`, bounds-checked.
pub(crate) fn be_u64(buf: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(buf.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_jpeg_from_magic() {
        assert_eq!(
            detect_container(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 0], "x"),
            Container::Jpeg
        );
    }

    #[test]
    fn detects_heif_and_video_from_ftyp_brand() {
        let heic = [&[0, 0, 0, 0x18], &b"ftyp"[..], &b"heic"[..]].concat();
        assert_eq!(detect_container(&heic, "x"), Container::Heif);
        let mov = [&[0, 0, 0, 0x14], &b"ftyp"[..], &b"qt  "[..]].concat();
        assert_eq!(detect_container(&mov, "x"), Container::IsoVideo);
    }

    #[test]
    fn falls_back_to_extension_when_magic_is_unknown() {
        assert_eq!(
            detect_container(&[0, 1, 2, 3], "/a/b.HEIC"),
            Container::Heif
        );
        assert_eq!(detect_container(&[], "/a/b.mp4"), Container::IsoVideo);
        assert_eq!(detect_container(&[], "/a/b.pdf"), Container::Unknown);
    }

    #[test]
    fn locate_returns_none_for_unknown_container() {
        assert_eq!(locate(&[0, 1, 2, 3], "/a/b.pdf"), None);
    }

    #[test]
    fn be_readers_are_bounds_checked() {
        assert_eq!(be_u16(&[0x12], 0), None);
        assert_eq!(be_u16(&[0x12, 0x34], 0), Some(0x1234));
        assert_eq!(be_u32(&[0, 0, 0], 0), None);
        assert_eq!(be_u64(&[0; 4], 0), None);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // The tolerant-parsing invariant in executable form: every extractor must
    // return an `Option` for *any* input, never panic — the same posture the
    // Android `dumpsys` parsers are held to. Each container's magic-byte prefix
    // is prepended to arbitrary bytes so the dispatcher actually routes into the
    // real parser instead of bailing at detection.
    proptest! {
        #[test]
        fn jpeg_parser_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let mut input = vec![0xFF, 0xD8, 0xFF, 0xE1];
            input.extend_from_slice(&bytes);
            let _ = jpeg::exif_thumbnail(&input);
        }

        #[test]
        fn heif_parser_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let mut input = vec![0, 0, 0, 0];
            input.extend_from_slice(b"ftyp");
            input.extend_from_slice(b"heic");
            input.extend_from_slice(&bytes);
            let _ = heif::thumbnail_item(&input);
        }

        #[test]
        fn mov_parser_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let mut input = vec![0, 0, 0, 0];
            input.extend_from_slice(b"ftyp");
            input.extend_from_slice(b"qt  ");
            input.extend_from_slice(&bytes);
            let _ = mov::cover_atom(&input);
        }

        #[test]
        fn locate_and_encode_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = locate(&bytes, "/some/path.heic");
            let _ = encode::to_jpeg_thumbnail(&bytes, DEFAULT_MAX_DIM);
        }
    }
}
