# `quokka storage` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Goal

A new read-only `qk storage` command that prints an iOS-Settings-style storage breakdown: **System · Apps · Media · Other · Free**, with one row per bucket, byte counts, percentages, and a small bar each. It is the "where did the 256 GB actually go" view.

Composes existing infrastructure — `device.status()`, `device.apps()` (with size enrichment, the same path `qk apps` uses), and `device.afc_walk()` (the same path `qk analyze` uses). No new trait methods are introduced.

## Why a new command and not extra fields on `status`

Producing the breakdown requires:

- Enumerating every installed app and reading each one's `house_arrest` Documents size (seconds, scales with app count).
- Walking `/var/mobile/Media` recursively over AFC (more seconds, scales with media volume).

That is 10–30s of work in practice. The dashboard / `qk status` must stay cheap — milliseconds, not seconds — so the breakdown gets its own command name where the user has explicit consent to pay the cost.

The dashboard's three-line breakdown (System / Data used / Free, from the dashboard spec) is **not** affected by this spec and remains the cheap default view.

## Non-goals

- No Messages, Mail, iCloud Drive, iMessage attachments, or other per-app sub-buckets. Lockdown does not expose them, and inferring them from AFC paths is unreliable. They fold into **Other**.
- No per-app or per-file drill-down. That is what `qk apps` and `qk analyze` already provide. `qk storage` is a fixed-five-row summary, period.
- No caching across runs. Each invocation re-enumerates and re-walks. Disk cache of the last walk is listed in future work alongside the same item from the `analyze` spec.
- No JSON output in v1. Listed in future work.
- No flag to embed this view inside the dashboard. Dashboard stays cheap. Listed in future work.

## User flow

1. User runs `qk storage`.
2. Multi-phase progress UI:
   - Phase 1 — `Reading device status…` (spinner, ~1s).
   - Phase 2 — `Enumerating apps and sizes… (12/87)` (progress bar driven by the same enrichment callback `qk apps` already wires up).
   - Phase 3 — `Walking media files… 1,234 files, 12.4 GB` (spinner with the `WalkProgress` callback the analyze spec defines).
3. After all three phases complete, the breakdown prints. Program exits `Ok`.

## Output

Target width 60 columns. Header line mirrors the dashboard's storage bar. Then one row per bucket: label (8 chars), small bar (8 chars), bytes (right-aligned 10 chars), percent (right-aligned 4 chars). Rows always print in the fixed order below — empty buckets still print (as `0 B    0%`), so the visual is consistent run-to-run.

Mock:

```
Storage  ████████░░ 60%  150.6 / 256 GB used

  System  ██░             12.4 GB     8%
  Apps    ████░           48.2 GB    30%
  Media   ███░            34.8 GB    22%
  Other   █░               9.8 GB     6%
  Free    ░░░░░░          95.4 GB    60%
```

- **System** — `status.system_bytes` (lockdown `TotalSystemCapacity`, added by the dashboard spec). If `None`, bucket renders `—   —%` and is excluded from the Other math (clamp to 0 contribution).
- **Apps** — `Σ app.size_bytes` from `device.apps()` after size enrichment completes. Apps reporting `None` for size contribute 0 (logged to stderr at the end as `Note: N apps reported no size and are not counted in Apps.` when N > 0).
- **Media** — `Σ file.size_bytes` from `device.afc_walk(MEDIA_ROOTS, ...)` over the same `MEDIA_ROOTS` constant the analyze spec defines (`DCIM`, `Downloads`, `Recordings`, `Books`).
- **Other** — `max(0, data_used_bytes - apps_bytes - media_bytes)`. The clamp catches the rare case where Apps+Media overshoots Data used (apps double-counting shared cache, AFC seeing system files, read drift between phases). When clamped to 0, append a single line to stderr: `Note: storage math overshot — Apps + Media exceeded Data used by X MB. "Other" clamped to 0.`
- **Free** — `status.free_bytes`.

Percentages are computed against `status.total_bytes` and rounded to nearest integer. Sum-to-100 is **not** forced; if rounding produces 99 or 101, that's fine — the eye does not notice and forcing it introduces fiddly logic.

## Architecture

### No new trait methods

`storage::run` is pure orchestration over existing trait methods:

```rust
pub async fn run(device: &dyn Device) -> Result<()> {
    let status = device.status().await?;
    let apps   = collect_apps_with_sizes(device).await?;   // drives enrichment to completion
    let media  = collect_media(device).await?;             // afc_walk over MEDIA_ROOTS

    let breakdown = build_breakdown(&status, &apps, &media);
    print!("{}", render(&breakdown));
    Ok(())
}
```

`build_breakdown` and `render` are pure functions, unit-testable end-to-end.

### `StorageBreakdown` struct (lives in `src/commands/storage.rs`, not the trait)

