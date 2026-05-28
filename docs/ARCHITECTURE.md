# Architecture

This document explains how quokka is put together and, more importantly, *why*.
If you only read one section, read [The `Device` seam](#the-device-seam) — it is
the rule the rest of the codebase is shaped around.

## The big picture

quokka is a command-line tool. A run looks like this:

```text
quokka.rs / qk.rs   →   lib.rs::run()   →   device::connect()   →   commands::*::run()
   (thin shim)          (parse + dispatch)   (one connection)      (per-command logic)
```

1. **`src/bin/quokka.rs` and `src/bin/qk.rs`** are four-line shims. They exist
   only so the tool can be invoked under two names. They both call `run()`.
2. **`src/lib.rs`** owns `run()`. It parses arguments with `clap`, connects to
   the device **once**, then dispatches to the matching command module. All
   integration tests go through the library, never by spawning the binary.
3. **`src/device/mod.rs`** is the boundary with the iPhone (see below).
4. **`src/commands/`** holds one module per command. Commands receive a
   `&dyn Device` — they never see the iPhone directly.
5. **`src/ui.rs`** centralises terminal output helpers (byte formatting,
   progress bars, spinners) so every command renders consistently.

Because `run()` lives in the library and commands take a trait object, the
whole tool can be driven in a test without a binary, a terminal, or an iPhone.

## The `Device` seam

**Every operation quokka performs on an iPhone goes through the `Device` trait
in `src/device/mod.rs`.** This is the single most important design decision in
the project.

```rust
#[async_trait]
pub trait Device: Send + Sync {
    async fn status(&self) -> Result<DeviceStatus>;
    async fn apps(&self) -> Result<Vec<App>>;
    async fn with_dynamic_sizes(&self, apps: Vec<App>, on_batch: BatchCallback) -> Result<Vec<App>>;
    async fn app(&self, bundle_id: &str) -> Result<Option<App>>;
    async fn uninstall_app(&self, bundle_id: &str) -> Result<()>;
    async fn afc_walk(&self, roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>>;
    async fn afc_delete(&self, path: &str) -> Result<()>;
}
```

There are two implementations:

- **`RealDevice`**, in a private `mod real` submodule, talks to the
  [`idevice`](https://github.com/jkcoxson/idevice) crate over `usbmuxd`.
- **`FakeDevice`** is an in-memory implementation used by tests. It is `pub` so
  the integration tests in `tests/` can construct it; production code never does.

### Why the seam exists

The `idevice` crate is pre-1.0 and **ships breaking changes at nearly every
point release until 0.2.0**. The seam isolates that churn:

- **No `idevice` type may appear in the public surface of `device/mod.rs`.**
  The trait deals only in quokka's own types (`DeviceStatus`, `App`,
  `MediaFile`, …). When `idevice` breaks, the damage is contained to
  `mod real` — the trait and everything above it stay still.
- The dependency is pinned with `=0.1.x` in `Cargo.toml` for the same reason.
- Because commands depend on the *trait*, not the crate, every command is
  testable against `FakeDevice` with zero hardware.

### Adding a capability

To add a new device operation (say, reading crash logs):

1. Add a method to the `Device` trait.
2. Implement it in `mod real` against `idevice`.
3. Implement it in `FakeDevice` with seeded in-memory data.
4. Consume the trait method from a command.
5. Write unit/integration tests using `FakeDevice`.

If step 4 needs an `idevice` type, the seam has leaked — fix the trait instead.

## Commands

Each file in `src/commands/` is one command and exposes an
`async fn run(device: &dyn Device, …)`:

- **`status.rs`** — fetches a `DeviceStatus` and prints the dashboard once.
- **`apps.rs`** — lists user apps by size. On a TTY it opens a `ratatui`
  picker whose sizes update *live* as a background "Phase 2" enrichment pass
  streams in dynamic disk usage. Also handles `--uninstall`.
- **`analyze.rs`** — walks the AFC media roots (`/DCIM`, `/Downloads`,
  `/Recordings`, `/Books`), then either prints the heaviest files (read-only)
  or, with `--delete`, opens a `ratatui` deletion picker.
- **`dashboard.rs`** — a **pure renderer**: it turns a `DeviceStatus` into the
  two-column dashboard string. No I/O, so every layout decision is
  unit-testable. Shared by `status` and the launcher.
- **`menu.rs`** — the interactive launcher shown when `quokka` is run with no
  subcommand on a TTY.
- **`card/`** — `qk card` renders a 1080×1080 PNG snapshot of the device for
  social sharing. The pipeline has four pure layers and one IO layer:
  `data.rs` projects `DeviceStatus + now → CardData` (all time-derived
  values are pre-formatted strings, so the renderer is deterministic);
  `badges.rs` evaluates 15 eligibility checks and ranks the top 3;
  `render.rs` is a pure `fn render_svg(&CardData) -> String`; `png.rs`
  rasterises via `resvg` with JetBrains Mono embedded via `include_bytes!`
  and registered in `usvg::Options::fontdb`; `share.rs` formats the Twitter
  intent URL. `mod.rs` is the only layer that touches the filesystem and
  spawns `open` for Preview.

`apps` and `analyze` are the only commands with an interactive TUI; `card`
writes a PNG and exits; everything else prints a plain block of output.

## UX principles

These are enforced by the shared helpers in `ui.rs` and by `anstream` /
`owo-colors` at the stream level — don't reinvent them:

- **Colour and animation gate on TTY + `NO_COLOR` automatically.** `anstream`
  strips ANSI when piped; `indicatif::ProgressDrawTarget::stderr()` hides
  spinners on a non-TTY.
- **Operations longer than ~1s show a spinner or progress bar.**
- **Errors say what happened *and* what to do.** No raw stack traces.
- **Destructive actions require explicit confirmation.** Without a TTY,
  destructive commands abort rather than assume "yes" — see `analyze --delete`.
- **`--dry-run` behaviour is the default** where it makes sense: `analyze`
  never deletes unless `--delete` is set.

## Test layers

| Layer           | Location                        | Needs an iPhone? | Runs in CI? |
| --------------- | ------------------------------- | ---------------- | ----------- |
| Unit            | `#[cfg(test)]` next to the code | No               | Yes         |
| Integration     | `tests/integration.rs`          | No (uses fake)   | Yes         |
| End-to-end      | `tests/e2e_*.rs` (`e2e` feature)| Yes              | No          |

The first two layers are the **regression net**: they pin the current
behaviour so a future change that breaks it fails CI before it can merge.
That is why new logic must ship with tests in the same change — see
[CONTRIBUTING.md](../CONTRIBUTING.md).

The `e2e` layer can't run in CI (no hardware), so CI compile-checks it instead:
a broken e2e test file still fails the build.

## iOS / `idevice` notes

quokka deliberately uses **only lockdown-classic services** — `lockdown`,
`diagnostics_relay`, `afc`, `installation_proxy` — over `usbmuxd`. It does not
open the `core_device_proxy` / RemoteXPC tunnel, which would be required for
the DVT / DTServiceHub services on iOS 17+. The MVP does not need them, and
avoiding the tunnel keeps the privilege requirements at zero.

A few iOS 17.4+ quirks are handled in `mod real` and worth knowing before you
touch battery code:

- `diagnostics_relay.mobilegestalt(...)` returns `MobileGestaltDeprecated` for
  every key on modern iOS — don't use it. Battery **level** comes from the
  lockdown `com.apple.mobile.battery` domain instead.
- `diagnostics_relay.gasguage()` still works, but its response is wrapped one
  level deep under a `"GasGauge"` key — unwrap the inner dict first.
- On iOS 17+, `FullChargeCapacity` is reported as a *percentage* of design
  capacity (matching Settings → Battery Health). The heuristic in
  `compute_health_percent`: if the value is ≤ 100, treat it as a percentage;
  otherwise compute the ratio against `DesignCapacity`.
- Battery **temperature** is no longer cheaply available on iOS 17+ — it would
  require an `ioregistry` dump of `AppleSmartBattery` (tens of thousands of
  lines for one number). It is intentionally left as `—`.

## Scope boundaries

The following are **intentionally not built**, because iOS makes them
impossible from a desktop companion without a jailbreak — don't add them, and
push back on requests that assume them:

- Per-app cache cleanup (each app's cache lives inside its own sandbox).
- Crash log retrieval, full device backups, Wi-Fi pairing.

If a feature request looks like one of these, point the reporter at this
section and at the README.
