# `qk logs` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Summary

A live, developer-friendly log viewer for the connected iPhone. Streams the device's syslog through `com.apple.syslog_relay`, parses every line into a structured `LogEntry`, and renders it in a ratatui TUI with color-by-level, level filtering, process filtering, substring search, pause/resume, and save-to-file.

The CLI also has a `--no-tui` mode that streams plain text to stdout — output composes with `grep` / `tee` / `jq` without needing the TUI at all.

## Motivation

Triaging an iPhone-side bug today means SSH-ing into the device (jailbreak), spinning up macOS Console.app (which has its own learning curve and depends on Apple's private framework), or shipping log lines to a remote service. None of those fit the case "I have the device on USB and I want to see what it's doing right now." `qk logs` is the terminal-native option for that workflow.

## Scope

### In scope

- TUI log viewer with color-by-level, pause/resume, level filter, process filter, substring search, save-to-file.
- `--no-tui` plain-stream mode for shell pipelines.
- Structured `LogEntry` parsing from the legacy BSD-syslog-ish format iOS emits today.

### Out of scope

- `os_log` archive parsing (`com.apple.os_trace_relay` / `.logarchive` files). Different service, different format, different scope. Future work.
- Correlation across reboots. The stream begins when the command starts; prior log lines are not reachable through `syslog_relay`.
- Remote forwarding (syslog server export, OTLP, etc.). Save-to-file covers the "I need this somewhere else" use case.
- Filter persistence across runs. Each invocation starts with the default filter. Future work.
- Process-tree view, aggregation, or any analysis. This is a stream viewer, not an analyzer.

## CLI surface

### Synopsis

```
quokka logs [--no-tui]
            [--min-level <level>]
            [--process <substring>]
            [--save <path>]
```

- `--no-tui`: plain stream, no TUI.
- `--min-level <level>`: pre-applied level filter. One of `debug | info | notice | warning | error | fault`. Default is `notice` (matches the TUI default; hides spammy debug/info).
- `--process <substring>`: pre-applied case-insensitive substring match against the process name.
- `--save <path>`: write every line (after filters apply) to `<path>` in addition to its normal sink (TUI buffer or stdout). Appends if file exists.

All flags work in both modes.

### Behavior

#### TUI mode (default)

1. User runs `qk logs`. The TUI opens immediately. While the syslog session is being negotiated, the body shows `Waiting for device logs…`.
2. Lines begin streaming. The viewport auto-scrolls to the bottom by default. Each line is colored by its `LogLevel`.
3. Top status bar shows: device name, total lines received, current filter summary, current search query, mode (Live / Paused).
4. Bottom keybind bar shows the active keys.
5. User filters / searches / saves interactively. `q` or Ctrl-C exits cleanly.

Target minimum: 80 columns × 24 rows. Below 60 columns wide, the TUI refuses to start with an error pointing the user at `--no-tui`.

```
┌─ qk logs · Lucas's iPhone · 12,847 lines · 23 matched · ▶ Live ───┐
│ Filter: level≥warning · process=SpringBoard          Search: "foo"│
├───────────────────────────────────────────────────────────────────┤
│ 14:32:17.123  SpringBoard[63]    <Warning>  Bluetooth: reconnect…│
│ 14:32:17.456  mediaserverd[91]   <Error>    Codec init failed: -5│
│ 14:32:18.001  WirelessProx[112]  <Notice>   New scan results (4) │
│ ...                                                                │
│                                                                    │
├───────────────────────────────────────────────────────────────────┤
│ ↑↓ scroll · space pause · l level · p process · / search · n next │
│ N prev · w save · c clear · q quit                                 │
└───────────────────────────────────────────────────────────────────┘
```

Each row: `HH:MM:SS.mmm  process[pid]  <Level>  message`, with the whole row tinted by level. Timestamp uses device local time if it parses cleanly; otherwise raw text from the line. Multi-line messages (continuation lines without a leading timestamp) are joined with `↵ ` into one row, so scrolling stays predictable.

##### Keybinds

| Key | Action |
|---|---|
| `↑` `↓` | Scroll one line. Page-size jump on `PgUp` / `PgDn`. |
| `g` / `G` | Jump to top / bottom of buffer. `G` also re-engages auto-scroll. |
| `space` | Toggle Live / Paused. Paused freezes the viewport; new lines accumulate off-screen and the header shows `Paused · N new`. |
| `l` | Cycle minimum level: `debug → info → notice → warning → error → fault → debug`. Visible in the filter line. |
| `p` | Enter process filter. Bottom bar becomes a text input; Enter applies, Esc cancels. Empty value clears the filter. Case-insensitive substring. |
| `/` | Enter search query. Highlight matches inside visible rows; count them in the header (`23 matched`). |
| `n` / `N` | Jump to next / previous match (only meaningful when a search is active). |
| `w` | Save the current filtered buffer (visible lines, in scroll order) to `qk-logs-YYYYMMDD-HHMMSS.log` in cwd. Shows the saved path in the status bar for a few seconds. |
| `c` | Clear the buffer. New lines continue arriving. |
| `q` / `Ctrl-C` | Exit cleanly. |

Filter is the persistent state; search is the transient overlay. Both apply on top of each other.

#### Plain mode (`--no-tui`)

1. User runs `qk logs --no-tui`. The command writes one parsed line per output line to stdout, in a stable format, until Ctrl-C.
2. No color when stdout is not a TTY (the existing `anstream` integration handles this — `--no-tui` does not change the color decision).
3. Errors go to stderr. Exit code `0` on clean Ctrl-C, non-zero on stream error.

One line per entry, deterministic, easy to grep:

```
2026-05-25T14:32:17.123Z  SpringBoard[63]  <Warning>  Bluetooth: reconnecting…
```

ISO-8601 UTC timestamp, two-space separators between fields.

#### Color mapping

```
Fault     bright_red
Error     red
Warning   yellow
Notice    default (no color)
Info      bright_black (dim)
Debug     bright_black (dim)
Unknown   default
```

Same `owo_colors` palette quokka already uses. The whole row is tinted (not just the level token).

## Architecture

### Device trait changes

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Fault,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp_unix_ms: Option<i64>,
    pub host: String,                    // e.g. "Lucass-iPhone"
    pub process: String,                 // e.g. "SpringBoard" or "App(WebKit)"
    pub pid: Option<u32>,
    pub level: LogLevel,
    pub message: String,                 // continuation lines already joined with "\n"
}

