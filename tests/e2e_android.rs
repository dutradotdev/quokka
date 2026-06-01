//! End-to-end smoke test for the Android backend. Drives the real `adb`
//! backend against a physical Android device over USB, through the same
//! library entry point as the integration tests. Run manually with
//! `cargo test --features e2e-android`. Never run in CI — CI only compiles it.

#![cfg(feature = "e2e-android")]

use std::time::Duration;

use quokka_cli::device::{self, Platform};

/// How long to sample logcat before declaring the stream healthy. Long enough
/// for a quiet device to emit something, short enough to keep the test snappy.
const LOGCAT_SAMPLE: Duration = Duration::from_secs(2);

#[tokio::test]
async fn connects_to_a_real_android_and_reads_real_app_sizes() {
    let dev = match device::connect(None, Some(Platform::Android)).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("e2e-android: skipping — no device available ({e})");
            return;
        }
    };

    let status = dev.status().await.expect("status() failed");
    eprintln!(
        "e2e-android: connected to {} (Android {})",
        status.model.as_deref().unwrap_or("<unknown>"),
        status.os_version.as_deref().unwrap_or("<unknown>"),
    );

    let info = dev.info().await.expect("info() failed");
    eprintln!("e2e-android: serial {}", info.serial);

    // The headline feature: real per-app sizes from `dumpsys diskstats`. At
    // least one installed app must report a non-zero size — that is the proof
    // the diskstats arrays parsed on this device/OEM, not just that the call
    // returned.
    let apps = dev.apps().await.expect("apps() failed");
    assert!(!apps.is_empty(), "expected at least one user app");
    let sized = apps.iter().filter(|a| a.size_bytes > 0).count();
    eprintln!(
        "e2e-android: {sized}/{} apps report a non-zero size",
        apps.len()
    );
    assert!(
        sized > 0,
        "no app reported a size — `dumpsys diskstats` parsing likely broke on this device"
    );

    // Media walk over the device's own roots — must reach files without the
    // walk erroring out on this Android version.
    let roots = dev.media_roots();
    let on_progress: device::WalkCallback = Box::new(|_progress| {});
    let media = dev
        .afc_walk(roots, on_progress)
        .await
        .expect("afc_walk() failed");
    eprintln!("e2e-android: walked {} media files", media.len());

    // Logcat for a couple of seconds — prove the stream opens and yields.
    let mut rx = dev.stream_logs().await.expect("stream_logs() failed");
    let mut seen = 0usize;
    let deadline = tokio::time::sleep(LOGCAT_SAMPLE);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            entry = rx.recv() => match entry {
                Some(Ok(_)) => seen += 1,
                Some(Err(e)) => {
                    eprintln!("e2e-android: logcat stream error: {e}");
                    break;
                }
                None => break,
            },
        }
    }
    eprintln!("e2e-android: read {seen} logcat entries");
}
