# `qk capture` — Interactive TUI (Phase 6)

Date: 2026-05-26
Status: Draft, pending implementation
Author: Lucas (with Claude)
Scope: `qk capture` stream and hosts modes only. `--dns` and `--sni` modes remain unchanged.

## Context

Phases 1–5 of `qk capture` ship a working CLI tool that prints packets
line-by-line to stdout, with file output (`--save`), filters (`--app`,
`--proto`, etc.), and four output modes. The original spec deferred
"visualização em TUI rica (ratatui) — fica pra v2 se houver demanda"
to a follow-up. This document is that follow-up.

Lucas is the only user today. The trigger is friction with live
captures: filters are static (CLI-flag only, requires restart), the
plain-text stream is hard to read at speed, and `--hosts` uses ANSI
clear-screen which flickers and can't be navigated.

## Goals

1. Inline filter editing while a capture is live (no restart).
2. Scrollable history of recent packets with a details pane for the
   selected row.
3. Stream and Hosts as Tab-switchable views inside a single TUI session,
   sharing one filter and one aggregator.
4. Visual hierarchy via color (direction, protocol, drop warnings).

## Non-goals

- Replacing or wrapping `--dns` and `--sni`. Those keep their current
  line-based output.
- A `--no-tui` escape hatch for stream mode. Pipe consumers and scripts
  must use other tools (or invoke the pre-Phase-6 binary). This is a
  deliberate behavior break — see "Breaking changes" below.
- A Wireshark-style display filter that's separate from the capture
  filter. The TUI uses a single filter that resets the world (see
  "Filter semantics").
- Mode switching to `--dns` or `--sni` inside the TUI. Those remain
  separate invocations.
- TUI for `--save` output format selection. `--save` continues to take
  a path argument; format follows extension (Phase 3).
- Theme customization, custom keybindings, mouse support.

## Breaking changes

`qk capture` without a TTY (pipes, redirects, CI) used to work and now
errors out. The error message points to `qk logs` for the
non-interactive streaming use case, since `qk logs` already supports
`--no-tui` for the same pattern.

Pre-existing flags (`--max`, `--save`, `--app`, `--pid`, `--port`,
`--proto`, `--interface`, `--hosts`, `--dns`, `--sni`) continue to work
unchanged. The TUI pre-populates its filter state from `--app`,
`--pid`, `--port`, `--proto`, `--interface` so passing them on the
command line still works.

## Architecture

`src/commands/capture.rs` is reorganized into a directory module to keep
each file focused:

```
src/commands/capture/
  mod.rs       entry point: run(), Options, Filter, re-exports
  parser.rs    parse_summary, parse_dns_query, extract_sni
  pcap_io.rs   CaptureFile, SaveFormat
  tui.rs       App, View, draw loop, event handler (new)
  hosts.rs     HostAggregator
```

All public items from today's `capture.rs` continue to be reachable
via `crate::commands::capture::*` through re-exports in `mod.rs`, so
existing tests don't need import changes.

No new external dependencies. `ratatui` and `crossterm` are already in
`Cargo.toml` (used by `qk apps` and `qk analyze --delete`).

## Components

### `App` (state)

```rust
pub struct App {
    view: View,                          // Stream | Hosts
    rows: VecDeque<DisplayRow>,          // ring buffer, cap 5000
    aggregator: HostAggregator,          // live, fed regardless of view
    filter: Filter,                      // single filter, shared
    stream_state: StreamViewState,       // cursor + scroll + selected
    hosts_state: HostsViewState,         // cursor + collapsed processes
    prompt: Option<PromptState>,         // None = idle; Some = editing
    stats: Stats,                        // count, dropped, started_at
}

enum View { Stream, Hosts }

struct DisplayRow {
    pkt: Packet,
    parsed: Option<ParsedPacket>,        // cached at ingest
}

struct PromptState {
    field: FilterField,
    buffer: String,
    error: Option<String>,               // inline validation msg
}

enum FilterField { App, Pid, Port, Proto, Interface }
```

### Layout (master-detail)

