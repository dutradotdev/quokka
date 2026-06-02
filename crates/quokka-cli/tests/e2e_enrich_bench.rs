//! Performance sweep for `with_dynamic_sizes` against a real iPhone.
//!
//! Run with:
//! ```sh
//! cargo test --features e2e --test e2e_enrich_bench -- --nocapture --ignored
//! ```
//!
//! Marked `#[ignore]` so `cargo test --features e2e` (which exercises the
//! smoke test) doesn't spend 5+ minutes hammering installation_proxy. Opt in
//! explicitly when tuning.
//!
//! Prints a markdown table to stderr — no assertions. Treat the output as the
//! input to a decision, not as pass/fail.

#![cfg(feature = "e2e")]

use std::time::Duration;

use quokka_cli::device::{bench, App};

const RUNS_PER_COMBO: usize = 3;

#[tokio::test]
#[ignore]
async fn sweep_batch_and_concurrency() {
    let harness = match bench::Harness::connect().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("bench: skipping — no device available ({e})");
            return;
        }
    };

    let basic = harness.apps().await.expect("Phase 1 apps() failed");
    let user_apps: Vec<App> = basic.into_iter().filter(|a| !a.is_system).collect();
    if user_apps.is_empty() {
        eprintln!("bench: skipping — no user apps installed");
        return;
    }
    eprintln!(
        "bench: {} user apps, {} runs per combo, default = (batch={}, concurrent={})",
        user_apps.len(),
        RUNS_PER_COMBO,
        bench::DEFAULT_BATCH_SIZE,
        bench::DEFAULT_MAX_CONCURRENT,
    );

    // Grid covers under/over the current defaults in both axes. Skip combos
    // that would produce fewer batches than `max_concurrent` (concurrency is
    // capped by batches anyway, so they degenerate).
    let batch_sizes = [4_usize, 8, 16, 32];
    let max_concurrents = [2_usize, 4, 8];

    eprintln!();
    eprintln!("| batch | concurrent | runs (ms)         | median (ms) |");
    eprintln!("|-------|------------|-------------------|-------------|");

    let mut results: Vec<(usize, usize, Duration)> = Vec::new();

    for &bs in &batch_sizes {
        for &mc in &max_concurrents {
            let mut times = Vec::with_capacity(RUNS_PER_COMBO);
            for run in 0..RUNS_PER_COMBO {
                match harness.enrich_timed(user_apps.clone(), bs, mc).await {
                    Ok(d) => times.push(d),
                    Err(e) => {
                        eprintln!(
                            "bench: (batch={bs}, concurrent={mc}) run {} errored: {e}",
                            run + 1
                        );
                    }
                }
            }
            if times.is_empty() {
                continue;
            }
            times.sort();
            let median = times[times.len() / 2];
            let runs_str = times
                .iter()
                .map(|d| format!("{:>5}", d.as_millis()))
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!(
                "| {bs:>5} | {mc:>10} | {runs_str} | {:>11} |",
                median.as_millis()
            );
            results.push((bs, mc, median));
        }
    }

    if let Some((bs, mc, best)) = results.iter().min_by_key(|(_, _, d)| *d) {
        let default = results
            .iter()
            .find(|(b, c, _)| {
                *b == bench::DEFAULT_BATCH_SIZE && *c == bench::DEFAULT_MAX_CONCURRENT
            })
            .map(|(_, _, d)| *d);
        eprintln!();
        eprintln!(
            "bench: fastest = (batch={bs}, concurrent={mc}) at {} ms median",
            best.as_millis()
        );
        if let Some(d) = default {
            let speedup = d.as_secs_f64() / best.as_secs_f64();
            eprintln!(
                "bench: default ({}, {}) = {} ms median ({:.2}× slower than best)",
                bench::DEFAULT_BATCH_SIZE,
                bench::DEFAULT_MAX_CONCURRENT,
                d.as_millis(),
                speedup,
            );
        }
    }
}
