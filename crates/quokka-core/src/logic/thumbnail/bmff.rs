//! Shared ISO base-media file format (ISO-BMFF) box walking, used by both the
//! HEIF thumbnail-item parser and the MOV/MP4 cover-atom parser. QuickTime
//! `.mov`, MP4, and HEIF all share this box structure.
//!
//! Tolerant by construction: a size that runs past the read window is clamped,
//! and any header that can't be read returns `None` so the caller skips the box
//! rather than panicking.

use super::{be_u32, be_u64};

/// A parsed box header: its 4-character type, the absolute range of its
/// contents, and the absolute end of the whole box.
#[derive(Clone, Copy)]
pub(super) struct BoxHeader {
    pub kind: [u8; 4],
    pub content_start: usize,
    pub content_end: usize,
    pub box_end: usize,
}

/// Read the box header at absolute offset `at`, handling 32-bit, 64-bit
/// (`size == 1`), and to-end (`size == 0`) sizes. The content/box ends are
/// clamped to the buffer so a box that runs past the read window still parses
/// the children that *are* present.
pub(super) fn read_box(buf: &[u8], at: usize) -> Option<BoxHeader> {
    let size32 = be_u32(buf, at)? as usize;
    let kind: [u8; 4] = buf.get(at + 4..at + 8)?.try_into().ok()?;
    let (content_start, box_end) = match size32 {
        1 => (at + 16, at.checked_add(be_u64(buf, at + 8)? as usize)?),
        0 => (at + 8, buf.len()),
        n => (at + 8, at.checked_add(n)?),
    };
    if content_start > buf.len() || box_end < content_start {
        return None;
    }
    Some(BoxHeader {
        kind,
        content_start,
        content_end: box_end.min(buf.len()),
        box_end,
    })
}

/// First box of type `target` in the half-open absolute range `[at, end)`.
pub(super) fn find_box(
    buf: &[u8],
    mut at: usize,
    end: usize,
    target: &[u8; 4],
) -> Option<BoxHeader> {
    while at + 8 <= end {
        let b = read_box(buf, at)?;
        if &b.kind == target {
            return Some(b);
        }
        if b.box_end <= at {
            return None; // no forward progress — malformed
        }
        at = b.box_end;
    }
    None
}

/// Every direct child box in the half-open absolute range `[at, end)`.
pub(super) fn child_boxes(buf: &[u8], mut at: usize, end: usize) -> Vec<BoxHeader> {
    let mut out = Vec::new();
    while at + 8 <= end {
        let Some(b) = read_box(buf, at) else { break };
        if b.box_end <= at {
            break;
        }
        let next = b.box_end;
        out.push(b);
        at = next;
    }
    out
}
