//! Application facade — the neutral, surface-agnostic API the CLI, `--json`,
//! and the (future) Tauri GUI all consume.
//!
//! Each function takes a `&dyn Device`, drives the device + the pure logic,
//! and returns a serializable DTO. Nothing here knows about terminals,
//! ratatui, or Tauri: the CLI renders these DTOs as text, `--json` prints
//! them, and the GUI maps each function to a one-line `#[tauri::command]`.
//!
//! Errors surface as [`DeviceError`] (serializable as `{ kind, message }`)
//! rather than `anyhow::Error`, so every surface gets a stable, branchable
//! failure shape. The device layer still works in `anyhow` internally; we
//! recover the typed error by downcasting at this boundary.

pub mod redact;

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Receiver;

use crate::card::data::{self, CardData};
use crate::card::{png, render};
use crate::device::{
    App, BatchCallback, Device, DeviceError, DeviceInfo, DeviceStatus, LogEntry, MediaFile,
    PullCallback, WalkCallback,
};
use crate::logic::thumbnail;
use crate::logic::{analyze, media};

// Re-export the report DTO that lives next to its pure builders, so callers
// reach the whole facade surface through `app::`.
pub use crate::logic::media::MediaReport;
// Same for the thumbnail DTOs + streaming-batch types — the GUI mirrors these
// in `bindings.ts` and reuses the enrichment channel pattern for `ThumbBatch`.
pub use crate::logic::thumbnail::{ThumbBatch, ThumbCallback, ThumbFormat, Thumbnail};

/// Recover the typed [`DeviceError`] from an `anyhow::Error`, falling back to
/// [`DeviceError::Other`] when the source wasn't a `DeviceError`. This is the
/// single conversion point between the `anyhow`-based device layer and the
/// typed facade boundary.
fn to_device_error(err: anyhow::Error) -> DeviceError {
    match err.downcast::<DeviceError>() {
        Ok(typed) => typed,
        Err(other) => DeviceError::Other(other.to_string()),
    }
}

/// Full device dashboard snapshot.
pub async fn status(device: &dyn Device) -> Result<DeviceStatus, DeviceError> {
    device.status().await.map_err(to_device_error)
}

/// Static identity. With `redact`, PII fields are masked before returning —
/// the masked form is what the caller renders or serializes.
pub async fn info(device: &dyn Device, redact: bool) -> Result<DeviceInfo, DeviceError> {
    let info = device.info().await.map_err(to_device_error)?;
    Ok(if redact {
        redact::device_info(info)
    } else {
        info
    })
}

/// User-installed apps, enriched with dynamic (cache + downloads) sizes.
/// `on_batch` fires per enrichment batch so a UI can stream live size
/// updates; pass a no-op callback when you only want the final list.
pub async fn apps(device: &dyn Device, on_batch: BatchCallback) -> Result<Vec<App>, DeviceError> {
    let base = device.apps().await.map_err(to_device_error)?;
    device
        .with_dynamic_sizes(base, on_batch)
        .await
        .map_err(to_device_error)
}

/// User + system apps with bundle sizes only (no enrichment). Used by `card`'s
/// TOP APPS section, which wants the real storage heavyweights regardless of
/// who installed them.
pub async fn all_apps(device: &dyn Device) -> Result<Vec<App>, DeviceError> {
    device.all_apps().await.map_err(to_device_error)
}

/// One auto-mark group from the `analyze` heuristics: a human label, what it
/// detects, and the matched file paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoMark {
    pub label: String,
    pub description: String,
    pub paths: Vec<String>,
}

/// Result of walking the media roots: every file sorted largest-first, totals,
/// and the auto-mark heuristic groups (live-photo motion, edited originals,
/// old screenshots, exact duplicates).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeReport {
    pub files: Vec<MediaFile>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub marks: Vec<AutoMark>,
}

