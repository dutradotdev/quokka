# Changelog

All notable changes to quokka are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
