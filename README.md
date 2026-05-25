# quokka 🐹

> Inspect and tidy an iPhone connected to your Mac over USB — from the terminal.

[![CI](https://github.com/dutradotdev/quokka/actions/workflows/ci.yml/badge.svg)](https://github.com/dutradotdev/quokka/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

**quokka** is a small, fast CLI for the Mac that talks to an iPhone over a USB
cable. It shows you what's on the device — storage, battery health, installed
apps, the heaviest files — and lets you reclaim space without opening Finder or
iTunes.

It is built on the Rust
[`idevice`](https://github.com/jkcoxson/idevice) crate, and deliberately scoped
to **what iOS actually permits from outside the device sandbox** — no
jailbreak, no elevated privileges.

The binary is `quokka`; a shorter alias `qk` is installed alongside it. They
are byte-for-byte identical — `qk status` is the same as `quokka status`.

---

## Quick look

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

## What it does

| Command           | What it does                                                                                          |
| ----------------- | ----------------------------------------------------------------------------------------------------- |
| `quokka`          | Interactive launcher — the dashboard above plus a menu to jump into the commands below.               |
| `quokka status`   | Print the device dashboard once: name, model, iOS version, storage breakdown, battery health.         |
| `quokka info`     | Print the device's static identity in three labeled blocks (Device / System / Network). `--redact` masks serial, UDID, IMEIs, MACs for safe screenshots. |
| `quokka apps`     | Live, sortable list of installed user apps by size. Pick one to uninstall, or pass `--uninstall`.     |
| `quokka analyze`  | Walk the media folders, surface the heaviest files. Read-only by default; `--delete` opens a picker.  |
| `quokka media`    | Survey the camera roll & downloads: counts/sizes per kind, per-month breakdown, top 10 largest. Pass `-d` for likely-duplicate groups. |
| `quokka logs`     | Stream the device's syslog. TUI viewer with color-by-level, level filter (`l`), process filter (`p`), search (`/`), pause (space), save (`w`). Use `--no-tui` for plain stdout. |
| `quokka reboot`   | Soft reboot via `diagnostics_relay`. Asks to confirm; pass `--yes` to skip. |
| `quokka shutdown` | Power off via `diagnostics_relay`. Same confirmation posture. |
| `quokka devices`  | List every iPhone reachable through usbmuxd. Works without picking a specific device. |

### Examples

```sh
quokka status                          # one-shot device dashboard
qk info                                # full identity (Device + System + Network blocks)
qk info --redact                       # ...with serial / UDID / IMEI / MAC masked
qk apps                                # interactive app picker (sizes stream in live)
qk apps --uninstall com.example.app    # uninstall a specific app (asks to confirm)
qk apps --uninstall com.example.app --yes   # ...without the confirmation prompt
qk analyze                             # print the 20 heaviest media files
qk analyze --top 50                    # ...the heaviest 50 instead
qk analyze --delete                    # open the interactive deletion picker (needs a TTY)
qk media                               # camera-roll / downloads survey
qk media -d                            # ...plus likely-duplicate groups
qk logs                                # TUI log viewer (q to quit)
qk logs --no-tui --min-level warning   # plain-stream mode, warning+ only
qk logs --no-tui --process SpringBoard --save /tmp/sb.log   # filter + tee
qk reboot                              # asks to confirm
qk shutdown --yes                      # non-interactive
```

`analyze` is **read-only unless you pass `--delete`**, and `--delete` only
works from an interactive terminal — it refuses to run in a pipe or CI rather
than guessing what to delete. The picker has a min-size filter, substring
search, and an auto-mark menu that flags Live Photo motion videos, edited-photo
originals, old screenshots, and exact duplicates.

`reboot` and `shutdown` are destructive: in a non-TTY shell they require
`--yes` rather than guessing the answer. Same posture as `apps --uninstall`.

### Multiple iPhones connected

With more than one device plugged in, quokka needs to know which one you mean:

```sh
qk devices                              # list every device with name + model + UDID
qk --udid 00008130-0019... info         # target by UDID
QK_UDID=00008130-0019... qk apps        # ...or via env var for a whole shell session
qk info                                 # 2+ devices on a TTY: opens an interactive picker
qk info                                 # 2+ devices in a pipe/CI with no --udid: errors with a hint
```

`--udid` / `-d` is a global flag — it works on every subcommand. It also reads
from `QK_UDID` in the environment, so you can set it once per shell.

## What it does NOT do (and why)

iOS sandboxes third-party tooling tightly. Some things people ask about are
genuinely **impossible** from a desktop companion without jailbreaking:

- **Surgical app cache cleanup.** Per-app caches live inside each app's
  sandbox, reachable only via `house_arrest` and only for apps that opt into
  file sharing. There is no "clear cache" verb.
- **Crash log retrieval, full backups, Wi-Fi pairing.** Out of scope for the
  MVP. Possibly a v2.

If you need any of the above today, look at
[libimobiledevice](https://libimobiledevice.org/) or Apple Configurator.

## Requirements

- A **Mac** with the Xcode command line tools installed (`xcode-select --install`).
- A **recent Rust toolchain** — the repo pins `stable` via `rust-toolchain.toml`.
- An **iPhone connected via cable**. Wi-Fi pairing is out of scope.
- The device must be **trusted**: the first time you plug it in, unlock the
  iPhone and tap "Trust this computer".
- **iOS 17 or newer.** quokka does not use the `core_device_proxy` / RemoteXPC
  tunnel, so no elevated privileges are needed.

## Install

### Homebrew (recommended)

```sh
brew install dutradotdev/tap/quokka
```

Updates are automatic when you `brew upgrade`. Works on Apple Silicon and Intel.

### One-line installer (shell)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dutradotdev/quokka/releases/latest/download/quokka-cli-installer.sh | sh
```

Downloads the pre-built binary from GitHub Releases and drops `quokka` + `qk`
into `~/.cargo/bin`. Re-run the same command to update.

### Cargo

If you already have a Rust toolchain:

```sh
cargo install --git https://github.com/dutradotdev/quokka quokka-cli
# or, from a local clone:
cargo install --path .
```

All three methods install **two binaries** to your `PATH`: `quokka` and the
shorter alias `qk` — they are byte-for-byte identical.

## Development

```sh
cargo build
cargo test                  # unit + integration tests — no iPhone needed
cargo test --features e2e   # also runs tests that need a real iPhone over USB
cargo clippy --all-targets -- -D warnings
cargo fmt
```

- **[CONTRIBUTING.md](CONTRIBUTING.md)** — how to set up, test, and submit a change.
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — how quokka is put together and why.
- **[CHANGELOG.md](CHANGELOG.md)** — what changed between versions.

Every push and pull request is checked by [CI](.github/workflows/ci.yml):
formatting, clippy, and the full test suite.

## License

MIT — see [LICENSE](LICENSE).