#[async_trait]
pub trait Device: Send + Sync {
    // ... existing methods ...

    /// Open a streaming syslog session. Returns an `mpsc::Receiver` that
    /// yields one `LogEntry` per parsed line. The session ends when the
    /// receiver is dropped, when the device disconnects, or when an
    /// unrecoverable framing error occurs (which arrives as the final
    /// `Err(...)` and then the channel closes).
    async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>>;
}
```

`tokio::sync::mpsc::Receiver<Result<LogEntry>>` is the contract. The seam rule holds — no `idevice` types leak. Channel buffer size is an implementation choice (`mod real` picks 1024).

### Real implementation notes

- Open `SyslogRelayClient::connect(provider).await`.
- Spawn a tokio task that loops: read one syslog frame, parse to `LogEntry`, send via the channel. On unrecoverable error, send the error and close.
- Parsing is a pure function `parse_syslog_line(raw: &str) -> Result<LogEntry, ParseError>` living in `src/device/syslog_parser.rs`, **unit-tested independently of the real client**. It accepts the legacy BSD-syslog-ish format iOS sends (`Mmm DD HH:MM:SS host process[pid] <Level>: message`), plus the continuation form (a line starting with whitespace is appended to the previous entry's message).

### FakeDevice additions

Add `pub seeded_logs: Vec<Result<LogEntry, String>>` and a tunable `log_tick: Duration` (default `Duration::from_millis(10)`). `stream_logs` spawns a task that emits each entry with `log_tick` spacing, then closes the channel. Tests seed a small mixed-level fixture and assert what the parser/filter chain produces.

### Command module structure

`src/commands/logs.rs`:

```rust
pub struct Options {
    pub no_tui: bool,
    pub min_level: LogLevel,
    pub process_filter: Option<String>,
    pub save_path: Option<PathBuf>,
}

pub async fn run(device: &dyn Device, opts: Options) -> Result<()> { ... }
```

`run` opens the stream, then dispatches:

```rust
if opts.no_tui || !stdout_is_tty() {
    plain::run(rx, opts.save_path, ...).await
} else {
    tui::run(rx, opts).await
}
```

A non-TTY in plain mode is fine. A non-TTY in TUI mode (no `--no-tui` passed) silently falls through to plain mode — never crash because the terminal is missing.

#### `mod plain`

One async loop: receive an entry, apply the filter chain, format it, write to stdout and (optionally) to the save file. On the channel closing, exit `Ok`. On a parse error from the channel, write `! parse error: …` to stderr and continue. Ctrl-C handled by tokio's signal handler — drains the channel, closes the save file, exits zero.

#### `mod tui`

Owns:

- `Buffer` — `VecDeque<LogEntry>` with cap `LOG_BUFFER_CAP = 10_000`. Push back, pop front on overflow.
- `Filter { min_level: LogLevel, process: Option<String> }` — current persistent filter.
- `Search { query: Option<String> }` — current transient search.
- `View { offset: usize, paused: bool, pending_while_paused: usize }`.
- An mpsc receiver from the device.

Event loop:

```
loop {
    select! {
        entry = rx.recv() => buffer.push(entry); maybe redraw;
        key   = events.next() => handle(key); maybe redraw;
        _     = tick (200 ms) => redraw if dirty;
    }
}
```

Filter and search are applied lazily during render — the buffer is never duplicated. Render iterates the buffer back-to-front, takes the visible window, applies `Filter`, formats, and (when a search is active) wraps matched substrings in a highlight span.

The Search-match counter in the header is computed by scanning the buffer once per redraw. With 10k lines and a needle of a few characters, this is sub-millisecond — acceptable for the redraw cadence.

#### Pure helpers (unit-testable)

- `parse_syslog_line(raw: &str) -> Result<LogEntry, ParseError>`
- `matches_filter(entry: &LogEntry, filter: &Filter) -> bool`
- `format_plain(entry: &LogEntry, w: &mut impl Write) -> io::Result<()>`
- `format_tui_row(entry: &LogEntry, search: Option<&str>) -> Line<'static>` — returns a ratatui `Line` with color spans, optionally with search highlight spans inserted.

