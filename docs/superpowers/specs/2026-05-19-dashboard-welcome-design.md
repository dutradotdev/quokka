# Dashboard welcome screen — design

**Date:** 2026-05-19
**Status:** Approved (pending implementation plan)
**Author:** Lucas Dutra (with Claude)

## Goal

Replace the current `quokka` (no args) menu header — a tiny 5-line ASCII quokka, tagline, and select list — with a welcome **dashboard** that shows the iPhone's current state as soon as the user runs the binary. Two-column layout: a detailed ASCII quokka tinted by the device's enclosure color on the left, and the existing status block (enriched with several new fields) on the right. The numbered menu sits below.

The point is delight: the first thing a user sees when they plug in their iPhone and run `quokka` should feel like a small character greeting them with a live read of their device.

## Non-goals

- No interactive TUI framework (ratatui) just for the dashboard — content is static after one read.
- No per-app cache cleanup, crash logs, or anything else listed under "out of scope" in `CLAUDE.md`.
- No changes to `apps` or `analyze` commands.
- No persistent cache of device color or any other field between runs.
- No phone number, IMEI, serial number, or other PII-sensitive fields.

## Layout

Target width 80 columns. ASCII art on the left (~28 chars wide, 10 lines), status block on the right. The menu sits below the two columns. When `crossterm::terminal::size()` reports a width below 70 columns, the layout falls back to stacked: art on top, status below, menu under that.

Mock (80 col, all fields populated):

```
                                Lucas's iPhone (iPhone 14 Pro Max)
        ___________             iOS 18.2 (build 22C152)
       /           \
      /   o     o   \           Storage  ████████░░ 60%  150.6 / 256 GB
     |     \___/     |                   ├─ System    12.4 GB
      \    \___/    /                    ├─ Data     148.2 GB
       \___________/                     └─ Free      95.4 GB
       /|         |\
      / |         | \            Battery  level   87% ⚡ charging
     (__|_________|__)                    health  91%
         U       U                        cycles  142
                                          temp    27.4 °C

  Inspect and tidy your iPhone from the Mac · by Lucas Dutra

  ❯ Apps      List & uninstall user apps
    Analyze   Find the heaviest media files
    Refresh   Re-read device info
    Quit
```

The `Status` item is removed from the menu — the dashboard *is* the status screen. Its slot is replaced by **Refresh**, which forces a re-read of `device.status()` and redraws.

## Changes to the `Device` trait (`src/device.rs`)

All new fields are `Option`-typed so existing tests that build `DeviceStatus::default()` keep working and so the renderer degrades gracefully on older iOS where a field might not be available.

### `DeviceStatus`

Added:

- `model_friendly: Option<String>` — resolved from `model` (e.g. `iPhone15,3`) via a static lookup table.
- `ios_build: Option<String>` — lockdown `BuildVersion`.
- `enclosure_color: Option<String>` — raw value from lockdown `DeviceEnclosureColor` (or `DeviceColor` as fallback). Opaque string; mapping to a terminal color happens in the UI layer.

### `Storage`

Added:

- `system_bytes: Option<u64>` — lockdown disk domain `TotalSystemCapacity`.
- `data_used_bytes: Option<u64>` — derived from `TotalDataCapacity - AmountDataAvailable` (or equivalent keys).

The existing `total_bytes` / `free_bytes` stay. Renderer shows the three-line breakdown only when both new fields are `Some`; otherwise falls back to the single-line bar that already works.

### `Battery`

Added:

- `is_charging: Option<bool>` — from `com.apple.mobile.battery` (`BatteryIsCharging` and/or `ExternalConnected`; whichever turns out to be reliable on iOS 17+ — to be confirmed during implementation).

## Model name lookup (`src/device/model_names.rs`)

New module. Pure function `friendly_name(identifier: &str) -> Option<&'static str>` over a static `&[(&str, &str)]` slice or a `phf` map (decide during implementation; both are fine). Case-insensitive match. Unknown identifiers return `None`, the renderer falls back to showing the raw identifier alone.