```rust
pub struct StorageBreakdown {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub system_bytes: Option<u64>,
    pub apps_bytes: u64,
    pub media_bytes: u64,
    pub other_bytes: u64,           // already clamped; never negative
    pub overshoot_bytes: u64,       // 0 unless clamp fired; drives the stderr note
    pub apps_missing_size: usize,   // count of apps whose size_bytes was None
}
```

Building this struct from `(DeviceStatus, &[AppEntry], &[MediaFile])` is one pure function. All edge cases (status fields missing, overshoot, divide-by-zero on percent) live inside it.

### Reuse of existing seams

- `collect_apps_with_sizes(device)` calls the same trait method `qk apps` uses, draining the enrichment stream to completion. Concretely: subscribe to the size-update channel and wait until every app has either reported a size or been marked terminal, then return the final list. The exact API is implementation-defined — the spec only requires "wait until apps' sizes are final."
- `collect_media(device)` calls `device.afc_walk(MEDIA_ROOTS, on_progress)` with a callback that updates the Phase 3 spinner. `MEDIA_ROOTS` is the same `const &[&str]` the analyze spec introduces.

If those two helpers grow into a shared module (`commands::shared::storage_math` or similar) later, that is a refactor — not a v1 concern. For v1, two private helpers in `storage.rs` are fine.

### Dispatch (`src/lib.rs`)

Add `Storage` subcommand to the clap enum. Dispatches to `commands::storage::run(&*device)`.

### Menu integration

Add `Storage` to the launcher menu between `Analyze` and `Info`:

```
Apps · Analyze · Storage · Info · Refresh · Reboot · Shutdown · Quit
```

Selecting `Storage` runs `storage::run(device)` and waits for Enter, same pattern as `Apps` / `Analyze`. The full progress UI plays inside the menu too — users see the same three phases.

## Error handling

- **`status` fails entirely**: same trust-and-replug guidance the other commands use. Abort `qk storage`. (Without status we can't render Total/Free, so partial output is useless.)
- **App enrichment partially fails**: continue with whatever sizes we have. Apps with `None` size contribute 0 and are counted in `apps_missing_size` for the stderr footer note.
- **AFC walk hits a permission error on a subdirectory**: same posture the analyze spec takes — log a warning to stderr, skip the subdir, continue.
- **AFC walk fails entirely (connect error)**: Media bucket renders as `—` and the stderr footer notes `Media unavailable — AFC connect failed. Numbers below exclude media files.`. Other math treats `media_bytes = 0` in this case.

All wrapping with `anyhow::Context` happens at the trait boundary as usual — no raw `idevice` strings in user-visible output.

## Testing

### Unit (`#[cfg(test)] mod tests` in `storage.rs`)

- `build_breakdown` with fully-populated inputs → exact byte counts and percents.
- `build_breakdown` with `system_bytes: None` → System renders `—`, Other math treats System as 0 contribution but does not double-subtract.
- `build_breakdown` with an Apps+Media sum that overshoots Data used → `other_bytes = 0`, `overshoot_bytes` is the excess. Render output contains the stderr note string (rendered via the same function — the test can intercept via a `Vec<String>` notes return).
- `build_breakdown` with some apps reporting `None` size → those contribute 0 to apps_bytes and `apps_missing_size` reflects the count.
- `render` with the canonical mock above produces byte-for-byte the mock string. (Snapshot.)
- Percent rounding edge cases — 0%, 100%, and the case where rounded percents sum to 99 / 101 (assert we do **not** force 100).

### Integration (`tests/integration.rs`)

- `FakeDevice::default()` seeded with the same `apps` and `media` fixtures used by the apps and analyze integration tests → `storage::run` returns `Ok` and the output contains the five fixed labels and a `Total` line.
- A `FakeDevice` with `media: vec![]` and no apps → all five bucket lines present, Apps and Media show `0 B`, Other shows `data_used` (no overshoot).
- A `FakeDevice` whose app fixture sums to more than data used → final output contains the overshoot note string, Other is `0 B`.

### E2E (behind `--features e2e`)

- Smoke: run `storage::run` against the real device. Assert it returns within a generous bound (e.g. 90s — apps + AFC walk can be slow on a 256 GB device), and that the output contains the five labels and that all numbers parse as valid byte counts. Do not assert on specific values.

## Future work (deferred)

- `--json` output for scripting.
- Disk cache of the last walk + enrichment (stale-while-revalidate), shared with `qk apps` and `qk analyze`. Listed in the analyze spec too — when it lands, all three commands benefit.
- `qk status --storage` (or a `dashboard --storage` flag) that embeds this view in the dashboard for a single rich snapshot.
- Splitting **Other** further if a stable lockdown / DVT path turns up (e.g. via the iOS 17+ RemoteXPC tunnel) — for now it stays a residual bucket.
- Best-effort sub-categorization of Media (Photos vs Videos vs Audio vs Books) using the same `kind_from_ext` helper the analyze spec defines. Trivial follow-up if asked.
