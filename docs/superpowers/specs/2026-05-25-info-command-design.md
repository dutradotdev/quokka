# `qk info` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Summary

A read-only `qk info` command that prints the connected iPhone's identity — name, model, iOS build, serial, IMEI, network addresses, supervised state, developer mode — in three labeled blocks. Supports `--redact` for masking PII before screenshots. Reads everything through lockdown `get_value`; no AFC, no TUI.

## Motivation

`qk status` shows dynamic state (battery level, free storage) and the dashboard already presents it. There is no first-class command for the device's *identity* — the static, "what device is this" fields a developer or sysadmin reaches for when triaging a phone (serial, UDID, iOS build, supervised flag). The data is reachable via plain lockdown reads; the value is in curating and rendering it.

## Scope

### In scope

- Read ~15 lockdown values and group them into three blocks: **Device**, **System**, **Network**.
- `--redact` / `-r` flag that masks PII fields for safe sharing.
- Graceful degradation: any optional field that fails to read is omitted entirely (no `—`, no "unknown").
- Menu launcher entry.

### Out of scope

- Carrier / cellular plan info. `com.apple.commcenter` is restricted, data is volatile.
- Phone number, ICCID, MEID. PII surface we do not want by default.
- Uptime / boot time. Not reachable through lockdown-classic on iOS 17+ without DVT instruments.
- `--json` output. Listed in future work.
- Interactive TUI. Output is plain text, single shot, to stdout.

## CLI surface

### Synopsis

```
quokka info [-r | --redact]
```

- `--redact` / `-r` (default off): mask PII fields per the rules below.

### Behavior

1. User runs `qk info` (optionally with `--redact`).
2. Spinner: `Reading device info…`.
3. Three blocks print to stdout, separated by a blank line: **Device**, **System**, **Network**.
4. Program exits `Ok`.

Target width 60 columns. Label column 18 chars left-aligned, value left-aligned. Fields with `None` values are omitted (no placeholder). Block order and field order are fixed — same input produces byte-for-byte same output.

Mock with every field populated:

```
Device
  Name              Lucas's iPhone
  Model             iPhone 15 Pro Max (iPhone16,2)
  Model number      MQ8X3LL/A
  Region            LL/A
  Color             Natural Titanium
  Serial            F2LXXXXXXXXX
  UDID              00008130-001A2B3C4D5E6F7G

System
  iOS               18.2 (build 22C152)
  Hardware          D74AP
  CPU               arm64e
  Activation        Activated
  Supervised        No
  Developer mode    Off

Network
  Wi-Fi MAC         AA:BB:CC:DD:EE:FF
  Bluetooth MAC     AA:BB:CC:DD:EE:F0
  IMEI              350123456789012
  IMEI 2            350123456789013
```

A block whose every field is `None` after filtering is skipped entirely (header included). A device returning only `Name`, `Model`, and `iOS` still prints a useful output without empty headers.

#### Redaction

When `--redact` is set, the renderer masks PII by keeping the last 4 characters and replacing everything before them with `*`. Length is preserved (including separators in MACs and UDID). Values shorter than 4 characters become a fully-masked string of the same length.

Masked fields:

- `Serial`, `UDID` (Device block)
- `Wi-Fi MAC`, `Bluetooth MAC`, `IMEI`, `IMEI 2` (Network block)

Not masked: name, model, model number, region, color, iOS version/build, hardware, CPU, activation, supervised, developer mode.

Mock with `--redact` on the same input:

```
Device
  Name              Lucas's iPhone
  Model             iPhone 15 Pro Max (iPhone16,2)
  Model number      MQ8X3LL/A
  Region            LL/A
  Color             Natural Titanium
  Serial            ********XXXX
  UDID              ***************************6F7G

...

Network
  Wi-Fi MAC         **:**:**:**:EE:FF
  Bluetooth MAC     **:**:**:**:EE:F0
  IMEI              ***********9012
  IMEI 2            ***********9013
```

## Architecture

### Device trait changes

A new struct, parallel to `DeviceStatus`:

