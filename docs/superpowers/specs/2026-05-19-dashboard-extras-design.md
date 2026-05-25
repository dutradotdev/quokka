# Dashboard extras — design

**Date:** 2026-05-19
**Status:** Approved (pending implementation plan)
**Author:** Lucas Dutra (with Claude)

## Goal

Enrich the welcome dashboard introduced in `2026-05-19-dashboard-welcome-design.md` with four new categories of info that are useful at a glance: **content** (app count), **energy** (charger wattage), **system** (locale, timezone, Developer Mode, Find My), and **history** (last backup, pair date).

The dashboard stays a single instant snapshot — no new commands. The added rows render only when their data is available, and the "alert" line at the bottom only appears when a flag is in an unusual state, so the screen does not grow taller on every device.

## Non-goals

- No new subcommand. Everything renders inside the existing dashboard / `quokka status` output.
- No iOS update check against Apple's IPSW feed (out of scope: requires network + sustained version mapping).
- No DCIM photo/video count (AFC walk costs 5–30 s — kills the "instant" feel of the dashboard).
- No additional PII surface — carrier, phone number, serial number, IMEI, exact pair time stay out as agreed in the previous spec.
- No biggest-app callout — only the count, per user pick: static `StaticDiskUsage` understates real footprint, the `apps` command is the authoritative view.

## Layout

Target stays 80 columns. The art column and right-hand status block are unchanged. Two changes:

1. The OS line now appends locale + time zone when both are known.
2. A two-line **footer** is rendered below the side-by-side block (or below the stacked status block in narrow mode).
   - Footer line 1 — trivia: `47 apps · last backup 3 days ago · paired since Mar 2024`. Only the segments whose values are `Some` are joined with ` · `. If every segment is `None`, the entire line is omitted.
   - Footer line 2 — alerts: each "abnormal" flag adds a `⚠ <message>` segment, separated by ` · `. If no flag is abnormal, the line is omitted. Definition of abnormal: `developer_mode == Some(true)` → "Developer Mode on"; `find_my == Some(false)` → "Find My off". `None` and the "expected" value both render nothing.

Inline change to the battery row: when charging, the current `⚡ charging` becomes `⚡ <watts>W <description>` if `adapter_watts` is known (e.g. `⚡ 20W USB-C`). Falls back to `⚡ charging` when only the boolean is known.

Mock (80 col, fully populated, all alerts triggered):

```
                                Lucas's iPhone (iPhone 14 Pro Max)
        ___________             iOS 18.2 (build 22C152) · pt-BR · São Paulo
       /           \
      /   o     o   \           Storage  ████████░░ 60%  150.6 / 256 GB
     |     \___/     |                   ├─ System    12.4 GB
      \    \___/    /                    ├─ Data     148.2 GB
       \___________/                     └─ Free      95.4 GB
       /|         |\
      / |         | \           Battery  level   87% ⚡ 20W USB-C
     (__|_________|__)                   health  91%
         U       U                        cycles  142
                                          temp    27.4 °C

  47 apps · last backup 3 days ago · paired since Mar 2024
  ⚠ Developer Mode on
```

If everything is `None` / "expected", the dashboard renders exactly the same lines as it does today — no empty footer, no padding.

## Changes to `DeviceStatus` (`src/device/mod.rs`)

All new fields are `Option`-typed for the same reason the existing extras are: missing values must render as "nothing", never as zero or empty string.

```rust
pub struct DeviceStatus {
    // ... existing ...
    pub locale: Option<String>,                // e.g. "pt-BR"
    pub time_zone: Option<String>,             // e.g. "America/Sao_Paulo"
    pub app_count: Option<usize>,              // user apps only
    pub developer_mode: Option<bool>,
    pub find_my: Option<bool>,
    pub last_backup_unix: Option<i64>,         // seconds since epoch
    pub paired_since_unix: Option<i64>,
}

pub struct Battery {
    // ... existing ...
    pub adapter_watts: Option<u32>,
    pub adapter_description: Option<String>,   // e.g. "USB-C", "USB Host"
}
```