/// Walk the media roots and build the full analyze report. `now_unix` drives
/// the age-based heuristics (old screenshots), kept as a parameter so the
/// result is deterministic and testable.
pub async fn analyze(
    device: &dyn Device,
    now_unix: i64,
    on_progress: WalkCallback,
) -> Result<AnalyzeReport, DeviceError> {
    let files = device
        .afc_walk(device.media_roots(), on_progress)
        .await
        .map_err(to_device_error)?;
    let total_files = files.len();
    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    let marks = analyze::heuristics::detect_all(&files, now_unix)
        .into_iter()
        .filter(|m| m.enabled)
        .map(|m| AutoMark {
            label: m.label.to_string(),
            description: m.description.to_string(),
            paths: m.indices().iter().map(|&i| files[i].path.clone()).collect(),
        })
        .collect();
    let files = analyze::sort_by_size(files);
    Ok(AnalyzeReport {
        files,
        total_files,
        total_bytes,
        marks,
    })
}

/// Survey the media area: per-kind/per-month breakdown, largest files, and
/// (optionally) likely-duplicate groups.
pub async fn media(
    device: &dyn Device,
    find_duplicates: bool,
    on_progress: WalkCallback,
) -> Result<MediaReport, DeviceError> {
    let files = device
        .afc_walk(device.media_roots(), on_progress)
        .await
        .map_err(to_device_error)?;
    Ok(media::build_report(
        &files,
        find_duplicates,
        crate::fmt::now_unix(),
        None,
        device.media_roots(),
    ))
}

/// One file that could not be deleted, with the reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFailure {
    pub path: String,
    pub error: String,
}

/// Outcome of a batch delete: which paths were removed and which failed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub deleted: Vec<String>,
    pub failed: Vec<DeleteFailure>,
}

/// Delete each path via AFC, collecting per-path success/failure. A single
/// failed delete does not abort the rest — the caller sees the full outcome.
pub async fn delete_files(
    device: &dyn Device,
    paths: &[String],
) -> Result<DeleteOutcome, DeviceError> {
    let mut outcome = DeleteOutcome::default();
    for path in paths {
        match device.afc_delete(path).await {
            Ok(()) => outcome.deleted.push(path.clone()),
            Err(e) => outcome.failed.push(DeleteFailure {
                path: path.clone(),
                error: to_device_error(e).to_string(),
            }),
        }
    }
    Ok(outcome)
}

/// Copy `remote` (an absolute device media path, the same space `analyze` /
/// `media` walk) to the local `dest`, streaming in bounded chunks. `on_progress`
/// fires after each chunk with the running byte count, so a UI can divide it
/// against the size it already has from the walk. The GUI maps one Tauri command
/// to this so it can open a media file locally before deleting it.
pub async fn pull_file(
    device: &dyn Device,
    remote: &str,
    dest: &Path,
    on_progress: PullCallback,
) -> Result<(), DeviceError> {
    device
        .pull_file(remote, dest, on_progress)
        .await
        .map_err(to_device_error)
}

