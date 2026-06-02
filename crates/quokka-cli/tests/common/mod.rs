//! Shared helpers for the e2e suites.
//!
//! Lives in a subdirectory so cargo treats it as a plain module (pulled in via
//! `mod common;`) rather than its own test binary. Only the e2e-gated crates
//! reference it, so it never compiles into the default `cargo test` run.
#![allow(dead_code)]

use anyhow::Result;
use quokka_cli::device::{self, Device, Platform};

/// Resolve `QK_PLATFORM` (`ios` / `android`, case-insensitive) into a
/// [`Platform`], mirroring the `qk` binary's `--platform`/`QK_PLATFORM`
/// handling. Unset or unrecognised values fall back to autodetect (`None`).
pub fn env_platform() -> Option<Platform> {
    match std::env::var("QK_PLATFORM")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "ios" => Some(Platform::Ios),
        "android" => Some(Platform::Android),
        _ => None,
    }
}

/// Connect honoring `QK_UDID` / `QK_PLATFORM` — the same env vars the `qk`
/// binary reads. The e2e test process has no TTY, so `device::connect` can't
/// open a picker; with several devices attached across platforms it errors on
/// the ambiguity. Pointing these env vars at one device lets the suite run
/// regardless of what else is plugged in, instead of skipping.
pub async fn connect() -> Result<Box<dyn Device>> {
    let udid = std::env::var("QK_UDID").ok();
    device::connect(
        udid.as_deref(),
        env_platform(),
        &quokka_cli::ui::select_device,
    )
    .await
}