```rust
pub struct DeviceInfo {
    pub name: String,                          // DeviceName
    pub model_identifier: String,              // ProductType, e.g. "iPhone16,2"
    pub model_friendly: Option<String>,        // resolved via model_names::friendly_name
    pub model_number: Option<String>,          // ModelNumber, e.g. "MQ8X3LL/A"
    pub region_info: Option<String>,           // RegionInfo
    pub enclosure_color: Option<String>,       // DeviceEnclosureColor (or DeviceColor fallback)
    pub serial: String,                        // SerialNumber
    pub udid: String,                          // UniqueDeviceID

    pub ios_version: String,                   // ProductVersion
    pub ios_build: Option<String>,             // BuildVersion
    pub hardware_model: Option<String>,        // HardwareModel, e.g. "D74AP"
    pub cpu_architecture: Option<String>,      // CPUArchitecture
    pub activation_state: Option<String>,      // ActivationState
    pub is_supervised: Option<bool>,
    pub developer_mode_enabled: Option<bool>,

    pub wifi_address: Option<String>,          // WiFiAddress
    pub bluetooth_address: Option<String>,     // BluetoothAddress
    pub imei: Option<String>,                  // InternationalMobileEquipmentIdentity
    pub imei2: Option<String>,                 // InternationalMobileEquipmentIdentity2
}
```

New trait method:

```rust
#[async_trait]
pub trait Device: Send + Sync {
    // ... existing methods ...

    async fn info(&self) -> Result<DeviceInfo>;
}
```

All `Option` fields degrade silently to `None` on a per-field `get_value` failure. Only required fields (`name`, `model_identifier`, `serial`, `udid`, `ios_version`) abort the call. The seam rule from `CLAUDE.md` holds: no `idevice` type leaks through `DeviceInfo`.

A separate method (not extending `DeviceStatus`) is the right shape because `status()` is cheap and called repeatedly by the dashboard / `Refresh` loop, while `info()` reads ~15 keys and only needs to run once per session. Keeping them separate avoids paying the info cost on every `Refresh`.

### Real implementation notes

- Reuse the existing `LockdownClient`. One `connect`, then call `get_value(domain, key)` per field. Sequential is fine — these are small key reads, not bulk operations.
- `model_friendly` resolves via `device::model_names::friendly_name` (introduced by the dashboard spec). If that module is not yet merged when this lands, ship a minimal stub that returns `None` and let the dashboard work pick up the table later.
- Required fields use `?`; optional fields wrap each `get_value` call in `.await.ok()`.

### FakeDevice additions

Add `pub info: DeviceInfo` with a fully-populated default that matches the mock above. `info()` returns `self.info.clone()`.

Add a `with_minimal_info()` helper that produces a `DeviceInfo` with only the required fields set and every `Option` as `None` — feeds the graceful-degradation integration test.

### Command module structure

`src/commands/info.rs` mirrors the shape of `status.rs`:

```rust
pub async fn run(device: &dyn Device, redact: bool) -> Result<()> { ... }

fn render(info: &DeviceInfo, redact: bool) -> String { ... }  // pure, unit-tested

fn render_device_block(info: &DeviceInfo, redact: bool) -> Option<String> { ... }
fn render_system_block(info: &DeviceInfo) -> Option<String> { ... }  // no PII, no redact arg
fn render_network_block(info: &DeviceInfo, redact: bool) -> Option<String> { ... }

fn redact_tail(value: &str, visible_tail: usize) -> String { ... }
```

Each `render_*_block` returns `None` when every field is `None`; `render` joins `Some` blocks with `\n\n`. No magic numbers — named `const`s at the top of the module:

```rust
const LABEL_WIDTH: usize = 18;
const REDACT_VISIBLE_TAIL: usize = 4;
```

The model-friendly + identifier concatenation is a pure helper:

```rust
fn format_model(info: &DeviceInfo) -> String {
    match &info.model_friendly {
        Some(friendly) => format!("{friendly} ({})", info.model_identifier),
        None => info.model_identifier.clone(),
    }
}
```

`run` opens a brief spinner (same `indicatif` helper `status` uses), calls `device.info()`, prints `render(&info, redact)` to stdout, returns `Ok`.

### Dispatch + menu integration