```
┌───────────────────────────────────────────────────────────────┐
│ qk capture · 1247 pkts · 0 dropped · 0:23 · [Stream | Hosts]  │  top bar
│ Filters: app=instagram | proto=tcp                            │  filter row
├───────────────────────────────────────────────────────────────┤
│ Time         Dir  Process            Proto  Src  → Dst   B    │  header
│ 12:34:56.789  ↑   Instagram (4521)   TCP    …    → …    1424  │
│ 12:34:56.812  ↓   Instagram (4521)   TCP    …    → …    4096  │  scrollable
│ ▶12:34:57.001 ↑   Instagram (4521)   TCP    …    → …    512   │  selected
│ ...                                                            │
├───────────────────────────────────────────────────────────────┤
│ Selected packet                                                │  detail pane
│ Process:   Instagram (pid 4521)                                │
│ Direction: inbound on en0                                      │
│ Endpoints: 31.13.65.36:443 → 192.168.1.42:54321                │
│ Protocol:  TCP, 4096 bytes                                     │
│ Comment:   pid=4521 comm=Instagram iface=en0 io=0              │
├───────────────────────────────────────────────────────────────┤
│ [a]pp [p]roto [P]ort [i]face [d]pid [c]lear [q]uit            │  hotkey footer
└───────────────────────────────────────────────────────────────┘
```

Hosts view replaces the middle section with a tree-table (processes as
expandable headers, hosts indented underneath) and uses the detail pane
for the selected host's recent activity.

### Filter prompt (vim-style, inline)

Hotkeys (`a`, `p`, `P`, `i`, `d`) replace the footer with an inline
prompt:

```
:app instagram_                                          Esc cancel
```

Enter applies; Esc cancels; invalid input shows an error suffix:

```
:pid abc                                  expected number · Esc cancel
```

`c` clears all filters. `q` quits.

### Hotkey summary

All keys are case-sensitive (lowercase `p` opens the proto prompt;
uppercase `P` opens the port prompt — chosen so the two most common
filters get the same letter family without ambiguity).

| Key       | Action                                  |
|-----------|-----------------------------------------|
| `a`       | Open app prompt                         |
| `p`       | Open proto prompt (tcp/udp/icmp)        |
| `P`       | Open port prompt                        |
| `i`       | Open interface prompt                   |
| `d`       | Open pid prompt                         |
| `c`       | Clear all filters                       |
| `/`       | (Phase 6.1, not in initial scope)       |
| `Tab`     | Toggle Stream ↔ Hosts                   |
| `↑`/`↓`   | Scroll / move selection                 |
| `Enter`   | (Hosts) expand/collapse process row     |
| `q`, `Esc`| Quit (Esc only quits if no prompt open) |

## Data flow

```
pcapd (idevice)
  → RealDevice::capture_packets() — produces Packet via try_send
  → mpsc::Receiver<Packet> — same channel as today
  → tui::run() — single tokio task owning App

In tui::run():
  tokio::select! {
    pkt = rx.recv()           => app.ingest(pkt),
    key = events.next()        => app.handle_key(key),
    _ = redraw_tick.tick()     => if app.dirty { app.draw(frame); app.dirty = false },
    _ = tokio::signal::ctrl_c() => break,
  }
```

`App::ingest(pkt)`:
1. Compute `parse_summary(&pkt)` once. Stored in `DisplayRow`.
2. If `filter.accepts(&pkt, &parsed)` is false, drop and return.
3. Push to `rows`; pop front if size > 5000.
4. Feed `aggregator`.
5. Mark dirty.

`App::handle_key(k)`:
- State machine over `(prompt_state, key)` → `(new_prompt_state, mutation)`.
- All key handling is synchronous and pure (no I/O).
- Filter apply triggers `rebuild_buffer_and_aggregator()`:
  ```rust
  rows.retain(|r| filter.accepts(&r.pkt, r.parsed.as_ref()));
  aggregator = HostAggregator::new();
  for r in &rows { if let Some(p) = &r.parsed { aggregator.add(&r.pkt, p); } }
  ```

## Filter semantics

The filter is a single object that acts as a **lens**. When it changes:

