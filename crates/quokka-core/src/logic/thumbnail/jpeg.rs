//! Locate the embedded thumbnail of a JPEG: the JPEG image stored in IFD1 of
//! the EXIF block, inside the `APP1` segment. The thumbnail is a self-contained
//! JPEG referenced by `JPEGInterchangeFormat` (offset) and
//! `JPEGInterchangeFormatLength` (length), both relative to the TIFF header.
//!
//! Reused by [`heif`](super::heif): a HEIF `Exif` item carries the same
//! TIFF/EXIF structure, so its IFD1 thumbnail is found by the same code once
//! the item's leading offset prefix is skipped.

use super::ThumbRegion;

/// JPEG start-of-image marker.
const SOI: [u8; 2] = [0xFF, 0xD8];
/// `APP1` application marker — carries the EXIF block.
const MARKER_APP1: u8 = 0xE1;
/// Start-of-scan marker. Past it lies entropy-coded image data, never an `APP1`
/// segment — so the marker walk stops here.
const MARKER_SOS: u8 = 0xDA;
/// `"Exif\0\0"` — the six-byte identifier prefixing an EXIF `APP1` payload.
const EXIF_PREFIX: &[u8] = b"Exif\0\0";

/// EXIF/TIFF tag for the embedded thumbnail's byte offset (relative to the TIFF
/// header).
const TAG_THUMB_OFFSET: u16 = 0x0201;
/// EXIF/TIFF tag for the embedded thumbnail's byte length.
const TAG_THUMB_LENGTH: u16 = 0x0202;
/// Size of one TIFF IFD entry: tag(2) + type(2) + count(4) + value/offset(4).
const IFD_ENTRY_SIZE: usize = 12;

/// Byte order of a TIFF block — EXIF can be either, declared by its header.
#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

impl Endian {
    fn u16(self, buf: &[u8], at: usize) -> Option<u16> {
        let raw: [u8; 2] = buf.get(at..at + 2)?.try_into().ok()?;
        Some(match self {
            Endian::Big => u16::from_be_bytes(raw),
            Endian::Little => u16::from_le_bytes(raw),
        })
    }

    fn u32(self, buf: &[u8], at: usize) -> Option<u32> {
        let raw: [u8; 4] = buf.get(at..at + 4)?.try_into().ok()?;
        Some(match self {
            Endian::Big => u32::from_be_bytes(raw),
            Endian::Little => u32::from_le_bytes(raw),
        })
    }
}

/// Find the embedded EXIF thumbnail in a JPEG file's leading bytes, returning
/// its absolute span. `None` when the file carries no EXIF `APP1` thumbnail.
pub fn exif_thumbnail(header: &[u8]) -> Option<ThumbRegion> {
    if header.get(0..2)? != SOI {
        return None;
    }
    let mut pos = 2;
    loop {
        // Every segment begins with a 0xFF marker byte; a misaligned scan means
        // a malformed file — bail tolerantly.
        if *header.get(pos)? != 0xFF {
            return None;
        }
        let marker = *header.get(pos + 1)?;
        if marker == MARKER_SOS {
            return None;
        }
        // Segment length covers the 2 length bytes plus the payload, but not
        // the 2 marker bytes.
        let seg_len = super::be_u16(header, pos + 2)? as usize;
        let payload_start = pos + 4;
        let payload_end = pos + 2 + seg_len;
        if marker == MARKER_APP1 {
            let payload = header.get(payload_start..payload_end)?;
            if let Some(region) = exif_thumb_in_app1(payload, payload_start) {
                return Some(region);
            }
        }
        pos = payload_end;
    }
}

/// Extract the IFD1 thumbnail from an `APP1` payload that begins with the EXIF
/// prefix. `payload_abs` is the payload's absolute offset in the file, so the
/// returned span is absolute. Public to `super` so [`heif`](super::heif) can
/// reuse it on a HEIF `Exif` item's TIFF block.
pub(super) fn exif_thumb_in_app1(payload: &[u8], payload_abs: usize) -> Option<ThumbRegion> {
    if !payload.starts_with(EXIF_PREFIX) {
        return None;
    }
    let tiff_abs = payload_abs + EXIF_PREFIX.len();
    let tiff = payload.get(EXIF_PREFIX.len()..)?;
    thumb_in_tiff(tiff, tiff_abs)
}