`paired_since_unix` lives in `DeviceStatus` even though it is read from the local filesystem (the pairing-record file's creation time), not from the device. The boundary is "everything the welcome screen needs to render" — the dashboard renderer should not learn about filesystem paths.

## Lockdown reads

Added to `RealDevice::read_lockdown_info`:

| Field | Source |
| --- | --- |
| `locale` | lockdown root `Locale` (e.g. `pt_BR`) — normalize to BCP-47 form (`pt-BR`) before storing |
| `time_zone` | lockdown root `TimeZone` |
| `developer_mode` | lockdown `com.apple.security.mac.amfi` `DeveloperModeStatus` (may be `MobileGestaltDeprecated` on iOS 17+) |
| `find_my` | lockdown `com.apple.MobileDeviceCrashCopy` / `com.apple.fmip.fmipd` — to be confirmed during probing |
| `last_backup_unix` | lockdown `com.apple.mobile.iTunes` `LastiTunesSyncFromDevice` (date value, may be deprecated) |
| `adapter_watts` / `adapter_description` | lockdown `com.apple.mobile.battery` `AdapterDetails` dict — only populated while charging |

For battery: `AdapterDetails` is a dict; relevant keys observed historically are `Watts` (integer), `Description` ("USB-C", "USB Power Adapter", "USB Host"), and `FamilyCode`. Read defensively — any may be absent.

**Caveat — same shape as the enclosure-color caveat in the previous spec:** several of these keys are known to return `MobileGestaltDeprecated` or simply nothing on iOS 17+. The first task of the implementation plan is to **probe a real device and confirm what each key actually returns**, then drop the ones that are dead from the implementation entirely (rather than wiring code that always returns `None`). The footer logic already degrades to "render nothing" when every segment is missing, so a partial result still ships value.

## App count (`installation_proxy`)

`status()` gains a third parallel arm (alongside `read_lockdown_info` and `read_battery_diag`) that calls `lookup_apps(provider, None, false, "User")` — the same fast path the `apps` command uses. The dashboard only needs the *count*, so the response is reduced to `apps.len()` and discarded. Bundle ids and names are not stored on `DeviceStatus`.

Cost on a 50-app device is ~100–300 ms. Already overlapped with the slowest existing call (battery diag), so wall-clock impact is bounded by the slowest of the three.

If the call fails (e.g. installation_proxy unreachable) the whole `status()` should **not** fail — the app-count arm degrades to `None` and the footer drops the segment.

## Paired-since (local filesystem)

The pair record lives at `/var/db/lockdown/<UDID>.plist` (system path) or `~/Library/Lockdown/<UDID>.plist` (user path, modern macOS). Read `fs::metadata(...).created()` for whichever path the `idevice` provider used, convert to Unix seconds.

`idevice`'s `IdeviceProvider` does not currently expose the on-disk path of the pairing record. Two options:
1. Re-derive the path from the `UDID` exposed by usbmuxd — duplicates internal `idevice` logic, but is one line.
2. Add a tiny helper in `mod real` that knows both candidate locations and `stat`s the first that exists.

Implementation plan picks (2) — keeps the assumption local and easy to update if `idevice` moves the path.

If neither file exists or `metadata().created()` is unsupported on the host filesystem, the field is `None` and the segment vanishes.

## Renderer changes (`src/commands/dashboard.rs`)

Two new private helpers:

```rust
fn render_footer(status: &DeviceStatus) -> Option<String>
fn format_relative_date(unix: i64, now_unix: i64) -> String  // "3 days ago", "Mar 2024", "today"
```

`render_footer` returns `Option<String>` with up to two lines joined by `\n`. `None` means "render nothing" — the top-level `render` skips both the blank separator and the footer in that case, preserving today's output for a device with no extras populated.

`format_relative_date` rules:
- < 60 s → "just now"
- < 24 h → "N hours ago" / "1 hour ago"
- < 30 days → "N days ago"
- < 365 days → "Mon YYYY" (e.g. "Mar 2024")
- ≥ 365 days → "YYYY"

Pure function, takes `now_unix` for testability. The dashboard's `render` reads the real clock once and threads it through.

Inline change to `render_battery`: when both `is_charging == Some(true)` and `adapter_watts.is_some()`, format `⚡ {watts}W {description?}`; trailing description is omitted when `None`.

The existing top-level `render` signature gains a `now_unix: i64` parameter. Callers (`commands::menu`, `commands::status`) pass `chrono::Utc::now().timestamp()` (or `std::time::SystemTime` arithmetic) so tests can pin time.

## Testing

Unit tests in `dashboard.rs`:
- Footer omitted entirely when every new field is `None`.
- Footer line 1 joins only the `Some` segments (e.g. only `app_count` set → "47 apps").
- Footer line 2 only shows when a flag is abnormal; both expected values → line omitted.
- Charging with `adapter_watts = Some(20)` and `adapter_description = Some("USB-C")` → "⚡ 20W USB-C".
- Charging with `adapter_watts = None` → falls back to "⚡ charging".
- OS line appends `locale · time_zone` only when both `Some`.
- `format_relative_date` covers each branch (just now, hours, days, months, years) with fixed `now_unix`.

Integration test in `tests/integration.rs`:
- `FakeDevice` populated with every new field → dashboard contains all expected fragments.
- `developer_mode = Some(false)` and `find_my = Some(true)` → footer line 2 absent.
- `developer_mode = Some(true)` → "Developer Mode on" present.
- `find_my = Some(false)` → "Find My off" present.

E2E (`--features e2e`): existing `quokka status` smoke test already covers the end-to-end happy path. The new fields are best-effort by design, so no new E2E assertions.

## Out of scope for this spec

- Per-segment colour. Footer rendering is plain (dimmed); only the `⚠` glyph in line 2 is coloured yellow, matching the existing warning glyph convention.
- A `--verbose` flag that surfaces additional fields (raw enclosure-color string, full `AdapterDetails` dict, etc.). Could ship later if there is demand.
- Persistent caching of pair date or last-backup-unix between runs. Each invocation reads fresh.
- Surfacing iOS-update-available — needs an off-device API call and a version-mapping table; revisit if user demand justifies the maintenance cost.