1. The ring buffer is filtered in place — non-matching rows are dropped
   from view (and from storage; we don't keep hidden rows).
2. The hosts aggregator is wiped and re-fed from the surviving rows.
3. Future packets are filtered at ingest time. Pre-filter rejected
   packets never enter the buffer or aggregator.

This means changing filters loses historical data outside the ring
buffer's window (~5000 packets). That is honest: historical data
outside the ring is already gone. It also means Stream view and Hosts
view always agree on what's counted.

Trade-off accepted: a heavy filter (`--app instagram` filtering out
99% of traffic) no longer benefits from producer-side early-exit. With
parse caching the actual cost is the same as Phase 5 (parse_summary
runs once per packet either way), so this is fine in practice.

## Mode dispatch

```
qk capture                          → TUI, view=Stream
qk capture --hosts                  → TUI, view=Hosts
qk capture --app foo --proto tcp    → TUI, filter pre-populated
qk capture --dns                    → unchanged (line-based output)
qk capture --sni                    → unchanged (line-based output)
qk capture | grep instagram         → error: needs TTY
qk capture --max 100                → TUI, exits after 100 captured pkts
qk capture --save out.pcapng        → TUI + file output in background
```

Inside the TUI, Tab toggles `view` between Stream and Hosts. The DNS
and SNI flags still gate their own code paths (no TUI for those in
this iteration).

## Error handling

| Failure                             | Behavior                                                                              |
|-------------------------------------|---------------------------------------------------------------------------------------|
| stdout is not a TTY                 | Exit 1 before opening pcapd: `"qk capture needs an interactive terminal."`            |
| pcapd disconnect mid-capture        | Red footer message; view freezes; buffer remains navigable; `q` to quit               |
| Invalid prompt input (e.g. pid=abc) | Inline red error in prompt row; prompt stays open; Enter retries; Esc cancels         |
| Terminal too small (<80×24)         | Single centered message instead of layout; auto-fixes on resize event                 |
| Panic in render task                | RAII guard restores raw mode + leaves alt screen before unwinding                     |
| Write error on `--save`             | Yellow footer warning; capture continues (existing Phase 3 behavior)                  |
| iPhone physically disconnected      | Same as pcapd disconnect                                                              |
| Permission denied on `--save` path  | `CaptureFile::open` fails before TUI starts; clear error, exit 1                      |

## Testing strategy

**Preserved (zero change):** parser tests, roundtrip pcap/pcapng tests,
CLI parser tests, all filter unit tests, all hosts aggregator tests.

**Modified:** integration tests against `FakeDevice` switch to a new
`Mode::Headless` variant that drains seeded packets without opening
the TUI. Returns the final `App` state for assertions.

**New tests:**

1. `App::ingest` unit tests — drop+keep behavior, ring overflow, reset.
2. `App::handle_key` state machine — every (prompt_state, key) → result.
3. Insta snapshot tests of `App::draw` via `ratatui::backend::TestBackend`:
   - Stream view with rows + active filter
   - Hosts view with 2 processes
   - Prompt open with partial buffer
   - Empty state
   - Terminal-too-small fallback
4. Headless integration test — fake device + seeded packets + filter,
   assert ring + aggregator end state.

**Manual validation (recorded in commit message):**

- Capture 1 min real iPhone traffic; exercise every hotkey
- Resize terminal mid-capture
- Run `qk capture | cat` (must fail cleanly)
- Run `qk capture --save out.pcapng` and verify file opens in Wireshark

## Performance

- Redraw budget: 30Hz (33ms ticks). Dirty flag prevents idle CPU.
- Ring buffer at 5000 entries × ~200 bytes = ~1MB resident.
- Filter rebuild on change: 5000 entries × 5 string compares = sub-ms.
- Aggregator rebuild on change: O(rows in buffer), single pass, sub-ms.

## Open questions / future work

- `/` search inside the buffer (substring across all visible columns).
  Deferred — straightforward to add once the App + key handling is
  stable.
- Sort columns in hosts view (`s` cycles by pkts/bytes_out/bytes_in).
  Deferred for the same reason.
- Theme / color customization. Today's `owo-colors` palette is used
  via fixed mappings; users wanting different colors edit the source.
- Wireshark-style separate capture filter / display filter. The
  current "filter = lens" model is simpler and matches the use case.
  If captures at line-rate become a real bottleneck, the producer-side
  capture filter from Phase 4 can be reintroduced behind a flag.

## Definition of Done

- All sections of this design implemented as described
- Existing 200+ tests still passing
- New tests added per the testing strategy
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` pass
- Manual validation steps performed on a real iPhone
- Commit message references this spec
