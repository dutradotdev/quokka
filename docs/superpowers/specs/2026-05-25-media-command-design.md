# `qk media` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Summary

A read-only `qk media` command that surveys the AFC-accessible media area and prints a single static report: counts and sizes per kind (Photos / Videos / Audio / Other), a per-month breakdown for the last 12 months, the 10 largest individual files, and — when `--find-duplicates` is set — a list of likely-duplicate groups identified by exact size match.

## Motivation

`qk analyze` finds the heaviest individual files so the user can delete them. `qk apps` shows which apps are eating space. Neither answers the broader question "what's the shape of my camera roll" — how many photos vs videos, when they were taken, are there obvious duplicates. `qk media` fills that gap without overlapping the other two.

## Scope

### In scope

- Walk `/var/mobile/Media` over AFC and report:
  - Total file count and bytes.
  - Per-kind buckets (Photo / Video / Audio / Other).
  - Per-month buckets (last 12 months).
  - Top 10 largest files.
- Optional `--find-duplicates` heuristic: group by `(size_bytes, kind)`, flag groups with `count > 1`.

### Out of scope

- Deletion. `qk analyze --delete` already covers that. `qk media` is read-only.
- Content-hash duplicate detection in v1. Listed in future work. v1 ships a cheap heuristic (exact size + kind match) that catches the common cases without pulling file bodies.
- EXIF / dimension / codec parsing. AFC gives us name, size, mtime. That is enough for v1.
- Live Photo pairing (treating IMG.HEIC + IMG.MOV as one item). Each file is one row. Listed in future work.
- "By location" grouping. AFC does not expose geotags; that lives inside the Photos database, which is unreachable.

## CLI surface

### Synopsis

```
quokka media [-d | --find-duplicates]
```

- `--find-duplicates` / `-d` (default off): also compute and print the likely-duplicates section. Adds no AFC traffic — pure post-processing over the walk result.

### Behavior

1. User runs `qk media` (optionally with `--find-duplicates` / `-d`).
2. Spinner during the walk: `Walking media files… 1,234 files, 12.4 GB` (same `WalkProgress` callback the analyze spec defines).
3. Walk completes. The report prints in fixed sections, in this order:
   - **Header line** — total files and bytes scanned.
   - **By kind** — one row per Photo / Video / Audio / Other.
   - **By month (last 12)** — one row per calendar month, derived from mtime.
   - **Largest 10** — the 10 biggest individual files.
   - **Likely duplicates** — only present when `--find-duplicates` was passed.
4. Program exits `Ok`.

Target width 60 columns. Section headers are bare lines; rows under them are two-space indented.

Mock with `--find-duplicates`:

```
Media on Lucas's iPhone
Scanned 4,217 files (68.4 GB) under DCIM, Recordings, Books, Downloads

By kind
  Photos     ████████░░    3,891 files   ·   42.2 GB
  Videos     ████░░░░░░      267 files   ·   24.1 GB
  Audio      ░░░░░░░░░░       31 files   ·    1.9 GB
  Other      ░░░░░░░░░░       28 files   ·    0.2 GB

By month (last 12)
  2026-05    421 files   ·    8.2 GB  ████████
  2026-04    312 files   ·    6.1 GB  ██████
  2026-03    287 files   ·    5.5 GB  █████
  ...
  2025-06    198 files   ·    3.1 GB  ███

Largest 10
  4.21 GB  Video  /DCIM/103APPLE/IMG_4521.MOV   (2026-05-12)
  2.88 GB  Video  /DCIM/103APPLE/IMG_4519.MOV   (2026-05-11)
  ...

Likely duplicates  (exact size match — heuristic, may include false positives)
  14 groups, 28 files, 1.2 GB potential savings
    4.21 GB  Video  × 2  /DCIM/103APPLE/IMG_4521.MOV + 1 other
    150 MB   Photo  × 3  /DCIM/102APPLE/IMG_1234.HEIC + 2 others
    ...
```

Bars in **By kind** are sized against the largest bucket. Bars in **By month** are sized against the largest month. Both clamp to 10 segments. Files with no mtime fall into a single `Unknown` bucket at the bottom of **By month** (shown only when non-empty).