/// Read the byte window `[offset, offset + len)` from `remote` (an absolute
/// device media path) and return it. Bounded by `len`, so the result fits in
/// memory; a window past EOF returns fewer bytes (or none), never an error. The
/// GUI maps one Tauri command to this and serves browser `Range` requests by
/// looping over windows — letting a video start playing before the whole file
/// has transferred.
pub async fn read_range(
    device: &dyn Device,
    remote: &str,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, DeviceError> {
    device
        .read_range(remote, offset, len)
        .await
        .map_err(to_device_error)
}

/// Produce a browser-renderable thumbnail for the media file at `remote`,
/// downscaled to fit `max_dim` on its longest edge. Reads as few bytes as
/// possible: it pulls the file's header window, locates an embedded thumbnail
/// (EXIF for JPEG, the thumbnail item for HEIF, the cover atom for MOV/MP4),
/// and re-encodes that to JPEG — no full-file pull, no image decoder beyond the
/// embedded thumbnail. Returns `Ok(None)` when the file carries no embedded
/// thumbnail B1 can use (the GUI keeps its kind icon); `Err` only on a genuine
/// device read failure.
pub async fn thumbnail(
    device: &dyn Device,
    remote: &str,
    max_dim: u32,
) -> Result<Option<Thumbnail>, DeviceError> {
    build_thumbnail(device, remote, max_dim).await
}

/// Stream thumbnails for `paths`, emitting one [`ThumbBatch`] per processed file
/// as results land. Mirrors [`apps`] enrichment: a bounded fan-out keeps a large
/// grid from opening one device read per file at once, files that yield no
/// thumbnail still advance `done`, and a per-file read/parse failure is skipped
/// (logged, not fatal) so one odd file never aborts the batch. A closed callback
/// channel simply drops updates while the remaining work continues.
pub async fn thumbnails(
    device: &dyn Device,
    paths: &[String],
    max_dim: u32,
    on_batch: ThumbCallback,
) -> Result<(), DeviceError> {
    use futures::stream::{FuturesUnordered, StreamExt};

    let total = paths.len();
    if total == 0 {
        return Ok(());
    }

    let mut in_flight = FuturesUnordered::new();
    let mut next = 0;
    while next < total && in_flight.len() < thumbnail::FAN_OUT {
        in_flight.push(thumbnail_or_skip(device, &paths[next], max_dim));
        next += 1;
    }

    let mut done = 0;
    while let Some(result) = in_flight.next().await {
        done += 1;
        on_batch(ThumbBatch {
            thumbnails: result.into_iter().collect(),
            done,
            total,
        });
        if next < total {
            in_flight.push(thumbnail_or_skip(device, &paths[next], max_dim));
            next += 1;
        }
    }
    Ok(())
}

/// Batch worker: a per-file read/parse failure degrades to `None` (warned, not
/// fatal) so the streaming batch keeps producing for the rest of the grid.
async fn thumbnail_or_skip(device: &dyn Device, remote: &str, max_dim: u32) -> Option<Thumbnail> {
    match build_thumbnail(device, remote, max_dim).await {
        Ok(thumb) => thumb,
        Err(e) => {
            eprintln!("warning: skipping thumbnail for {remote}: {e}");
            None
        }
    }
}

/// Read the header window, locate an embedded thumbnail, resolve its bytes, and
/// re-encode to JPEG. `Ok(None)` means "no usable embedded thumbnail"; `Err`
/// means the device read failed.
async fn build_thumbnail(
    device: &dyn Device,
    remote: &str,
    max_dim: u32,
) -> Result<Option<Thumbnail>, DeviceError> {
    let header = device
        .read_range(remote, 0, thumbnail::READ_WINDOW_BYTES)
        .await
        .map_err(to_device_error)?;
    let Some(region) = thumbnail::locate(&header, remote) else {
        return Ok(None);
    };
    let candidate = resolve_region(device, remote, &header, region).await?;
    Ok(
        thumbnail::encode::to_jpeg_thumbnail(&candidate, max_dim).map(|encoded| Thumbnail {
            remote: remote.to_string(),
            width: encoded.width,
            height: encoded.height,
            format: ThumbFormat::Jpeg,
            bytes: encoded.bytes,
        }),
    )
}

/// Resolve a located region to its bytes: slice it straight from the header
/// window when it sits inside (no extra device round-trip), otherwise issue one
/// bounded read for it.
async fn resolve_region(
    device: &dyn Device,
    remote: &str,
    header: &[u8],
    region: thumbnail::ThumbRegion,
) -> Result<Vec<u8>, DeviceError> {
    let end = region.offset.saturating_add(region.len);
    if end <= header.len() as u64 {
        return Ok(header[region.offset as usize..end as usize].to_vec());
    }
    device
        .read_range(remote, region.offset, region.len)
        .await
        .map_err(to_device_error)
}

/// A fully rendered share card: the projected data, the SVG, and the rasterized
/// PNG bytes. The GUI consumes this directly; the CLI writes the PNG to disk and
/// opens it. `Serialize` only — [`CardData`] holds `&'static str`, so it never
/// deserializes back into Rust.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedCard {
    pub data: CardData,
    pub svg: String,
    pub png: Vec<u8>,
}

