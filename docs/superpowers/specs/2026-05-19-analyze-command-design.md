# `quokka analyze` — design

**Status:** approved, ready for implementation plan
**Date:** 2026-05-19
**Owner:** Lucas Dutra

## Goal

Help the user free space on the iPhone by surfacing the heaviest files in the AFC-accessible media area, and (when explicitly opted-in) letting them delete a selected subset from a terminal UI.

This replaces the current stub (`src/commands/analyze.rs` → `bail!`).

## Non-goals

- Per-app cache cleanup. AFC does not reach app containers; that path is out of scope for the MVP per `CLAUDE.md`.
- Walking `PhotoData/`, `iTunes_Control/`, or other internal directories. Deleting from them corrupts the Photos library or sync state.
- Aggregated views (heaviest folders, heaviest types, charts). YAGNI for v1.
- Deleting via explicit path flag (e.g. `quokka analyze --delete /DCIM/100APPLE/IMG.MOV`). Future, if requested.

## User flow

1. User runs `quokka analyze` (or selects it from the launcher menu).
2. Spinner shows progress while AFC is walked: `Scanning… 1,234 files, 12.4 GB`.
3. Walk completes. The top `--top` (default 20) files by size are presented:
   - **Without `--delete`** (default, "dry-run"): a static text table is printed and the program exits. This is the read-only mode.
   - **With `--delete` on a TTY**: a ratatui multi-select picker opens, mirroring the `apps` picker. User toggles files, hits `enter`, confirms in a dialoguer prompt, and the selected files are deleted via AFC.
   - **With `--delete` on a non-TTY**: aborts with an error explaining that destructive runs need a TTY. (Same posture as `apps --uninstall` without `--yes`.)
4. After deletion, the picker exits and the program prints a one-line summary (`Deleted N files (X GB).`).

The launcher menu (`src/commands/menu.rs`) calls `analyze::run(device, 20, false)`, so the menu entry is always read-only. To delete, the user must invoke `quokka analyze --delete` from the shell.

## Scope of the walk

Curated, hardcoded list of AFC roots (paths relative to `/var/mobile/Media/`):

- `DCIM/` — camera roll (photos, videos, Live Photo `.MOV` companions). The big one.
- `Downloads/` — files saved via Safari / Files.app.
- `Recordings/` — Voice Memos.
- `Books/` — sideloaded ePubs and PDFs.

Each root is walked recursively. Files are collected with `{path, size_bytes}`. Directories themselves are not listed.

Anything outside these roots is invisible to `analyze`. This is the safety guardrail: a user picking everything in the list and confirming cannot break Photos.app or sync.

## CLI

Unchanged from today's clap definition in `lib.rs`:

```
quokka analyze [--top N] [--delete]
```

- `--top N` (default `20`): rows shown in the table / picker. Walk always covers everything; this only controls display.
- `--delete` (default off): gates write mode. Without it, the picker is not even opened — output is printed plain text and the program exits.

## Output

### Read-only (no `--delete`)

A static, formatted table (rendered via `ui::format_bytes`, consistent with `apps`):

```
       size  kind   path
   4.21 GB  Video  /DCIM/103APPLE/IMG_4521.MOV
   2.88 GB  Video  /DCIM/103APPLE/IMG_4519.MOV
   ...
   20 files shown · 18.7 GB · 12,344 total files scanned · 84.2 GB
```

`kind` is derived from extension (see "Classification" below).

### Interactive picker (with `--delete` on TTY)

Same skeleton as `apps::tui` — header status line, column header, list with checkbox + highlight, footer keybinds. Differences:

- Columns: `[checkbox]  size  kind  path`.
- No "Phase 2" enrichment — file sizes come straight from the AFC walk; the list is ready in one shot.
- Footer: `↑↓ nav · space toggle · enter delete · / search · q quit`. Search filters by substring of path.
- No `s` (sort) key. The list is always sorted by size descending.

On `enter`:

1. `dialoguer::Confirm` prompt:
   > Delete N files (X GB)? They will be removed from the device permanently.
2. If any selected path starts with `/DCIM/`, append:
   > Note: Photos.app may still show thumbnails until the next library refresh.
3. On confirm, delete each via AFC, print success/failure line per file (same `✓`/`✗` style as `apps::run_uninstall`).
4. Exit the picker.

## Architecture

### New types and trait methods (`src/device.rs`)

```rust
pub struct MediaFile {
    pub path: String,       // absolute path under AFC root, e.g. "/DCIM/100APPLE/IMG.MOV"
    pub size_bytes: u64,
}

pub struct WalkProgress {
    pub files_seen: usize,
    pub bytes_seen: u64,
}

pub type WalkCallback = Box<dyn Fn(WalkProgress) + Send + Sync>;

#[async_trait]
pub trait Device: Send + Sync {
    // ... existing methods ...

    /// Recursively list every file under each of `roots` via AFC. `roots`
    /// are AFC-relative paths (e.g. "/DCIM"). `on_progress` fires
    /// periodically with cumulative counts so the UI can show live updates.
    async fn afc_walk(&self, roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>>;

    /// Delete a single file via AFC. Errors are mapped to user-readable
    /// messages (file not found, permission denied, etc.).
    async fn afc_delete(&self, path: &str) -> Result<()>;
}
```