The initial table covers iPhone 8 through whatever is current at implementation time. Apple Watch / iPad identifiers are not included — quokka is iPhone-only.

## Renderer (`src/commands/dashboard.rs`, new module)

Pure functions, no I/O, fully unit-testable.

- `render(status: &DeviceStatus, term_width: u16) -> String` — top-level. Picks side-by-side vs. stacked based on `term_width`.
- `render_art(color: Option<&str>) -> String` — returns the ASCII quokka tinted with `pick_color(color)`.
- `render_status_block(status: &DeviceStatus) -> String` — the right-column content (header, iOS+build, storage breakdown, battery with charging indicator).
- `pick_color(enclosure_color: Option<&str>) -> owo_colors::AnsiColors` — the mapping table below.

### Color mapping

```
"Black" | "Space Black" | "Midnight" | "Graphite"                  → bright_black
"White" | "Starlight" | "Silver"                                   → white
"Blue" | "Sierra Blue" | "Pacific Blue" | "Blue Titanium"          → blue
"Red" | "Product Red" | "(PRODUCT)RED"                             → red
"Green" | "Alpine Green" | "Midnight Green"                        → green
"Gold" | "Yellow" | "Desert Titanium" | "Natural Titanium"         → yellow
"Purple" | "Deep Purple"                                           → magenta
"Pink" | "Rose Gold" | "Coral"                                     → bright_magenta
None / unknown                                                     → green   (current default)
```

Matching is case-insensitive and trims whitespace. Unknown values silently fall back to green — the dashboard never shows a "couldn't determine color" warning.

### Caveat: lockdown enclosure color may not be human-readable

Apple's lockdown values for `DeviceEnclosureColor` are sometimes numeric strings (e.g. `"1"`, `"#3a3a3c"`) rather than the names above. **The first task in the implementation plan is to probe a real device** to find out what shape the value actually takes and adjust the mapping accordingly. If it turns out to be opaque on modern iOS, this feature ships in fallback (green) mode and we revisit later — it does not block the other three extras.

## Menu changes (`src/commands/menu.rs`)

- `render_header()` is replaced by a call to `dashboard::render(&status, term_width)`.
- `device.status()` is called once per loop iteration (so Refresh re-runs it).
- Choices become `Apps`, `Analyze`, `Refresh`, `Quit`. `Refresh` continues the loop without running anything; the redraw at the top of the loop picks up the new status.
- Existing wait-for-Enter behavior after running a subcommand stays.

## Status command (`src/commands/status.rs`)

`quokka status` continues to exist as a non-interactive shortcut. It now calls `dashboard::render(&status, term_width)` instead of the old `render_status`. Old `render_status` becomes a private helper inside `dashboard.rs` (or is folded into `render_status_block`).

## Testing

Unit tests in `dashboard.rs`:

- Renders full status with all extras (assert friendly name, build, three storage lines, charging bolt all present).
- Renders with each new field individually missing (graceful degradation).
- Side-by-side vs. stacked layout switches at the width threshold.
- Each color mapping returns the expected `AnsiColors`, unknown returns green.

Unit tests in `device/model_names.rs`:

- Known identifier → expected friendly name.
- Unknown identifier → `None`.
- Case-insensitive match.

Integration test in `tests/integration.rs`:

- Fake `Device` returning a fully populated `DeviceStatus`.
- `dashboard::render` output contains friendly name, iOS build, three storage rows, charging indicator.
- A second case with `is_charging: Some(false)` does NOT contain the bolt.

E2E behavior is not added — the existing `e2e_smoke` already covers `quokka status` against a real device, and the new fields are best-effort.

## Out of scope for this spec

- Animating the quokka.
- Resolving enclosure color to a non-ANSI palette (truecolor) — fixed 8-color mapping for now.
- Localization of the menu strings.
- Showing carrier, region, or activation date even though the data is reachable — kept out to avoid feature creep and PII surface.
