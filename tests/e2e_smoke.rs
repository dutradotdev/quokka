//! End-to-end smoke tests. Drive the real `idevice` backend against a
//! physical iPhone over USB. Run manually with `cargo test --features e2e`.

#![cfg(feature = "e2e")]

use quokka_cli::device;

#[tokio::test]
async fn connects_to_a_real_device_and_reads_its_status() {
    let dev = match device::connect().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("e2e: skipping — no device available ({e})");
            return;
        }
    };
    let status = dev.status().await.expect("status() failed");
    eprintln!(
        "e2e: connected to {} (iOS {})",
        status.name.as_deref().unwrap_or("<unknown>"),
        status.ios_version.as_deref().unwrap_or("<unknown>"),
    );
}