/// Project the device into a share card and rasterize it. `now_unix` keeps the
/// projection deterministic; `redact` masks anything potentially personal.
pub async fn card(
    device: &dyn Device,
    now_unix: i64,
    redact: bool,
) -> Result<RenderedCard, DeviceError> {
    let data = data::collect(device, now_unix, redact)
        .await
        .map_err(to_device_error)?;
    let svg = render::render_svg(&data);
    let png = png::svg_to_png(&svg).map_err(to_device_error)?;
    Ok(RenderedCard { data, svg, png })
}

/// Request a soft reboot. Returns once the device acknowledges.
pub async fn reboot(device: &dyn Device) -> Result<(), DeviceError> {
    device.reboot().await.map_err(to_device_error)
}

/// Request a power-off. Returns once the device acknowledges.
pub async fn shutdown(device: &dyn Device) -> Result<(), DeviceError> {
    device.shutdown().await.map_err(to_device_error)
}

/// Open a streaming syslog session. Each item is one parsed [`LogEntry`] (or a
/// per-line error). The CLI renders these in a TUI; `--json` prints NDJSON; the
/// GUI bridges the receiver to Tauri events.
pub async fn stream_logs(
    device: &dyn Device,
) -> Result<Receiver<anyhow::Result<LogEntry>>, DeviceError> {
    device.stream_logs().await.map_err(to_device_error)
}

#[cfg(test)]
mod thumbnail_tests {
    //! End-to-end tests of the `thumbnail`/`thumbnails` facade against a
    //! `FakeDevice` seeded with a real JPEG carrying an embedded EXIF thumbnail.
    //! These live in-crate (not in `tests/facade.rs`) because building and
    //! decoding the fixture needs the `image` crate, a normal dependency only
    //! the crate itself can `use`.
    use super::*;
    use crate::device::FakeDevice;
    use crate::logic::thumbnail::encode::solid_jpeg as jpeg;
    use std::sync::{Arc, Mutex};

    /// A JPEG file whose EXIF `APP1` IFD1 carries `thumb` as its embedded
    /// thumbnail. Layout mirrors what `jpeg::exif_thumbnail` walks.
    fn jpeg_with_exif_thumbnail(thumb: &[u8]) -> Vec<u8> {
        // TIFF (big-endian): empty IFD0, IFD1 with thumb offset/length tags.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&0x002Au16.to_be_bytes());
        tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 @ 8
        tiff.extend_from_slice(&0u16.to_be_bytes()); // IFD0: 0 entries
        tiff.extend_from_slice(&14u32.to_be_bytes()); // next IFD (IFD1) @ 14
        tiff.extend_from_slice(&2u16.to_be_bytes()); // IFD1: 2 entries
        const THUMB_OFFSET_TAG: u16 = 0x0201;
        const THUMB_LENGTH_TAG: u16 = 0x0202;
        const TIFF_TYPE_LONG: u16 = 4;
        // IFD1 ends at TIFF offset 44 (16 + 2 entries × 12 + 4-byte next ptr),
        // where the thumbnail bytes are appended.
        const THUMB_TIFF_OFFSET: u32 = 44;
        let mut entry = |tag: u16, value: u32| {
            tiff.extend_from_slice(&tag.to_be_bytes());
            tiff.extend_from_slice(&TIFF_TYPE_LONG.to_be_bytes());
            tiff.extend_from_slice(&1u32.to_be_bytes());
            tiff.extend_from_slice(&value.to_be_bytes());
        };
        entry(THUMB_OFFSET_TAG, THUMB_TIFF_OFFSET);
        entry(THUMB_LENGTH_TAG, thumb.len() as u32);
        tiff.extend_from_slice(&0u32.to_be_bytes()); // no further IFD
        assert_eq!(tiff.len() as u32, THUMB_TIFF_OFFSET);
        tiff.extend_from_slice(thumb);

        let mut app1 = Vec::new();
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&tiff);
        let seg_len = (app1.len() + 2) as u16;

