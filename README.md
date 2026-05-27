# quokka 🐹

> Inspect and clean an iPhone connected to your Mac over USB, from the terminal.

[![CI](https://github.com/dutradotdev/quokka/actions/workflows/ci.yml/badge.svg)](https://github.com/dutradotdev/quokka/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

quokka is a Mac CLI for the iPhone you have plugged in. It reads storage,
battery, apps, media, identity, and the live syslog; it can also reclaim space
and reboot the phone. Everything runs through `usbmuxd` and lockdown-classic
services, so there's no jailbreak, no `core_device_proxy` tunnel, and no
elevated privileges.

It is built on the Rust [`idevice`](https://github.com/jkcoxson/idevice) crate
and works on iOS 17 and newer.

## ⚡ Quick start

```sh
brew install dutradotdev/tap/quokka-cli
qk
```

With no arguments, `qk` opens the interactive launcher. See [Install](#install)
below for `curl` and `cargo` alternatives.

## Demo

![quokka demo](docs/demo.gif)

## Dashboard preview

Run `quokka` with no arguments and you get a live device dashboard plus an
interactive menu:

```text
                   _    _             Lucas's iPhone (iPhone 14 Pro Max)
  __ _ _   _  ___ | | _| | ____ _      iOS 18.2 (build 22C152) · pt-BR · Sao Paulo
 / _` | | | |/ _ \| |/ / |/ / _` |
| (_| | |_| | (_) |   <|   < (_| |     Storage   ████░░░░░░   42%  107.5 GB / 256.0 GB
 \__, |\__,_|\___/|_|\_\_|\_\__,_|               ├─ System   12.4 GB
    |_|                                          ├─ Data     95.1 GB
                                                 └─ Free    148.5 GB

                                       Battery   level   87% ⚡ 20W USB-C
                                                 health  91%
                                                 cycles  142
                                                 temp    27.4 °C

  47 apps · last backup 2023 · paired since 2021
```

## Commands

| Command           | What it does                                                                                                                    |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `quokka`          | Interactive launcher. Dashboard plus a menu to jump into any command below.                                                     |
| `quokka status`   | Print the device dashboard once.                                                                                                |
| `quokka info`     | Static identity in three blocks (Device / System / Network). `--redact` masks serial, UDID, IMEI, and MAC for safe screenshots. |
| `quokka apps`     | Picker of installed user apps by size. `--uninstall <bundle-id>` removes one directly.                                          |
| `quokka analyze`  | Walk media folders and surface the heaviest files. Read-only by default; `--delete` opens a picker.                             |
| `quokka media`    | Survey camera roll and downloads. Per-kind counts, per-month breakdown, top-10 largest. `-d` adds likely-duplicate groups.      |
| `quokka logs`     | Stream the device's syslog in a TUI viewer. Filter by level or process, search, pause, save. `--no-tui` streams plain stdout.   |
| `quokka reboot`   | Soft reboot via `diagnostics_relay`. Confirms by default; `--yes` skips.                                                        |
| `quokka shutdown` | Power off via `diagnostics_relay`. Confirms by default; `--yes` skips.                                                          |
| `quokka devices`  | List every iPhone reachable through `usbmuxd`. Does not select one.                                                             |

## Examples

```sh
qk status                              # one-shot device dashboard
qk info                                # full identity (Device + System + Network)
qk info --redact                       # ...with serial / UDID / IMEI / MAC masked

qk apps                                # interactive app picker (sizes stream in live)
qk apps --uninstall com.example.app    # uninstall a specific app (asks to confirm)
qk apps --uninstall com.example.app --yes   # ...without the confirmation prompt

qk analyze                             # print the 20 heaviest media files
qk analyze --top 50                    # ...the heaviest 50 instead
qk analyze --delete                    # interactive deletion picker (needs a TTY)

qk media                               # camera roll / downloads survey
qk media -d                            # ...plus likely-duplicate groups

qk logs                                # TUI log viewer (q to quit)
qk logs --no-tui --min-level warning   # plain-stream mode, warning+ only
qk logs --no-tui --process SpringBoard --save /tmp/sb.log  # filter + tee

qk reboot                              # asks to confirm
qk shutdown --yes                      # non-interactive
```

`analyze` is read-only unless you pass `--delete`. `--delete` only works from
an interactive terminal; in a pipe or CI it refuses to run rather than guess
what to delete. The picker has a min-size filter, substring search, and an
auto-mark menu that flags Live Photo motion videos, edited-photo originals,
old screenshots, and exact duplicates.

`reboot` and `shutdown` are destructive. In a non-TTY shell they require
`--yes` rather than guess the answer, same as `apps --uninstall`.

## Multiple iPhones connected

If you have more than one device plugged in, pick which one quokka talks to:

```sh
qk devices                              # list every device with name + model + UDID
qk --udid 00008130-0019... info         # target by UDID
QK_UDID=00008130-0019... qk apps        # ...or via env var for a whole shell session

qk info                                 # 2+ devices on a TTY: opens an interactive picker
qk info                                 # 2+ devices in a pipe/CI without --udid: errors with a hint
```

`--udid` is a global flag that works on every subcommand (long-only — there is
no `-d` short form because `qk media -d` already means `--find-duplicates`). It
also reads from `QK_UDID` in the environment, so you can set it once per shell.

## Requirements

- A Mac with the Xcode command line tools (`xcode-select --install`).
- An iPhone connected via cable. Wi-Fi pairing is out of scope.
- The device must be trusted. The first time you plug it in, unlock the iPhone
  and tap _Trust this computer_.
- iOS 17 or newer.

## Install

### Homebrew (recommended)

```sh
brew install dutradotdev/tap/quokka
```

Works on Apple Silicon and Intel. `brew upgrade` keeps you on the latest
release.

### One-line installer (no Homebrew)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/dutradotdev/quokka/releases/latest/download/quokka-cli-installer.sh | sh
```

Pulls the prebuilt binary from GitHub Releases into `~/.cargo/bin`. Re-run the
same command to update.

### Cargo (build from source)

If you already have a Rust toolchain:

```sh
cargo install --git https://github.com/dutradotdev/quokka quokka-cli
# or, from a local clone:
cargo install --path .
```

Slower because it compiles locally. Use this when you want to track `main`
between tagged releases.

Every install method drops two binaries on your `PATH`: `quokka` and the
shorter `qk`. They share the same code, so `qk status` and `quokka status` do
the same thing.

## What it does not do (and why)

iOS sandboxes third-party tooling tightly. Some things people ask about cannot
be done from a desktop companion without jailbreaking:

- Per-app cache cleanup across the board. Caches live inside each app's
  sandbox, only reachable via `house_arrest` and only for apps that opt into
  file sharing. There is no "clear cache" verb that works everywhere.
- Crash log retrieval, full backups, Wi-Fi pairing. Out of scope for the MVP.
  Possibly a v2.

For any of these today, [libimobiledevice](https://libimobiledevice.org/) or
Apple Configurator are the alternatives.

## Development

```sh
cargo build
cargo test                  # unit + integration, no iPhone needed
cargo test --features e2e   # adds tests that need a real iPhone over USB
cargo clippy --all-targets -- -D warnings
cargo fmt
```

- [CONTRIBUTING.md](CONTRIBUTING.md): how to set up, test, and submit a change.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how quokka is put together and why.
- [CHANGELOG.md](CHANGELOG.md): what changed between versions.
- [SECURITY.md](SECURITY.md): how to report a vulnerability.

Every push and pull request runs through [CI](.github/workflows/ci.yml):
formatting, clippy, the full test suite, `cargo audit`, and `cargo deny`.

## License

MIT. See [LICENSE](LICENSE).
