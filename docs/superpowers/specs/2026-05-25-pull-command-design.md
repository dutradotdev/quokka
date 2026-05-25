# `qk pull` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Summary

A read-from-device command that copies an installed app's **Documents** folder to the host, via the `house_arrest` lockdown service. Scriptable (`qk pull <bundle-id>`) and exploratory (`qk pull` with no args opens a picker of eligible apps).

This is the "Finder → Files" panel of Apple Configurator, but as a CLI: pick an app that has `UIFileSharingEnabled = YES` in its Info.plist, get its Documents subtree onto the Mac, no jailbreak required.

## Motivation

Inspecting an app's local state (a SQLite cache, an exported JSON, a user-generated file) currently requires Finder's "Files" panel — a GUI flow that is slow, not scriptable, and clumsy when iterating. `qk pull` puts the same data behind a one-line shell command, so it composes with `jq`, `sqlite3`, and the rest of the toolchain.

## Scope

### In scope

- Copy an app's `Documents/` subtree from the device to a local path.
- TUI picker for the no-args case, listing only apps where `UIFileSharingEnabled = YES`.
- `--list` flag to print eligible bundle ids (scripting hook).
- `--dest`, `--force` flags for explicit destination and overwrite behavior.
- Path-traversal defense against malicious filenames in the remote subtree.

### Out of scope

- Write-back to the device (the inverse direction). Separate spec when needed.
- App containers outside `Documents/` (Library, tmp, Caches). `house_arrest` exposes `VendDocuments` only for non-developer apps; `VendContainer` requires a developer-signed app and the developer disk image. Out of scope for the MVP per `CLAUDE.md`.
- `qk pull --all` (every eligible app at once). Future work.
- Incremental / mirror mode (`rsync` semantics). v1 is a full copy each time.
- File filtering (`--include`, `--exclude`). v1 pulls everything under Documents.

## CLI surface

### Synopsis

```
quokka pull [<bundle-id>]
            [--dest <path>]
            [--force]
            [--list]
```

- `<bundle-id>` (positional, optional): the app to pull from. When omitted on a TTY, the picker opens. When omitted on a non-TTY, the command aborts with a usage error.
- `--dest <path>` (default `./<bundle-id>/`): local destination directory. Created if missing.
- `--force`: allow writing into a non-empty destination, overwriting files with the same relative path. Without it, a non-empty destination aborts.
- `--list`: print the bundle ids of every eligible app (one per line) and exit. Useful for scripting (`for b in $(qk pull --list); do qk pull "$b" --dest "out/$b/"; done`).

### Behavior

#### Scripted (`qk pull <bundle-id>`)

1. User runs `qk pull com.foo.bar`.
2. Spinner: `Opening Documents on com.foo.bar…`
3. After the house_arrest session is up, an AFC walk lists every file (count + bytes). The destination is preflighted; if it fails, the command aborts before transferring anything.
4. Pull begins. A progress bar shows bytes transferred and file count: `█████░░░░░  47%  39.6 MB / 84.2 MB  ·  67 / 142 files`.
5. On completion: `✓ Pulled 142 files (84.2 MB) to ./com.foo.bar/` (or `✓ Pulled 138 files (82.9 MB), 4 skipped` if some failed — failures listed in stderr).

#### Picker (`qk pull` with no args)

1. User runs `qk pull`. The TUI picker opens with the list of **eligible** apps (those with `supports_file_sharing == Some(true)`). Ineligible apps are excluded — the picker is curated.
2. Single-select. `enter` chooses; `q` quits.
3. After selection, dest is `./<bundle-id>/` and the same scripted flow runs.

The picker is not a place to customize destination — dest-tweaking is CLI-only via `--dest`.

#### Destination preflight

The default `./<bundle-id>/` is deliberate: safer than dumping `Documents/` into cwd, and self-explanatory in shell history.

Preflight runs before any AFC read:

| Dest state | Without `--force` | With `--force` |
|---|---|---|
| Does not exist | Created, OK | Created, OK |
| Empty directory | OK | OK |
| Non-empty directory | Abort with error | OK — files with matching relative paths overwritten; existing files outside the pulled set are left untouched |
| Existing regular file | Abort with error | Abort with error (refuse to delete a file to make a dir) |

The "non-empty + `--force`" semantics are deliberately additive (not "wipe-then-write"): pulling the same app twice yields a coherent on-disk Documents subtree without risk of nuking unrelated files the user put there.

## Architecture

### Device trait changes

```rust
#[async_trait]
pub trait Device: Send + Sync {
    // ... existing methods ...

    /// Walk the Documents folder of one installed app. Returns one entry per
    /// file (no directory entries). Empty when Documents is empty. Errors
    /// when the app is not installed or does not support file sharing.
    async fn house_arrest_walk(&self, bundle_id: &str) -> Result<Vec<MediaFile>>;

    /// Read one file from the Documents folder of one installed app. Path is
    /// the AFC-relative path returned by `house_arrest_walk`.
    async fn house_arrest_read(&self, bundle_id: &str, path: &str) -> Result<Vec<u8>>;
}
```