No `if`-ladder for level → color. A `const`:

```rust
const LEVEL_STYLE: [(LogLevel, Style); 7] = [ ... ];
fn style_for_level(level: LogLevel) -> Style { ... }
```

### Dispatch + menu integration

`src/lib.rs` gains a `Logs(LogsArgs)` subcommand with the flags above. Dispatches to `commands::logs::run(&*device, opts)`.

`src/commands/menu.rs` gains a `Logs` entry between `Media` and `Info`:

```
Apps · Analyze · Media · Logs · Info · Refresh · Reboot · Shutdown · Quit
```

Menu invocation runs `logs::run(device, Options::default())` — TUI mode, default filter, no save path. On TUI exit, the menu redraws as usual.

## Verification needed

1. **Upstream `idevice` API surface** for syslog frames (`SyslogRelayClient`) — confirm against `tools/src/syslog.rs`.
2. **Exact format of iOS syslog lines** on current iOS (17/18) — confirm the BSD-syslog-ish shape assumed by the parser. Capture a sample dump during the first probe and use it as a parser fixture.

## Error handling

- **Lockdown / syslog_relay connect fails**: trust-and-replug guidance, abort.
- **Device disconnects mid-stream**: TUI shows a banner on row 2 (`✗ Stream ended: device disconnected.`) and waits for the user to press `q`. Plain mode prints the error to stderr and exits non-zero.
- **A single line fails to parse**: it becomes a `LogEntry` with `level: Unknown`, `process: "?"`, and the raw text as `message`. No data loss, no warning — `Unknown` is the failure signal.
- **Save file fails to open or write**: TUI shows `Save failed: <reason>` in the status bar for a few seconds and silently disables further saves. Plain mode prints to stderr and exits non-zero on the write error (because in plain mode, save is often the primary purpose).
- **Buffer overflow** (more than `LOG_BUFFER_CAP`): silently drops the oldest entry. The header counter (`12,847 lines`) reflects all-time received, not buffer size, so the user can tell when drops are happening.

## Testing strategy

### Unit (`src/device/syslog_parser.rs` and `src/commands/logs.rs`)

- `parse_syslog_line` table-driven test with:
  - Each level (`<Fault>`, `<Error>`, `<Warning>`, `<Notice>`, `<Info>`, `<Debug>`).
  - Missing pid (`process` without `[pid]`).
  - Process with module suffix (`App(WebKit)`).
  - Continuation lines join correctly into the previous entry.
  - Garbage line → `LogEntry { level: Unknown, message: <raw>, process: "?" }`.
- `matches_filter`:
  - `min_level: Warning` rejects `Info` / `Notice` / `Debug`, accepts `Warning` / `Error` / `Fault`.
  - `process: Some("springboard")` matches `SpringBoard` and `springboardhelper` (substring, case-insensitive).
  - Combination — both must match.
- `format_plain` snapshot — ISO-8601, two-space separators, byte-for-byte.
- `format_tui_row` — given a search of `"foo"` and a message containing `"foo"`, produces a `Line` with at least one styled span whose text is `"foo"`.

### Integration (`tests/integration.rs`)

- `FakeDevice` seeded with `~20` mixed-level entries → `logs::run(device, Options { no_tui: true, min_level: LogLevel::Notice, .. })` produces stdout matching the expected filtered set (Debug + Info dropped, rest kept, order preserved).
- Same fixture, `min_level: LogLevel::Warning`, `process_filter: Some("springboard".into())` → output contains only entries matching both predicates.
- `save_path: Some(tmp.path().join("out.log"))` in plain mode → file exists, contains the same content as stdout (modulo no color).
- TUI mode is **not** driven in tests (consistent with `apps::tui` and `analyze::tui`).

### E2E (`tests/e2e_*.rs`, behind `--features e2e`)

- Smoke: connect, open `stream_logs`, await 5 seconds, assert at least one parsed `LogEntry` with a known level (i.e. at least one line parsed cleanly into something other than `Unknown`). Tear down.

## Future work

- `os_log` archive support via `com.apple.os_trace_relay` (rich subsystem/category metadata, signposts, formatted messages).
- Filter persistence to `~/.config/quokka/logs.toml` (last min-level, last process filter).
- Saved "filter presets" (named filters for common workflows).
- Regex search instead of substring. Held back because regex on every redraw at scale needs a compiled-cache layer.
- "Hide before timestamp T" / "Hide after timestamp T" for time-range filtering.
- Per-process color tagging (each process gets a stable derived color).
- Highlight rules (color any line matching a user-supplied substring, regardless of level).
- TTL on the on-screen save indicator.