        let mut file = vec![0xFF, 0xD8, 0xFF, 0xE1];
        file.extend_from_slice(&seg_len.to_be_bytes());
        file.extend_from_slice(&app1);
        file.extend_from_slice(&[0xFF, 0xD9]); // EOI
        file
    }

    fn fake_with_files(files: &[(&str, Vec<u8>)]) -> FakeDevice {
        let range_files = files
            .iter()
            .map(|(path, bytes)| (path.to_string(), bytes.clone()))
            .collect();
        FakeDevice {
            range_files,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn thumbnail_extracts_and_downscales_embedded_jpeg() {
        let path = "/DCIM/100APPLE/IMG_0001.JPG";
        let fake = fake_with_files(&[(path, jpeg_with_exif_thumbnail(&jpeg(120, 60)))]);

        let thumb = app_thumbnail(&fake, path).await;
        assert_eq!(thumb.remote, path);
        assert_eq!(thumb.format, ThumbFormat::Jpeg);
        // 120×60 thumbnail downscaled to fit max_dim 64 on the long edge.
        assert_eq!((thumb.width, thumb.height), (64, 32));
        assert_eq!(&thumb.bytes[0..3], &[0xFF, 0xD8, 0xFF]); // valid JPEG
    }

    /// A file with no recognizable container yields `None`, not an error — the
    /// GUI keeps its kind icon.
    #[tokio::test]
    async fn thumbnail_is_none_for_non_media() {
        let path = "/Downloads/manual.pdf";
        let fake = fake_with_files(&[(path, b"%PDF-1.7 not an image".to_vec())]);
        let result = thumbnail(&fake, path, 64).await.expect("ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn thumbnails_streams_one_batch_per_file_and_counts_progress() {
        let jpg = "/DCIM/100APPLE/IMG_0001.JPG";
        let pdf = "/Downloads/manual.pdf";
        let fake = fake_with_files(&[
            (jpg, jpeg_with_exif_thumbnail(&jpeg(80, 80))),
            (pdf, b"%PDF not an image".to_vec()),
        ]);
        let paths = vec![jpg.to_string(), pdf.to_string()];

        let collected: Arc<Mutex<Vec<ThumbBatch>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        thumbnails(
            &fake,
            &paths,
            64,
            Box::new(move |batch| sink.lock().unwrap().push(batch)),
        )
        .await
        .expect("ok");

        let batches = collected.lock().unwrap();
        // One batch per processed file; cumulative done reaches total.
        assert_eq!(batches.len(), 2);
        assert_eq!(batches.last().unwrap().done, 2);
        assert!(batches.iter().all(|b| b.total == 2));
        // Exactly the JPEG produced a thumbnail; the PDF advanced done only.
        let produced: usize = batches.iter().map(|b| b.thumbnails.len()).sum();
        assert_eq!(produced, 1);
        let thumb = batches
            .iter()
            .flat_map(|b| &b.thumbnails)
            .next()
            .expect("one thumbnail");
        assert_eq!(thumb.remote, jpg);
    }

    /// Convenience: unwrap the single-file facade to a `Thumbnail`, using the
    /// 64-pixel test `max_dim`.
    async fn app_thumbnail(device: &dyn Device, remote: &str) -> Thumbnail {
        thumbnail(device, remote, 64)
            .await
            .expect("ok")
            .expect("some thumbnail")
    }
}
