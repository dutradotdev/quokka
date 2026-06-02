//! Shared helpers for the `quokka-core` integration tests (facade + GUI
//! contract). In a subdirectory so cargo treats it as a plain module pulled in
//! via `mod common;`, not its own test binary. Different test crates use
//! different subsets, hence `allow(dead_code)`.
#![allow(dead_code)]

use quokka_core::device::{BatchCallback, BatchUpdate, WalkCallback, WalkProgress};

/// Fixed "now" for age-based heuristics so reports are deterministic.
/// 2024-05-28 UTC.
pub const NOW: i64 = 1_716_854_400;

pub fn noop_walk() -> WalkCallback {
    Box::new(|_p: WalkProgress| {})
}

pub fn noop_batch() -> BatchCallback {
    Box::new(|_u: BatchUpdate| {})
}

/// Serialize → deserialize → serialize is a fixed point, pinning the IPC
/// payload the CLI's `--json` and the GUI's frontend both consume.
pub fn assert_round_trips<T>(value: &T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    let json = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    let again = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, again, "round-trip changed the payload");
}
