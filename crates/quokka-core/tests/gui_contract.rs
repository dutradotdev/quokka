//! GUI contract spike — proves the Rust side of the future Tauri GUI before a
//! line of Tauri is written.
//!
//! The vision (`docs/vision.md`, Fase 2) is that the GUI consumes the **data**,
//! not the CLI's text: the private Tauri repo depends on `quokka-core`, holds a
//! `Device` in managed state, wraps each [`app`] function in a `#[tauri::command]`,
//! and renders the serialized DTOs in JS. That hides exactly three integration
//! surfaces where "just wrap it" is actually real glue — this test exercises all
//! three against `FakeDevice`, so a regression here fails the contract long
//! before the GUI does:
//!
//! 1. **Shared state** — the device lives in `Arc<dyn Device>` (Tauri managed
//!    state) and is driven across tasks, like commands firing concurrently.
//! 2. **Serializable DTOs** — every facade return value round-trips through
//!    serde, the IPC the frontend reads.
//! 3. **Streaming → events** — the enrichment / walk callbacks and the log
//!    `Receiver` bridge to channels, the shape `AppHandle::emit` / `ipc::Channel`
//!    consume.
//!
//! No iPhone needed — everything runs against `FakeDevice`.

use std::sync::Arc;

use quokka_core::app;
use quokka_core::device::{
    BatchCallback, BatchUpdate, Device, DeviceError, FakeDevice, LogEntry, LogLevel, WalkCallback,
    WalkProgress,
};

mod common;
use common::{assert_round_trips, noop_batch, noop_walk, NOW};

/// Surface 1: the device is held as `Arc<dyn Device>` — Tauri managed state —
/// and driven from a spawned task. If `Device` weren't `Send + Sync + 'static`
/// and the facade futures `Send`, this wouldn't compile; that is precisely what
/// `tauri::State<AppState>` + async commands require.
#[tokio::test]
async fn device_lives_in_shared_state_and_is_driven_across_tasks() {
    let state: Arc<dyn Device> = Arc::new(FakeDevice::default());

    // One "command" runs on a spawned task while another runs inline, both
    // borrowing the same shared device — the concurrent-commands shape.
    let bg = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { app::status(&*state).await.map(|s| s.name) })
    };

    let info = app::info(&*state, false).await.expect("info ok");
    assert_eq!(info.model_identifier, "iPhone16,2");

    let name = bg.await.expect("task joins").expect("status ok");
    assert_eq!(name.as_deref(), Some("Test iPhone"));

    // Action commands (no DTO) also go through the same shared handle.
    app::reboot(&*state).await.expect("reboot ok");
}

/// Surface 2: every query DTO the GUI reads survives the IPC round-trip.
/// `card` is `Serialize`-only by design (`CardData` holds `&'static str`), so it
/// is checked as a one-way serialize with the keys the frontend reads.
#[tokio::test]
async fn every_facade_dto_serializes_for_ipc() {
    let device = FakeDevice::default();

    assert_round_trips(&app::status(&device).await.expect("status"));
    assert_round_trips(&app::info(&device, false).await.expect("info"));
    assert_round_trips(&app::info(&device, true).await.expect("info redacted"));
    assert_round_trips(&app::apps(&device, noop_batch()).await.expect("apps"));
    assert_round_trips(&app::all_apps(&device).await.expect("all_apps"));
    assert_round_trips(
        &app::analyze(&device, NOW, noop_walk())
            .await
            .expect("analyze"),
    );
    assert_round_trips(&app::media(&device, true, noop_walk()).await.expect("media"));

    let paths = vec!["/DCIM/103APPLE/IMG_4521.MOV".to_string()];
    assert_round_trips(&app::delete_files(&device, &paths).await.expect("delete"));

    // RenderedCard: serialize-only. The GUI reads { data, svg, png }.
    let card = app::card(&device, NOW, false).await.expect("card");
    let value = serde_json::to_value(&card).expect("serialize card");
    for key in ["data", "svg", "png"] {
        assert!(value.get(key).is_some(), "card JSON missing `{key}`");
    }
}

/// Surface 2 (errors): failures reach the frontend as a stable, branchable
/// `{ kind, message }` object — not an opaque string or a panic.
#[tokio::test]
async fn errors_serialize_as_kind_and_message() {
    let device = FakeDevice::with_status_error("lockdown refused the connection");
    let err = app::status(&device).await.expect_err("should fail");
    assert!(matches!(err, DeviceError::Other(_)));

    let value = serde_json::to_value(&err).expect("serialize error");
    assert_eq!(value["kind"], "Other");
    assert_eq!(value["message"], "lockdown refused the connection");
}