Two methods, not one bulk "pull entire app" method: the command layer needs the file list before transfer (for preflight, total-bytes count, and progress) and per-file reads (for incremental progress and per-file error handling). The seam rule holds — `MediaFile` is the existing struct from the analyze spec; no `idevice` types leak.

`Vec<u8>` for the file body is the v1 trade-off. Most Documents files are small. For the rare multi-hundred-MB case (cache DBs from heavy apps), v1 still works — the read buffers in memory once before being written to disk. Streaming to a writer is future-work.

Extension to `AppEntry` (whatever struct `device.apps()` returns):

```rust
pub struct AppEntry {
    // ... existing fields ...
    pub supports_file_sharing: Option<bool>,   // None = unknown; True = UIFileSharingEnabled
}
```

`None` is preserved as a distinct state, so a future iOS quirk where the key cannot be read does not silently mark every app as ineligible. The picker and `--list` treat `Some(true)` as eligible; everything else is hidden.

### Real implementation notes

- For each house_arrest method, open `HouseArrestClient::connect(provider, bundle_id, "VendDocuments").await`. Per-call session open is the v1 posture (matches `afc_walk` and `afc_delete`); pooling lands when benchmarks justify it.
- Walk uses the same algorithm as the existing AFC walk in the analyze spec, run against the house-arrest-scoped AFC client instead of the global one.
- Read uses `AfcClient::open / read / close` on the scoped client.
- `supports_file_sharing` is read from the app's Info.plist via the same `installation_proxy` browse call that already enriches `AppEntry` — one extra key in the requested fields, not a new round-trip.
- Two failure modes mapped to user-friendly errors at the trait boundary:
  - House_arrest refuses the bundle id (not installed): `App not installed: <bundle-id>`.
  - House_arrest refuses `VendDocuments` (`UIFileSharingEnabled=false`): `App <bundle-id> does not support file sharing. Run \`qk pull --list\` to see eligible apps.`

### FakeDevice additions

Two additions:

- `pub house_arrest: HashMap<String, Vec<(String, Vec<u8>)>>` — bundle id → list of `(path, content)` pairs. `house_arrest_walk(b)` returns the keys; `house_arrest_read(b, p)` returns the bytes. Missing bundle id → the same error variant the real path emits.
- `AppEntry` fixtures gain a `supports_file_sharing` value (default `Some(true)` for the standard fixture; tests that need ineligible apps construct them explicitly).

### Command module structure

`src/commands/pull.rs`:

```rust
pub struct Options {
    pub bundle_id: Option<String>,
    pub dest: Option<PathBuf>,
    pub force: bool,
    pub list: bool,
}

pub async fn run(device: &dyn Device, opts: Options) -> Result<()> { ... }
```

`run` flow:

1. If `opts.list` → call `device.apps()`, print every bundle id where `supports_file_sharing == Some(true)`, one per line, return `Ok`.
2. Resolve the bundle id: `opts.bundle_id` if set, otherwise `picker::run(device).await?` on a TTY, otherwise return the non-interactive usage error.
3. Resolve dest: `opts.dest.unwrap_or_else(|| PathBuf::from(format!("./{bundle_id}/")))`.
4. Preflight dest (see table above). Abort on conflict.
5. `device.house_arrest_walk(&bundle_id).await?` → file list. If empty, print `Documents is empty for <bundle-id>.` and return `Ok`.
6. Set up a progress bar (one bar, total = sum of sizes). For each file:
   - `device.house_arrest_read(&bundle_id, &file.path).await`
   - On error: emit `✗ <path>: <reason>` to stderr, bump the failure counter, continue.
   - On success: ensure parent dir exists, write bytes to `dest.join(strip_leading_slash(&file.path))`, bump progress by `file.size_bytes`.
7. Print the final summary line: success count + bytes + skipped count if any.

#### Picker (`pull::picker` private submodule)

Mirrors `apps::tui`'s skeleton — same header / column-header / list / footer layout — but **single-select** and **read-only-filtered** (only eligible apps appear). Columns: `name · bundle id · documents size (best-effort)`. Documents size is `device.house_arrest_walk` summed; if the walk fails for an app, the row shows `—` and stays selectable (the actual pull will surface the same error).

Sizes load lazily as the user scrolls — same Phase-2-enrichment posture `apps` already uses. Otherwise first paint would trigger N walks against the device sequentially.

Footer: `↑↓ nav · enter pull · q quit`. No multi-select, no `/` search in v1.

#### Pure helpers (unit-tested)

- `default_dest_for(bundle_id: &str) -> PathBuf` — returns `./<bundle-id>/`.
- `preflight_dest(dest: &Path, force: bool) -> Result<DestState>` — returns one of `Created`, `Empty`, `OverwriteAllowed`, or `Err`.
- `local_path_for(dest: &Path, remote: &str) -> Result<PathBuf, PathTraversal>` — joins `dest` with the remote path's components after stripping the leading slash. Rejects remote paths containing `..` segments as a defense against path traversal.

