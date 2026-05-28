# Changelog

All notable changes to quokka are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `qk card` — render a 1080×1080 PNG snapshot of the connected iPhone with
  storage breakdown by category (Photos / Apps / Other), chip name, battery
  health, install age, last-backup age, and up to 3 earned badges from a
  15-entry catalog. Saves to `~/Desktop` and opens in Preview by default;
  prints a pre-filled Twitter intent URL for sharing. `--redact` masks the
  build number, oldest-app name, exact dates, and bucketed backup age.
- New `DeviceStatus` fields populated by `RealDevice::status()`: `chip_name`
  (from lockdown `HardwarePlatform`), `storage_breakdown` (from
  `com.apple.disk_usage`), `oldest_app` (from `installation_proxy`
  `LSInstallDate`), `jailbreak_detected` (bundle-ID match against a curated
  list), and `is_beta_build` (regex on the iOS build string).

### Fixed

- Crash in `qk logs` search highlighting when the message contained
  characters whose lowercase form has a different byte length than the
  uppercase form (e.g. Turkish `İ`).
- `qk reboot` / `qk shutdown` checked the wrong stream for a TTY (stdout
  instead of stdin), which could falsely refuse to confirm when stdout was
  piped but stdin was interactive.
- `qk apps` would abort the whole app-size enrichment on the first failing
  batch instead of treating later batches as best-effort.
- `--udid <wifi-udid>` failed when another USB device was also connected;
  the USB-only filter is now skipped when the user names a UDID explicitly.

### Changed

- `analyze --delete` confirmation now defaults to "no" so a bare Enter no
  longer deletes files.
- `analyze` top-N selection switched to a fixed-size min-heap (O(N log K))
  for cheaper scans on devices with very large media libraries.
- `qk reboot` / `qk shutdown` no longer make an extra lockdown round-trip
  when `--yes` is set.

## [0.1.0] - MVP

The MVP. quokka talks to an iPhone connected over USB to a Mac, using only
lockdown-classic services — no jailbreak, no elevated privileges.

### Added

- `quokka status` — device dashboard: name, model, iOS version and build,
  storage breakdown (system / data / free), and battery health.
- `quokka apps` — list installed user apps by size, with a `ratatui` picker
  whose sizes refresh live as enrichment streams in. Supports `--uninstall`
  (with `--yes` to skip the confirmation prompt).
- `quokka analyze` — walk the media folders (`/DCIM`, `/Downloads`,
  `/Recordings`, `/Books`) and surface the heaviest files. Read-only by
  default; `--delete` opens an interactive deletion picker on a TTY.
- Interactive launcher when `quokka` / `qk` is run with no subcommand.
- `qk` short-alias binary, identical to `quokka`.
- Unit, integration, and (USB-only) end-to-end test layers.
- CI workflow: formatting, clippy, and the full test suite on every push and
  pull request.

[Unreleased]: https://github.com/dutradotdev/quokka/commits/main
