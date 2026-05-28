# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

The contributor-facing docs cover the same ground for humans: `README.md`
(what quokka is), `CONTRIBUTING.md` (how to submit a change), and
`docs/ARCHITECTURE.md` (how it fits together). Keep them in sync with this file.

## Build & test

```sh
cargo build
cargo test                  # unit + integration tests, no iPhone needed
cargo test --features e2e   # adds tests that require a real iPhone over USB
cargo run --bin quokka -- status
cargo run --bin qk -- status        # short alias, same binary content
cargo test --test <name>            # run a single integration test file
cargo test -- <substring>           # filter by test name substring
```

A `PostToolUse` hook in `.claude/settings.json` runs `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` automatically after any Edit/Write/MultiEdit that touches `src/**/*.rs`, `tests/**/*.rs`, or `Cargo.toml`. Failures come back as blocking hook output — fix them before continuing. You normally do **not** need to invoke fmt / clippy / test manually after editing Rust files.

The same three checks run in CI (`.github/workflows/ci.yml`) on every push and pull request to `main`, so a regression fails before it can merge. The `e2e` tests are compile-checked in CI but never executed there.

The repo pins `stable` via `rust-toolchain.toml` (the system default may be older). Do not change this without checking — several deps require `edition2024`.

## Architecture

**The `Device` trait in `src/device/mod.rs` is the seam that makes the whole project testable.** Every iPhone operation goes through this trait. The real implementation lives in a private `mod real` submodule that talks to the [`idevice`](https://github.com/jkcoxson/idevice) crate; tests use the in-module `FakeDevice`. **No `idevice` type may be exposed through the public surface of `device/mod.rs`** — the `idevice` crate ships breaking changes at every point release until 0.2.0, and the seam exists to absorb them.

When adding a capability (e.g. battery, app list, AFC walk), add a method to the `Device` trait and a corresponding implementation in `mod real`. Commands consume the trait, not `idevice` directly.

The dependency is pinned with `=0.1.x` for the same reason. When bumping, re-do the API research before touching code — the `tools/src/` directory of the upstream repo is the canonical example of current usage.

### Two binaries, one entry point

`src/lib.rs` exports `run()`. Both `src/bin/quokka.rs` and `src/bin/qk.rs` are thin shims that call it — they are intentionally identical. `qk` is the short alias. Integration tests in `tests/` go through the lib (not by spawning the binary).

### Command structure

`run()` parses with clap, calls `device::connect()` once, then dispatches to `commands::{status,apps,analyze,menu}::run(&*device, ...)` — `menu` is the interactive launcher for a bare `quokka` on a TTY. The dispatch lives in `src/lib.rs`; per-command logic in `src/commands/`. `commands::dashboard` is not a dispatch target — it is the pure dashboard renderer reused by `status` and `menu`. UI helpers (byte formatting, blocks, progress bars) are centralized in `src/ui.rs` so every command renders consistently.

### Test layers

1. **Unit tests** (`#[cfg(test)]` next to the code) — pure logic, no hardware, no fake.
2. **Integration tests** in `tests/integration.rs` — exercise commands against `FakeDevice`. The fake is what makes these possible without an iPhone.
3. **E2E tests** in `tests/e2e_*.rs` behind `--features e2e` — drive the real `idevice` backend (`RealDevice`) against a physical iPhone over USB, through the same library entry points as the integration tests. **Never run in CI** — CI only compile-checks them.

Always write unit and integration tests in the same change as the code, not after.

### iOS / `idevice` notes

- The MVP uses **only** lockdown-classic services (`lockdown`, `diagnostics_relay`, `afc`, `installation_proxy`) over usbmuxd. No `core_device_proxy` / RemoteXPC tunnel — that would be required for DVT/DTServiceHub services on iOS 17+, which the MVP does not use.
- The crate requires a crypto backend; we enable `aws-lc` (upstream default; `ring` is the alternative).
- Wi-Fi pairing is intentionally out of scope. `RealDevice::connect` prefers a USB device when both are present.

**iOS 17.4+ quirks for battery info** (already handled in `device/mod.rs`):
- `diagnostics_relay.mobilegestalt(...)` returns `Status = "MobileGestaltDeprecated"` for every key. Don't use it. ([libimobiledevice#1542](https://github.com/libimobiledevice/libimobiledevice/issues/1542)).
- Battery level comes from lockdown domain `com.apple.mobile.battery`, key `BatteryCurrentCapacity`.
- `diagnostics_relay.gasguage()` still works but the response is **wrapped one level deep**: `{"GasGauge": {"CycleCount": ..., "FullChargeCapacity": ..., "DesignCapacity": ...}}`. Unwrap the inner dict first.
- On iOS 17+, `FullChargeCapacity` is reported as a percentage of design (matches iOS Settings → Battery Health → Maximum Capacity). On older iOS it was in mAh. Heuristic: if FCC ≤ 100, treat as percentage; otherwise compute FCC/DC×100.
- Battery temperature is no longer in `gasguage` on iOS 17+. Only available via `ioregistry(AppleSmartBattery)`, which returns tens of thousands of lines for one number — not worth the cost in the MVP. Temperature renders as `—`.

## UX principles

- Colour and animation already gate on TTY + `NO_COLOR` at the stream level (`anstream` + `owo-colors`); `indicatif::ProgressDrawTarget::stderr()` hides spinners on non-TTY. Don't reinvent these checks.
- Operations >1s show a spinner or progress indicator.
- Errors must say what happened **and** what to do. No raw stack traces.
- Destructive actions require explicit confirmation. Without a TTY, abort rather than assume "yes" unless an explicit non-interactive flag is provided.
- `--dry-run` is the default where it makes sense (`analyze` doesn't delete unless `--delete` is set).

## What's out of scope (don't add)

- Per-app cache cleanup (impossible without jailbreak)
- Crash logs, full backups, Wi-Fi pairing

`qk apps` and `qk analyze --delete` open a ratatui interactive picker — `apps`
streams live size updates as Phase 2 enrichment completes. `status`, read-only
`analyze`, and the bare-`quokka` launcher print plain output blocks (the
launcher's menu is a `dialoguer` select, not ratatui).

`qk card` renders an SVG → PNG (1080×1080) via `resvg`/`usvg`/`tiny-skia` with
JetBrains Mono embedded via `include_bytes!`. The renderer is a pure function
of a pre-projected `CardData` (no `SystemTime::now` in the layout layer) so
SVG output is byte-identical given identical device state plus a fixed `now`.
Reads from lockdown domains already in use by `status` / `info` / `apps`
(`com.apple.disk_usage` adds `CameraUsage` / `MobileApplicationUsage` /
`OtherUsage`; `installation_proxy.browse` adds `LSInstallDate`).

If a request looks like one of the above, push back and reference this section.