### Dispatch + menu integration

`src/lib.rs` gains a `Pull(PullArgs)` subcommand with the flags above. Dispatches to `commands::pull::run(&*device, opts)`.

`src/commands/menu.rs` gains a `Pull` entry between `Logs` and `Info`:

```
Apps · Analyze · Media · Logs · Pull · Info · Refresh · Reboot · Shutdown · Quit
```

Menu invocation runs `pull::run(device, Options::default())` — picker opens, default dest, no force. After the pull completes, the menu redraws as usual.

## Verification needed

1. **`HouseArrestClient::connect` signature** on the `idevice` crate (positional vs named args, exact constant name for `VendDocuments`) — confirm against `tools/src/house_arrest.rs`.
2. **`UIFileSharingEnabled` key name** as returned by `installation_proxy` browse — confirm against a sample fetch on a known eligible app.
3. **Behavior when `house_arrest` is asked for an ineligible app** — does it reject at connect-time or at first AFC call? Maps to which error message the user sees.

## Error handling

- **Lockdown / house_arrest connect fails**: trust-and-replug guidance. Same helper as other commands.
- **App not installed**: `App not installed: <bundle-id>`. Exit non-zero.
- **App does not support file sharing**: same shape with the `--list` hint. Exit non-zero.
- **Destination conflict** (non-empty without `--force`, or existing regular file at the dest path): human copy describing the state and the flag that would resolve it. Exit non-zero.
- **Path traversal attempt** (remote path with `..` segments): refuse the file, emit `✗ <path>: refused (path traversal)` to stderr, continue with the rest. Counts toward the failure total in the summary.
- **Per-file read or write error**: stderr `✗ <path>: <reason>`, continue. Final summary distinguishes success vs failure counts. Non-zero exit if any failures.
- **Disk full mid-pull**: errors propagate per-file as write failures. No cleanup of partial output — user can re-run with `--force` to resume after freeing space.

## Testing strategy

### Unit (`src/commands/pull.rs`)

- `default_dest_for("com.foo.bar")` → `PathBuf::from("./com.foo.bar/")`.
- `preflight_dest` table cases:
  - non-existent path + force=false → `Created`.
  - empty dir + force=false → `Empty`.
  - non-empty dir + force=false → `Err`.
  - non-empty dir + force=true → `OverwriteAllowed`.
  - existing regular file (force either) → `Err`.
- `local_path_for(dest=/tmp/out, remote="/foo/bar.txt")` → `/tmp/out/foo/bar.txt`.
- `local_path_for` with `remote="/foo/../etc/passwd"` → `Err(PathTraversal)`.
- Eligibility filter for `--list` and the picker: from a fixture of mixed `supports_file_sharing` values, only `Some(true)` entries pass.

### Integration (`tests/integration.rs`)

- `FakeDevice` seeded with `com.foo.bar` having 3 files in Documents → `pull::run(device, Options { bundle_id: Some("com.foo.bar".into()), dest: Some(tmp.path().into()), force: false, list: false })` writes all 3 files to `tmp`, returns `Ok`, summary contains "3 files".
- Same fixture + dest is a pre-populated non-empty dir + `force: false` → returns `Err`, no files written.
- Same fixture + dest is a pre-populated non-empty dir + `force: true` → all 3 files written, pre-existing unrelated files in dest remain.
- `bundle_id` of an ineligible app (`supports_file_sharing: Some(false)` in fixture) → returns `Err` with the file-sharing error message.
- `bundle_id` of a non-existent app → returns `Err` with the "App not installed" message.
- `--list` with a mixed-eligibility fixture → stdout contains exactly the eligible bundle ids, one per line, no other text.
- A fixture where one file's remote path contains `..` → that file is refused (stderr line), the other files succeed, final exit is non-zero.

The picker TUI is **not** driven in tests (same trade-off as `apps::tui` and `analyze::tui`).

### E2E (`tests/e2e_*.rs`, behind `--features e2e`)

- Smoke: list eligible apps via `device.apps()` filtered to `supports_file_sharing == Some(true)`. If none exist on the connected device, the test prints a warning and passes (skipping is the right posture — the test cannot guarantee a file-sharing app is installed on every dev's phone).
- If at least one exists: pick the first, run `pull::run` to a `tempfile::tempdir`, assert at least one file landed and the summary parses cleanly.

## Future work

- Streaming reads (chunk-based) to avoid buffering large files in memory.
- `qk pull --all` to pull every eligible app, each into its own subdirectory.
- Incremental / mirror mode (skip files where local size + mtime match remote).
- Filter flags (`--include`, `--exclude` with glob support).
- `qk push <bundle-id> <local-path>` — write-back. Symmetric, needs a write-side preflight design pass.
- Resume / checkpoint for interrupted pulls.
- Per-file integrity check (size match after write).
