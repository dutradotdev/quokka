//! Integration tests for the application facade (`quokka_core::app`) — the
//! surface the CLI, `--json`, and the GUI all consume. These assert on the
//! returned DTOs (not on rendered text) and pin the serializable contract via
//! serde round-trips. No iPhone needed — everything runs against `FakeDevice`.

use quokka_core::app;
use quokka_core::device::{DeviceError, DeviceInfo, FakeDevice, MediaFile};

mod common;
use common::{assert_round_trips, noop_batch, noop_walk, NOW};

#[tokio::test]
async fn status_returns_seeded_snapshot() {
    let fake = FakeDevice::default();
    let status = app::status(&fake).await.expect("status ok");
    assert_eq!(status.name.as_deref(), Some("Test iPhone"));
}

#[tokio::test]
async fn status_error_surfaces_as_device_error() {
    let fake = FakeDevice::with_status_error("simulated lockdown failure");
    let err = app::status(&fake).await.expect_err("should error");
    // The fake's stringly error isn't a typed DeviceError, so it lands in
    // `Other` — but it still carries the message and serializes cleanly.
    assert!(matches!(err, DeviceError::Other(_)));
    assert!(err.to_string().contains("simulated lockdown failure"));
    let json = serde_json::to_value(&err).expect("serialize");
    assert_eq!(json["kind"], "Other");
}

#[tokio::test]
async fn info_redacts_pii_when_requested() {
    let fake = FakeDevice {
        info: DeviceInfo {
            name: "Lucas's iPhone".into(),
            model_identifier: "iPhone16,2".into(),
            serial: "F2LXXXXXXXXX".into(),
            udid: "00008130-001A2B3C".into(),
            os_version: "18.2".into(),
            imei: Some("350123456789012".into()),
            ..DeviceInfo::default()
        },
        ..Default::default()
    };
    let plain = app::info(&fake, false).await.expect("info ok");
    assert_eq!(plain.serial, "F2LXXXXXXXXX");
    let masked = app::info(&fake, true).await.expect("info ok");
    assert!(masked.serial.starts_with("***…"));
    assert_eq!(masked.imei.as_deref(), Some("***…9012"));
    // Non-sensitive fields stay readable.
    assert_eq!(masked.name, "Lucas's iPhone");
}

#[tokio::test]
async fn media_builds_report_and_round_trips() {
    let fake = FakeDevice::default();
    let report = app::media(&fake, true, noop_walk())
        .await
        .expect("media ok");
    assert!(report.total_files > 0);
    assert!(report.duplicates.is_some());
    assert_round_trips(&report);
}

#[tokio::test]
async fn thumbnail_is_none_for_non_media_file() {
    // A file with no recognizable container yields `Ok(None)` — never an error.
    // The positive extraction path is unit-tested in-crate (it needs the
    // `image` codec); here we pin the facade's "no thumbnail" contract.
    let mut range_files = std::collections::HashMap::new();
    range_files.insert("/Downloads/manual.pdf".to_string(), b"%PDF-1.7".to_vec());
    let fake = FakeDevice {
        range_files,
        ..Default::default()
    };
    let result = app::thumbnail(&fake, "/Downloads/manual.pdf", 256)
        .await
        .expect("thumbnail ok");
    assert!(result.is_none());
}

#[test]
fn thumb_batch_round_trips() {
    // Pin the IPC payload the GUI mirrors in `bindings.ts`.
    use quokka_core::app::{ThumbBatch, ThumbFormat, Thumbnail};
    let batch = ThumbBatch {
        thumbnails: vec![Thumbnail {
            remote: "/DCIM/100APPLE/IMG_0001.JPG".into(),
            width: 256,
            height: 128,
            format: ThumbFormat::Jpeg,
            bytes: vec![0xFF, 0xD8, 0xFF, 0x00, 0x01],
        }],
        done: 1,
        total: 4,
    };
    assert_round_trips(&batch);
    // `format` serializes camelCase ("jpeg"), like the other DTO enums.
    let json = serde_json::to_value(&batch).expect("serialize");
    assert_eq!(json["thumbnails"][0]["format"], "jpeg");
}

