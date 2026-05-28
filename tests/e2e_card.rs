//! End-to-end test for `qk card` against a real connected iPhone.
//!
//! Gated behind the `e2e` feature. CI compile-checks this file but never
//! runs it — that's intentional: nothing here is meaningful without an
//! iPhone over USB. Run locally with:
//!
//! ```sh
//! cargo test --features e2e --test e2e_card
//! ```

#![cfg(feature = "e2e")]

use std::path::PathBuf;

use quokka_cli::commands::card;
use quokka_cli::device;
use quokka_cli::ui::now_unix;

#[tokio::test]
async fn card_run_against_real_device_writes_a_1080x1080_png() {
    let device = device::connect(None)
        .await
        .expect("a paired iPhone must be connected for this test");

    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let png_path: PathBuf = tmp.path().with_extension("png");

    card::run(
        &*device,
        now_unix(),
        card::CardArgs {
            output: Some(png_path.clone()),
            no_open: true,
            redact: false,
        },
    )
    .await
    .expect("card::run should succeed against a real device");

    let bytes = std::fs::read(&png_path).expect("PNG written");
    assert!(
        bytes.len() >= 50_000,
        "PNG suspiciously small: {} bytes",
        bytes.len()
    );
    let (w, h) = card::png::read_png_dimensions(&bytes).expect("valid PNG IHDR");
    assert_eq!((w, h), (1080, 1080));
}