/// Walk a TIFF block to IFD1 and read its thumbnail offset/length tags.
/// `tiff_abs` is the TIFF header's absolute file offset; the thumbnail tags are
/// TIFF-relative, so the returned span is absolute.
pub(super) fn thumb_in_tiff(tiff: &[u8], tiff_abs: usize) -> Option<ThumbRegion> {
    let endian = match tiff.get(0..2)? {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    // Magic 42 confirms the byte order was read correctly.
    if endian.u16(tiff, 2)? != 0x002A {
        return None;
    }
    let ifd0_off = endian.u32(tiff, 4)? as usize;
    let ifd1_off = next_ifd_offset(tiff, ifd0_off, endian)?;
    if ifd1_off == 0 {
        return None;
    }
    let (thumb_off, thumb_len) = thumb_tags(tiff, ifd1_off, endian)?;
    let offset = tiff_abs.checked_add(thumb_off as usize)? as u64;
    Some(ThumbRegion {
        offset,
        len: thumb_len as u64,
    })
}

/// Offset of the IFD following the one at `ifd_off` (its "next IFD" pointer).
fn next_ifd_offset(tiff: &[u8], ifd_off: usize, endian: Endian) -> Option<usize> {
    let count = endian.u16(tiff, ifd_off)? as usize;
    let next_ptr = ifd_off + 2 + count * IFD_ENTRY_SIZE;
    Some(endian.u32(tiff, next_ptr)? as usize)
}

/// Read the thumbnail offset + length from the entries of the IFD at `ifd_off`.
/// Both are LONG values stored inline in each entry's value field.
fn thumb_tags(tiff: &[u8], ifd_off: usize, endian: Endian) -> Option<(u32, u32)> {
    let count = endian.u16(tiff, ifd_off)? as usize;
    let mut thumb_off = None;
    let mut thumb_len = None;
    for i in 0..count {
        let entry = ifd_off + 2 + i * IFD_ENTRY_SIZE;
        let tag = endian.u16(tiff, entry)?;
        let value = endian.u32(tiff, entry + 8)?;
        match tag {
            TAG_THUMB_OFFSET => thumb_off = Some(value),
            TAG_THUMB_LENGTH => thumb_len = Some(value),
            _ => {}
        }
    }
    match (thumb_off, thumb_len) {
        (Some(off), Some(len)) if len > 0 => Some((off, len)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a TIFF block (big-endian) whose IFD1 points at a thumbnail at
    /// `thumb_off`/`thumb_len` (both TIFF-relative). IFD0 is empty.
    fn tiff_with_ifd1(thumb_off: u32, thumb_len: u32) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"MM"); // big-endian
        t.extend_from_slice(&0x002Au16.to_be_bytes());
        t.extend_from_slice(&8u32.to_be_bytes()); // IFD0 at offset 8
                                                  // IFD0: zero entries, next = IFD1 at offset 14.
        t.extend_from_slice(&0u16.to_be_bytes()); // count 0
        t.extend_from_slice(&14u32.to_be_bytes()); // next IFD offset
                                                   // IFD1 at 14: two entries, next = 0.
        t.extend_from_slice(&2u16.to_be_bytes()); // count 2
        let mut entry = |tag: u16, value: u32| {
            t.extend_from_slice(&tag.to_be_bytes());
            t.extend_from_slice(&4u16.to_be_bytes()); // type LONG
            t.extend_from_slice(&1u32.to_be_bytes()); // count 1
            t.extend_from_slice(&value.to_be_bytes());
        };
        entry(TAG_THUMB_OFFSET, thumb_off);
        entry(TAG_THUMB_LENGTH, thumb_len);
        t.extend_from_slice(&0u32.to_be_bytes()); // no further IFD
        t
    }

    #[test]
    fn reads_thumb_region_from_tiff_relative_offset() {
        let tiff = tiff_with_ifd1(100, 42);
        let region = thumb_in_tiff(&tiff, 1000).expect("region");
        assert_eq!(region.offset, 1100); // tiff_abs + thumb_off
        assert_eq!(region.len, 42);
    }

    #[test]
    fn locates_thumb_through_full_app1_segment() {
        // Build a JPEG: SOI, APP1{ "Exif\0\0" + TIFF }, then filler.
        let tiff = tiff_with_ifd1(8, 9);
        let mut app1 = Vec::new();
        app1.extend_from_slice(EXIF_PREFIX);
        app1.extend_from_slice(&tiff);
        let seg_len = (app1.len() + 2) as u16; // payload + 2 length bytes

        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&SOI);
        jpeg.push(0xFF);
        jpeg.push(MARKER_APP1);
        jpeg.extend_from_slice(&seg_len.to_be_bytes());
        let payload_abs = jpeg.len();
        jpeg.extend_from_slice(&app1);

        let region = exif_thumbnail(&jpeg).expect("region");
        // tiff starts at payload_abs + 6; thumb at +8 of tiff.
        assert_eq!(region.offset as usize, payload_abs + EXIF_PREFIX.len() + 8);
        assert_eq!(region.len, 9);
    }

    #[test]
    fn returns_none_without_soi() {
        assert_eq!(exif_thumbnail(&[0x00, 0x01, 0x02]), None);
    }

    #[test]
    fn returns_none_when_no_app1_thumbnail() {
        // SOI then straight to SOS — no APP1 at all.
        let jpeg = [0xFF, 0xD8, 0xFF, MARKER_SOS, 0x00, 0x02];
        assert_eq!(exif_thumbnail(&jpeg), None);
    }
}