#[tokio::test]
async fn analyze_sorts_files_and_flags_live_photos() {
    // A .MOV next to a matching .HEIC is the live-photo-motion signal.
    let fake = FakeDevice {
        media: vec![
            MediaFile {
                path: "/DCIM/100APPLE/IMG_0001.MOV".into(),
                size_bytes: 50,
                modified_unix: 1_700_000_000,
            },
            MediaFile {
                path: "/DCIM/100APPLE/IMG_0001.HEIC".into(),
                size_bytes: 9000,
                modified_unix: 1_700_000_000,
            },
        ],
        ..Default::default()
    };
    let report = app::analyze(&fake, NOW, noop_walk())
        .await
        .expect("analyze ok");
    assert_eq!(report.total_files, 2);
    // Sorted largest-first.
    assert_eq!(report.files[0].size_bytes, 9000);
    let live = report
        .marks
        .iter()
        .find(|m| m.label.contains("Live Photo"))
        .expect("live-photo mark present");
    assert_eq!(live.paths, vec!["/DCIM/100APPLE/IMG_0001.MOV".to_string()]);
    assert_round_trips(&report);
}

#[tokio::test]
async fn delete_files_records_each_path() {
    let fake = FakeDevice::default();
    let paths = vec!["/DCIM/103APPLE/IMG_4521.MOV".to_string()];
    let outcome = app::delete_files(&fake, &paths).await.expect("delete ok");
    assert_eq!(outcome.deleted, paths);
    assert!(outcome.failed.is_empty());
    assert_eq!(*fake.deleted.lock().unwrap(), paths);
    assert_round_trips(&outcome);
}

#[tokio::test]
async fn pull_file_copies_bytes_and_reports_progress() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let fake = FakeDevice::default();
    let dest = std::env::temp_dir().join("quokka_facade_pull_file.bin");
    let reported = Arc::new(AtomicU64::new(0));
    let sink = reported.clone();

    app::pull_file(
        &fake,
        "/DCIM/103APPLE/IMG_4521.MOV",
        &dest,
        Box::new(move |p| sink.store(p.copied_bytes, Ordering::SeqCst)),
    )
    .await
    .expect("pull ok");

    assert_eq!(
        std::fs::read(&dest).expect("dest written"),
        fake.pull_payload
    );
    assert_eq!(fake.pulled(), vec!["/DCIM/103APPLE/IMG_4521.MOV"]);
    assert_eq!(
        reported.load(Ordering::SeqCst),
        fake.pull_payload.len() as u64
    );

    std::fs::remove_file(&dest).ok();
}

#[tokio::test]
async fn read_range_returns_requested_window() {
    let fake = FakeDevice::default();
    let total = fake.range_payload.len() as u64;

    let window = app::read_range(&fake, "/DCIM/103APPLE/IMG_4521.MOV", 200, 64)
        .await
        .expect("read_range ok");
    assert_eq!(window, fake.range_payload[200..264]);

    // Past EOF clamps to a short read (here, empty) rather than erroring.
    let past = app::read_range(&fake, "/DCIM/103APPLE/IMG_4521.MOV", total, 64)
        .await
        .expect("read_range ok");
    assert!(past.is_empty());
}

#[tokio::test]
async fn card_renders_png_and_serializes() {
    let fake = FakeDevice::default();
    let rendered = app::card(&fake, NOW, false).await.expect("card ok");
    assert!(!rendered.png.is_empty(), "PNG bytes should be produced");
    assert!(rendered.svg.contains("<svg"));
    // RenderedCard is Serialize-only (CardData holds &'static str): assert
    // it serializes to a JSON object with the keys the GUI reads.
    let value = serde_json::to_value(&rendered).expect("serialize");
    assert!(value.get("svg").is_some());
    assert!(value.get("png").is_some());
    assert!(value.get("data").is_some());
}

#[tokio::test]
async fn apps_enriches_and_returns_user_apps() {
    let fake = FakeDevice::default();
    let apps = app::apps(&fake, noop_batch()).await.expect("apps ok");
    assert!(!apps.is_empty());
}
