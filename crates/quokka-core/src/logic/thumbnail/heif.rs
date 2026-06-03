//! Locate the embedded thumbnail of a HEIF/HEIC: the thumbnail *item* declared
//! in the `meta` box and pointed at by a `thmb` item reference from the primary
//! image.
//!
//! The walk is: read `meta` → `pitm` (which item is primary) → `iref` (the
//! `thmb` reference whose target is the primary gives the thumbnail item id) →
//! `iinf` (item types) → `iloc` (where each item's bytes live). The returned
//! span is the thumbnail item's bytes. Whether those bytes are a decodable
//! JPEG/PNG (B1) or HEVC (needing the Phase B2 decoder) is decided downstream
//! by [`encode`](super::encode) — this parser only locates them.
//!
//! ISO base-media box parsing is intentionally tolerant: any unexpected size,
//! version, or field width collapses to `None` rather than panicking, so an odd
//! file is skipped, never fatal.

use std::collections::HashMap;

use super::bmff::{child_boxes, find_box, BoxHeader};
use super::{be_u16, be_u32, be_u64, ThumbRegion};

/// Construction method 0 in `iloc` means the extent offset is a plain file
/// offset — the only method B1 resolves. Methods 1 (`idat`) and 2 (item) need
/// extra indirection and are skipped.
const CONSTRUCTION_FILE_OFFSET: u8 = 0;

/// Find the thumbnail item's byte span in a HEIF file's leading bytes.
pub fn thumbnail_item(header: &[u8]) -> Option<ThumbRegion> {
    let meta = find_box(header, 0, header.len(), b"meta")?;
    // `meta` is a FullBox: skip its 4-byte version/flags to reach child boxes.
    let children_start = meta.content_start + 4;
    let children_end = meta.content_end;

    let primary = find_box(header, children_start, children_end, b"pitm")
        .and_then(|b| parse_pitm(header, &b));
    let thumb_refs = find_box(header, children_start, children_end, b"iref")
        .map(|b| parse_iref(header, &b))
        .unwrap_or_default();
    let locations = find_box(header, children_start, children_end, b"iloc")
        .map(|b| parse_iloc(header, &b))
        .unwrap_or_default();

    let thumb_id = select_thumbnail_item(&thumb_refs, primary)?;
    let &(offset, len) = locations.get(&thumb_id)?;
    if len == 0 {
        return None;
    }
    Some(ThumbRegion { offset, len })
}

/// One `thmb` reference: `from` is the thumbnail item, `to` the items it is a
/// thumbnail of.
struct ThumbRef {
    from: u32,
    to: Vec<u32>,
}

/// Pick the thumbnail item id: prefer the `thmb` reference whose target is the
/// primary item; otherwise fall back to the first `thmb` reference.
fn select_thumbnail_item(refs: &[ThumbRef], primary: Option<u32>) -> Option<u32> {
    if let Some(primary) = primary {
        if let Some(r) = refs.iter().find(|r| r.to.contains(&primary)) {
            return Some(r.from);
        }
    }
    refs.first().map(|r| r.from)
}

/// Read an item id whose width depends on the box version: `u32` when `large`,
/// else `u16`.
fn read_item_id(buf: &[u8], at: usize, large: bool) -> Option<u32> {
    if large {
        be_u32(buf, at)
    } else {
        be_u16(buf, at).map(u32::from)
    }
}

/// `pitm` — the primary item id (the master image a `thmb` reference targets).
fn parse_pitm(buf: &[u8], b: &BoxHeader) -> Option<u32> {
    let version = *buf.get(b.content_start)?;
    read_item_id(buf, b.content_start + 4, version >= 1)
}

/// `iref` — collect the `thmb` (thumbnail) references. Item-id width is `u32`
/// for version ≥ 1, else `u16`.
fn parse_iref(buf: &[u8], b: &BoxHeader) -> Vec<ThumbRef> {
    let Some(&version) = buf.get(b.content_start) else {
        return Vec::new();
    };
    let large = version >= 1;
    let id_bytes = if large { 4 } else { 2 };
    let mut out = Vec::new();
    for rb in child_boxes(buf, b.content_start + 4, b.content_end) {
        if &rb.kind != b"thmb" {
            continue;
        }
        if let Some(parsed) = parse_thmb(buf, &rb, large, id_bytes) {
            out.push(parsed);
        }
    }
    out
}

