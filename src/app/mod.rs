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

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Receiver;

use crate::card::data::{self, CardData};
use crate::card::{png, render};
use crate::device::{
    App, BatchCallback, Device, DeviceError, DeviceInfo, DeviceStatus, LogEntry, MediaFile,
    WalkCallback,
};
use crate::logic::{analyze, media};

// Re-export the report DTO that lives next to its pure builders, so callers
// reach the whole facade surface through `app::`.
pub use crate::logic::media::MediaReport;

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