No `idevice` type leaks across the trait — consistent with the seam rule in `CLAUDE.md`.

### `mod real` implementation

- Open `AfcClient::connect(provider).await` once per call. (Pooling is a future optimization, only if benchmarks show it matters.)
- For `afc_walk`: maintain a manual queue of directory paths. Pop one, `read_directory(path)`, for each entry call `get_file_info(entry)`:
  - If directory: push onto queue.
  - If file: record `{path, size}` into the result vec; bump cumulative counters; fire `on_progress` every ~100 files or every ~200 ms (whichever first) so we don't spam the channel.
- For `afc_delete`: call `AfcClient::remove_path(path)` (verify exact upstream method name during implementation; the upstream `tools/src/afc.rs` is the reference).

### `FakeDevice`

Add `pub media: Vec<MediaFile>` to seed walk results. Default to a small plausible set (a 2 GB video, a 800 MB video, a 50 MB PDF, etc.) so integration tests have something to render.

`afc_walk` ignores `roots` and returns `self.media.clone()`, firing one synthetic progress event with the full count — same pattern `with_dynamic_sizes` uses today.

`afc_delete` pushes the deleted path onto a `Mutex<Vec<String>>` so tests can assert deletions happened.

### `src/commands/analyze.rs`

Structure mirrors `apps.rs`:

```
pub async fn run(device, top: usize, delete: bool) -> Result<()>
  → walk()             // spinner + AFC walk
  → top_n_by_size()    // sort, truncate
  → if delete && tty:
      tui::run(...)
    else:
      print render_file_list(...)
```

- `walk()` opens `indicatif` spinner; closure passed as `on_progress` updates spinner message with `format_bytes`.
- `render_file_list(...)` is the read-only table, pure function — unit-testable.
- `mod tui` defines `Outcome`, `State`, `run`, `draw`, `handle_event`, `row` — same module shape as `apps::tui`. Search/nav/toggle code follows the same patterns; copy-paste-and-prune is fine, no premature abstraction.

`Options` struct (parallel to `apps::Options`) carries `top` and `delete` from `lib.rs` into `run`. Not strictly required for two flags but matches the convention.

### Classification (`kind`)

Pure function `kind_from_ext(path: &str) -> &'static str`:

| Extension(s) | `kind`  |
|---|---|
| `mov`, `mp4`, `m4v`, `hevc` | `Video` |
| `jpg`, `jpeg`, `heic`, `png`, `gif` | `Photo` |
| `m4a`, `mp3`, `aac`, `wav` | `Audio` |
| `pdf`, `epub` | `Doc` |
| anything else / no extension | `Other` |

Case-insensitive. Unit-tested.

## Error handling

- **AFC connect fails**: error message points the user to the same trust-and-replug guidance the `apps` flow uses.
- **AFC walk hits a permission error on one subdir**: log a warning to stderr, skip the subdir, continue. Do not abort the whole walk.
- **AFC delete fails mid-batch**: print `✗ path: <reason>` and continue with the next file. Summary at the end counts successes and failures separately.
- **No files found**: print `No files in DCIM, Downloads, Recordings, Books.` and exit `Ok`. Do not open the picker.

## Testing

### Unit (`#[cfg(test)] mod tests` in `analyze.rs`)

- `kind_from_ext` cases (each kind plus unknown, plus uppercase).
- `top_n_by_size` truncates and sorts descending.
- `render_file_list` produces the expected header + rows + summary footer; handles the empty case.
- Confirm prompt copy (when present) includes the DCIM warning iff any selected path is under DCIM.

### Integration (`tests/integration.rs`, or a new file)

- `FakeDevice` seeded with a mixed `media` list → `run(device, 5, false)` on the non-TTY path prints exactly the top 5, sorted, with the correct grand total.
- `run(device, 5, true)` on a non-TTY environment errors out (mirrors `apps --uninstall` non-interactive behavior).
- `FakeDevice::afc_delete` records the deletion for assertions (TUI itself is not driven in tests — the picker is too stateful to fake without a full crossterm harness, same trade-off `apps::tui` makes).

### E2E (`tests/e2e_*.rs`, behind `--features e2e`)

- Smoke test: connect, walk the four roots, assert the call returns within a generous time bound (e.g. 60s) and produces at least one file. Never deletes.

## Future work (explicitly deferred)

- Delete-by-path flag (`--delete <path>...`) for scripting.
- Aggregated view (heaviest folders / heaviest types).
- Cache of last walk on disk to skip re-scanning, stale-while-revalidate UX (parallel to the optimization sketched for `apps`).
- Live streaming into the picker as the walk progresses (currently the spinner blocks until the walk completes).