If a section has zero rows (e.g. no audio at all), the section header still prints with a single line: `Audio   no files`. Consistent structure beats conditional sections — easier to read, easier to test.

#### Duplicate detection (v1, cheap heuristic)

Files are grouped by the tuple `(size_bytes, kind)`. Any group with `count > 1` is a "likely duplicate group". Output shows:

- Total group count, total file count, total potential savings (`Σ (size_bytes × (count - 1))`).
- Top 10 groups by potential savings, with: total size of one copy, kind, count, path of first file plus "+ N others".

This is the YAGNI version. Two distinct photos almost never have byte-identical sizes, so the false-positive rate is low in practice. **It does not pull file bodies** — uses only `MediaFile` data the walk already collected.

Confident duplicate detection (header-hash or full content hash) is future work.

## Architecture

### Device trait changes

One tiny extension to `MediaFile` (introduced by the analyze spec) — add `mtime_unix`:

```rust
pub struct MediaFile {
    pub path: String,
    pub size_bytes: u64,
    pub mtime_unix: Option<i64>,   // ← new; seconds since epoch from AFC get_file_info
}
```

`mtime_unix` is `Option` because AFC may return entries without a usable mtime (rare, but defensive). The analyze command ignores the new field. `FakeDevice` is updated to populate it when seeding.

The `mod real` implementation of `afc_walk` reads this from `get_file_info`'s `st_mtime` (verify the exact key name during implementation; libimobiledevice uses `st_mtime`).

No new trait methods. `media::run` is pure orchestration over the existing `afc_walk`.

### Real implementation notes

- `media::run` calls `device.afc_walk(MEDIA_ROOTS, on_progress)` with the same `MEDIA_ROOTS` constant the analyze spec defines (`DCIM`, `Downloads`, `Recordings`, `Books`).
- The on_progress callback updates the spinner with cumulative file count + bytes.

### FakeDevice additions

Extend the existing `media: Vec<MediaFile>` fixture with `mtime_unix` populated. Seed at least:

- A handful of photos spread across recent months (drives the **By month** test).
- An obvious duplicate pair (same size + same kind, different paths) for the duplicate test.
- One entry with `mtime_unix: None` for the Unknown-bucket test.

### Command module structure

`src/commands/media.rs`:

```rust
pub async fn run(device: &dyn Device, find_duplicates: bool) -> Result<()> {
    let files = collect_media(device).await?;   // same MEDIA_ROOTS + afc_walk as analyze
    let report = build_report(&files, find_duplicates, /* today */ Utc::now());
    print!("{}", render(&report));
    Ok(())
}
```

`MediaReport` lives in `src/commands/media.rs`, not the trait:

```rust
pub struct MediaReport {
    pub total_files: usize,
    pub total_bytes: u64,
    pub roots: &'static [&'static str],

    pub by_kind: [(Kind, usize, u64); 4],          // fixed order: Photo, Video, Audio, Other
    pub by_month: Vec<(YearMonth, usize, u64)>,    // sorted desc; capped to 12 + optional Unknown
    pub largest: Vec<MediaFile>,                   // top 10, sorted by size desc
    pub duplicates: Option<DuplicateReport>,
}

pub struct DuplicateReport {
    pub group_count: usize,
    pub file_count: usize,
    pub potential_savings_bytes: u64,
    pub top_groups: Vec<DuplicateGroup>,           // top 10 by savings
}

pub struct DuplicateGroup {
    pub size_bytes: u64,
    pub kind: Kind,
    pub paths: Vec<String>,                        // length >= 2
}
```

`Kind` is the existing enum (or `&'static str`) the analyze spec defines, shared between commands.

`YearMonth` is a thin newtype around `(i32 year, u32 month)` with a `Display` that renders `"2026-05"`. No `chrono` dependency added unless one already exists for something else — epoch-to-`(year, month)` is direct math.

Pure helpers (unit-tested individually):