`src/lib.rs` gains an `Info` subcommand with the `--redact` / `-r` flag, dispatching to `commands::info::run(&*device, redact)`.

`src/commands/menu.rs` gains an `Info` entry between `Analyze` and `Refresh`. Menu invocation runs `info::run(device, /* redact */ false)` and waits for Enter, same pattern as `Apps` and `Analyze`. The redact flag is only reachable from the CLI in v1.

## Verification needed

Probe against a real device before locking the implementation:

1. **`is_supervised`** — assumed at lockdown `com.apple.mobile.chaperone` domain, key `IsSupervised` (boolean). Devices that have never been supervised may omit the key entirely → `None`.
2. **`developer_mode_enabled` (iOS 16+)** — assumed at lockdown `com.apple.security.mac.amfi` domain, key `DeveloperModeStatus`. iOS <16 returns nothing → `None` (correct).
3. **Exact `idevice` crate method names** for each `get_value` call — confirm against `tools/src/` in the upstream repo.

The command must work even if `is_supervised` and `developer_mode_enabled` end up always `None` — they are nice-to-haves, not gates. If a probe path turns out to be unreachable, leave the field `Option<bool>` and let the renderer omit the line.

## Error handling

- **Lockdown connect fails entirely**: emit the trust-and-replug guidance the other commands already use (reuse the helper, do not duplicate copy).
- **A required field fails to read** (name, model, serial, udid, ios): abort with `Could not read device identity (failed on: <key>). Try replugging the device.` Required fields failing means the lockdown session is wedged; partial output would be misleading.
- **An optional field fails**: silently `None`, line omitted.

No raw `idevice` error text reaches the user. Wrap with `anyhow::Context` at the trait boundary; surface human copy at the command boundary.

## Testing strategy

### Unit (`#[cfg(test)] mod tests` in `info.rs`)

- `format_model` with both friendly+identifier and identifier-only.
- `redact_tail`: 15-digit IMEI keeps last 4; MAC `AA:BB:CC:DD:EE:FF` keeps last 4 chars (separators count as chars, intentional); 3-char value is fully masked; empty input returns empty.
- `render_device_block` with all fields, with only required fields, with mid-block field missing (e.g. no `model_number`). One case with `redact: true` asserts Serial and UDID are masked while Name/Model/Color are not.
- `render_system_block` with `is_supervised: None` omits the line; with `Some(true)` renders "Yes"; with `Some(false)` renders "No". Same for `developer_mode_enabled`.
- `render_network_block` returns `None` when every network field is `None`. One case with `redact: true` asserts every visible field is masked.
- `render` with `with_minimal_info()` produces exactly the Device + System blocks, no Network block, no trailing blank line.

### Integration (`tests/integration.rs`)

- `FakeDevice::default()` → `info::run(device, false)` produces output containing every label from the mock, and the raw serial / IMEI strings appear verbatim.
- `FakeDevice::default()` → `info::run(device, true)` produces output where the raw serial / UDID / IMEI / MAC strings do **not** appear, and the masked versions do.
- `FakeDevice::with_minimal_info()` → output contains `Name`, `Model`, `Serial`, `UDID`, `iOS`; does **not** contain `Network`, `IMEI`, `Wi-Fi MAC`, `Supervised`, `Developer mode`.
- Snapshot-style assertion on byte-for-byte output of the full-info case (both redacted and non-redacted) — reordering or whitespace drift fails the test.

### E2E (`tests/e2e_*.rs`, behind `--features e2e`)

- Smoke: connect, call `device.info()`, assert `name`, `model_identifier`, `serial`, `udid`, `ios_version` are non-empty. Probe `is_supervised` and `developer_mode_enabled` and **print** their values (not assert) — the e2e run also confirms the lockdown paths from the "Verification needed" section.

## Future work

- `--json` for scripting / piping into `jq`.
- Menu launcher gains a "Hide PII" toggle that re-runs `info::run(device, true)` without re-fetching from lockdown (cache `DeviceInfo` for the lifetime of the menu loop).
- Carrier and cellular plan info, if a future use case justifies the PII surface.
- Boot time / uptime if a non-DVT path turns up.