/// Surface 3a: the progress callbacks (`apps` enrichment, `analyze`/`media`
/// walk) bridge to a channel carrying serialized payloads — exactly what a
/// `move |update| app_handle.emit("…", update)` closure does in the GUI.
#[tokio::test]
async fn progress_callbacks_bridge_to_serialized_event_channels() {
    let device = FakeDevice::default();

    // apps enrichment → BatchUpdate events.
    let (batch_tx, mut batch_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let on_batch: BatchCallback = Box::new(move |u: BatchUpdate| {
        // The GUI would `emit` this JSON; serializing here proves the payload
        // crosses IPC. (BatchUpdate gained `Serialize` for exactly this.)
        let json = serde_json::to_string(&u).expect("serialize batch update");
        let _ = batch_tx.send(json);
    });
    app::apps(&device, on_batch).await.expect("apps ok");

    let mut batches = Vec::new();
    while let Some(json) = batch_rx.recv().await {
        let back: BatchUpdate = serde_json::from_str(&json).expect("batch round-trips");
        batches.push(back);
    }
    assert!(
        !batches.is_empty(),
        "no enrichment event reached the channel"
    );
    let last = batches.last().unwrap();
    assert_eq!(last.done, last.total, "final batch should be complete");

    // media walk → WalkProgress events.
    let (walk_tx, mut walk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let on_progress: WalkCallback = Box::new(move |p: WalkProgress| {
        let json = serde_json::to_string(&p).expect("serialize walk progress");
        let _ = walk_tx.send(json);
    });
    app::analyze(&device, NOW, on_progress)
        .await
        .expect("analyze ok");

    let mut progress = Vec::new();
    while let Some(json) = walk_rx.recv().await {
        let back: WalkProgress = serde_json::from_str(&json).expect("progress round-trips");
        progress.push(back);
    }
    assert!(!progress.is_empty(), "no walk progress reached the channel");
    assert!(progress.last().unwrap().files_seen > 0);
}

/// Surface 3b: the log stream is a `Receiver<Result<LogEntry>>`. The GUI drains
/// it on a task and `emit`s each entry; here we drain it and serialize each one,
/// proving the receiver-to-events bridge and that `LogEntry` crosses IPC.
#[tokio::test]
async fn log_stream_receiver_bridges_to_serialized_events() {
    let seeded = vec![
        Ok(LogEntry {
            timestamp_unix_ms: Some(1_716_854_400_000),
            time_text: Some("12:00:00".into()),
            host: "iPhone".into(),
            process: "SpringBoard".into(),
            pid: Some(123),
            level: LogLevel::Notice,
            message: "boot complete".into(),
        }),
        Ok(LogEntry {
            timestamp_unix_ms: Some(1_716_854_401_000),
            time_text: Some("12:00:01".into()),
            host: "iPhone".into(),
            process: "kernel".into(),
            pid: None,
            level: LogLevel::Error,
            message: "thermal pressure".into(),
        }),
    ];
    let device = FakeDevice {
        seeded_logs: seeded,
        ..Default::default()
    };

    let mut rx = app::stream_logs(&device).await.expect("stream opens");
    let mut emitted = Vec::new();
    while let Some(item) = rx.recv().await {
        let entry = item.expect("log entry ok");
        emitted.push(serde_json::to_string(&entry).expect("serialize log entry"));
    }

    assert_eq!(
        emitted.len(),
        2,
        "both seeded entries should stream through"
    );
    let first: LogEntry = serde_json::from_str(&emitted[0]).expect("entry round-trips");
    assert_eq!(first.process, "SpringBoard");
}

/// Platform-only capabilities are a queryable seam, not a platform branch. The
/// GUI gates its capture UI on `as_capture().is_some()` — `Some` on iOS-shaped
/// backends, `None` on Android — the same way `device_action` hides the row in
/// the CLI. No `if ios` anywhere.
#[tokio::test]
async fn capture_capability_is_a_queryable_seam() {
    let device = FakeDevice::default();
    assert!(
        device.as_capture().is_some(),
        "the iOS-shaped fake exposes capture via the extension trait"
    );
}