- `classify_by_kind(files) -> [(Kind, usize, u64); 4]`
- `bucket_by_month(files, today) -> (Vec<(YearMonth, usize, u64)>, Option<(usize, u64)>)` — second tuple is the Unknown bucket (count, bytes), or `None` if empty. `today` is passed in so tests are deterministic.
- `top_n_by_size(files, n)` — already exists in analyze; if not `pub`, hoist into a shared `commands::shared::media_math` module, or just reimplement (three lines).
- `find_duplicate_groups(files, top_n) -> DuplicateReport`

### Dispatch + menu integration

`src/lib.rs` gains a `Media { find_duplicates: bool }` subcommand, dispatching to `commands::media::run(&*device, find_duplicates)`.

`src/commands/menu.rs` gains a `Media` entry between `Analyze` and `Info`:

```
Apps · Analyze · Media · Info · Refresh · Reboot · Shutdown · Quit
```

Menu invocation runs `media::run(device, /* find_duplicates */ false)` — keeps the casual menu path cheap. Users who want duplicates run the CLI explicitly.

## Verification needed

1. **AFC `get_file_info` key name** for mtime — assumed `st_mtime`. Verify with a real device read.
2. **Granularity of mtime** — is it seconds or milliseconds in the AFC response? `mtime_unix: Option<i64>` accepts either; pick one in the implementation and document.

## Error handling

- **AFC connect fails**: same trust-and-replug guidance the analyze command uses. Reuse, do not duplicate copy.
- **AFC walk hits permission error on a subdir**: log warning to stderr, skip, continue (same posture as analyze).
- **No files found** (empty walk): print `No files in DCIM, Recordings, Books, Downloads.` and exit `Ok`. Do not print empty section headers in this case — an entirely empty walk has nothing useful to render.

## Testing strategy

### Unit (`#[cfg(test)] mod tests` in `media.rs`)

- `classify_by_kind` with a mix of extensions → exact counts and bytes per bucket, including case-insensitive matching (lowercase + uppercase + missing extension).
- `bucket_by_month` with files spanning 18 months → returns last 12 sorted descending, Unknown bucket excluded when no files lack mtime, included when at least one does.
- `bucket_by_month` with `today = 2026-05-25` and files all from `today` → single bucket `2026-05`, count and bytes correct.
- `top_n_by_size` returns at most N, sorted desc.
- `find_duplicate_groups` with files where 3 share `(size: 100, Photo)` and 2 share `(size: 200, Video)` → 2 groups, 5 files, savings `100 × 2 + 200 × 1 = 400`.
- `render` with a fully-populated report produces byte-for-byte the mock above (snapshot).
- `render` with no `duplicates` (find_duplicates was false) → output does **not** contain the `Likely duplicates` header.
- `render` of an empty walk → produces exactly `No files in DCIM, Recordings, Books, Downloads.\n` and nothing else.

### Integration (`tests/integration.rs`)

- `FakeDevice::default()` with the analyze fixture extended to include mtimes → `media::run(device, false)` returns `Ok`, output contains `By kind`, `By month`, `Largest 10`, does not contain `Likely duplicates`.
- Same fixture + `media::run(device, true)` → output contains `Likely duplicates` and a non-zero group count (the fixture is seeded with at least one obvious duplicate pair).
- A `FakeDevice` with `media: vec![]` → output is the `No files in…` line, exit `Ok`.

### E2E (behind `--features e2e`)

- Smoke: run `media::run(device, true)` against the real device. Assert it returns within ~60s, output contains the four kind labels, the `By month` and `Largest 10` headers, and the duplicates header. Do not assert on specific values.

## Future work

- Confident duplicate detection via header-hash (first/last 64 KB) pulled via AFC random reads. Behind a `--full-hash` flag.
- Content-hash (full file) duplicate detection — slow, behind `--full-hash=content`.
- Live Photo pairing (one row per HEIC+MOV pair).
- `--json` for scripting.
- `qk media --month YYYY-MM` to drill into one month's files (currently `qk analyze` is the way).
- EXIF-based grouping (camera model, dimensions) if a host-side EXIF parser is acceptable as a dependency.
- Disk cache of the last walk, shared with `qk analyze` and a future `qk storage`.