/// One `thmb` single-item-type-reference box: `from` id, count, then `to` ids.
fn parse_thmb(buf: &[u8], rb: &BoxHeader, large: bool, id_bytes: usize) -> Option<ThumbRef> {
    let mut p = rb.content_start;
    let from = read_item_id(buf, p, large)?;
    p += id_bytes;
    let count = be_u16(buf, p)? as usize;
    p += 2;
    let mut to = Vec::with_capacity(count);
    for _ in 0..count {
        to.push(read_item_id(buf, p, large)?);
        p += id_bytes;
    }
    Some(ThumbRef { from, to })
}

/// `iloc` — map each item id to the file offset + length of its first extent.
/// Only construction method 0 (plain file offset) is resolved; other methods
/// are skipped.
fn parse_iloc(buf: &[u8], b: &BoxHeader) -> HashMap<u32, (u64, u64)> {
    parse_iloc_inner(buf, b).unwrap_or_default()
}

fn parse_iloc_inner(buf: &[u8], b: &BoxHeader) -> Option<HashMap<u32, (u64, u64)>> {
    let version = *buf.get(b.content_start)?;
    let mut p = b.content_start + 4;

    // Two packed nibble pairs: offset/length sizes, then base-offset/index
    // sizes. Each size is a byte count in {0, 4, 8} (we also tolerate 2).
    let sizes0 = *buf.get(p)?;
    let sizes1 = *buf.get(p + 1)?;
    p += 2;
    let offset_size = (sizes0 >> 4) as usize;
    let length_size = (sizes0 & 0x0F) as usize;
    let base_offset_size = (sizes1 >> 4) as usize;
    let index_size = if version >= 1 {
        (sizes1 & 0x0F) as usize
    } else {
        0
    };

    let item_count = if version < 2 {
        let c = be_u16(buf, p)? as usize;
        p += 2;
        c
    } else {
        let c = be_u32(buf, p)? as usize;
        p += 4;
        c
    };

    let mut out = HashMap::new();
    for _ in 0..item_count {
        let item_id = if version < 2 {
            let id = be_u16(buf, p)? as u32;
            p += 2;
            id
        } else {
            let id = be_u32(buf, p)?;
            p += 4;
            id
        };

        let mut construction = CONSTRUCTION_FILE_OFFSET;
        if version >= 1 {
            // 12 reserved bits + 4-bit construction method.
            construction = (be_u16(buf, p)? & 0x0F) as u8;
            p += 2;
        }
        p += 2; // data_reference_index

        let base_offset = read_sized_uint(buf, p, base_offset_size)?;
        p += base_offset_size;

        let extent_count = be_u16(buf, p)? as usize;
        p += 2;

        let mut first_extent = None;
        for i in 0..extent_count {
            if index_size > 0 {
                p += index_size; // extent_index — unused
            }
            let extent_offset = read_sized_uint(buf, p, offset_size)?;
            p += offset_size;
            let extent_length = read_sized_uint(buf, p, length_size)?;
            p += length_size;
            if i == 0 {
                first_extent = Some((extent_offset, extent_length));
            }
        }

        if construction != CONSTRUCTION_FILE_OFFSET {
            continue;
        }
        if let Some((extent_offset, extent_length)) = first_extent {
            out.insert(item_id, (base_offset + extent_offset, extent_length));
        }
    }
    Some(out)
}

/// Read a big-endian unsigned of `size` bytes (0, 2, 4, or 8). `size == 0`
/// yields 0 (an absent field); any other width is rejected.
fn read_sized_uint(buf: &[u8], at: usize, size: usize) -> Option<u64> {
    match size {
        0 => Some(0),
        2 => be_u16(buf, at).map(u64::from),
        4 => be_u32(buf, at).map(u64::from),
        8 => be_u64(buf, at),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap `content` in a box with the 4-character `kind`, 32-bit size.
    fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (content.len() + 8) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(&size.to_be_bytes());
        b.extend_from_slice(kind);
        b.extend_from_slice(content);
        b
    }

    /// Build a minimal HEIF: `ftyp`, then `meta` carrying `pitm`/`iinf`/`iref`/
    /// `iloc`, then an `mdat` holding `thumb_bytes`. The thumbnail item (id 2)
    /// is a `thmb` of the primary item (id 1). `iloc` points item 2 at the
    /// absolute offset where `thumb_bytes` lands in `mdat`.
    fn heif_with_thumbnail(thumb_bytes: &[u8]) -> Vec<u8> {
        // pitm v0 → primary item 1.
        let pitm = boxed(b"pitm", &[0, 0, 0, 0, 0, 1]);

        // iinf v0: count 1, one infe v2 declaring item 2 as type "jpeg".
        let mut infe_content = vec![2, 0, 0, 0]; // version 2, flags 0
        infe_content.extend_from_slice(&2u16.to_be_bytes()); // item_id 2
        infe_content.extend_from_slice(&0u16.to_be_bytes()); // protection
        infe_content.extend_from_slice(b"jpeg"); // item_type
        let infe = boxed(b"infe", &infe_content);
        let mut iinf_content = vec![0, 0, 0, 0]; // version 0, flags
        iinf_content.extend_from_slice(&1u16.to_be_bytes()); // entry count
        iinf_content.extend_from_slice(&infe);
        let iinf = boxed(b"iinf", &iinf_content);

        // iref v0: a thmb box, from item 2 → to item 1.
        let mut thmb_content = Vec::new();
        thmb_content.extend_from_slice(&2u16.to_be_bytes()); // from
        thmb_content.extend_from_slice(&1u16.to_be_bytes()); // count
        thmb_content.extend_from_slice(&1u16.to_be_bytes()); // to
        let thmb = boxed(b"thmb", &thmb_content);
        let mut iref_content = vec![0, 0, 0, 0]; // version 0
        iref_content.extend_from_slice(&thmb);
        let iref = boxed(b"iref", &iref_content);

        // iloc v0: offset_size 4, length_size 4, base_offset_size 0. One item.
        // The absolute extent offset is patched in once the layout is known.
        let build_iloc = |abs_offset: u32| -> Vec<u8> {
            let mut c = vec![0, 0, 0, 0]; // version 0, flags
            c.push(0x44); // offset_size=4, length_size=4
            c.push(0x00); // base_offset_size=0, index_size=0
            c.extend_from_slice(&1u16.to_be_bytes()); // item_count
            c.extend_from_slice(&2u16.to_be_bytes()); // item_id 2
            c.extend_from_slice(&0u16.to_be_bytes()); // data_reference_index
                                                      // base_offset_size 0 → no bytes
            c.extend_from_slice(&1u16.to_be_bytes()); // extent_count
            c.extend_from_slice(&abs_offset.to_be_bytes()); // extent_offset
            c.extend_from_slice(&(thumb_bytes.len() as u32).to_be_bytes()); // extent_length
            boxed(b"iloc", &c)
        };

        let ftyp = boxed(b"ftyp", b"heic\0\0\0\0heic");

        // Assemble once with a placeholder offset to learn the layout, then
        // rebuild with the real absolute offset of the mdat payload.
        let assemble = |iloc: &[u8]| -> (Vec<u8>, usize) {
            let mut meta_content = vec![0, 0, 0, 0]; // meta version/flags
            meta_content.extend_from_slice(&pitm);
            meta_content.extend_from_slice(&iinf);
            meta_content.extend_from_slice(&iref);
            meta_content.extend_from_slice(iloc);
            let meta = boxed(b"meta", &meta_content);

            let mut file = Vec::new();
            file.extend_from_slice(&ftyp);
            file.extend_from_slice(&meta);
            // mdat header (8 bytes) then payload.
            let mdat = boxed(b"mdat", thumb_bytes);
            let payload_abs = file.len() + 8;
            file.extend_from_slice(&mdat);
            (file, payload_abs)
        };

        let (_, payload_abs) = assemble(&build_iloc(0));
        let (file, _) = assemble(&build_iloc(payload_abs as u32));
        file
    }

    #[test]
    fn locates_thumbnail_item_bytes_in_mdat() {
        let thumb = b"\xFF\xD8\xFF\xE0 pretend jpeg thumbnail bytes";
        let file = heif_with_thumbnail(thumb);
        let region = thumbnail_item(&file).expect("region");
        let got = &file[region.offset as usize..(region.offset + region.len) as usize];
        assert_eq!(got, thumb);
    }

    #[test]
    fn returns_none_without_meta_box() {
        let file = boxed(b"ftyp", b"heic\0\0\0\0heic");
        assert_eq!(thumbnail_item(&file), None);
    }
}
