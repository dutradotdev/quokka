# Contributing to quokka

Thanks for considering a contribution! This guide covers the practical bits.
For the *why* behind the design, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
first — it explains the one structural rule that keeps quokka testable.

By participating you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to help

- **Report a bug** or **request a feature** — use the issue templates. Please
  check the README's "What it does NOT do" section first; some requests are
  intentionally out of scope because iOS makes them impossible.
- **Pick up an issue** — anything labelled `good first issue` is a self-contained
  starting point. Comment on it so we don't duplicate work.
- **Improve docs** — corrections to the README, this guide, or `ARCHITECTURE.md`
  are always welcome and review fast.

## Project layout

```text
src/
├── lib.rs              # run() — shared entry point: clap parsing + dispatch
├── bin/
│   ├── quokka.rs       # primary binary — a thin shim over run()
│   └── qk.rs           # short alias — intentionally identical to quokka.rs
├── device/
│   ├── mod.rs          # the Device trait, FakeDevice, and the real impl
│   └── model_names.rs  # ProductType → marketing-name lookup
├── ui.rs               # terminal output helpers shared by every command
└── commands/
    ├── mod.rs
    ├── status.rs       # `quokka status`
    ├── apps.rs         # `quokka apps` — list / uninstall (ratatui picker)
    ├── analyze.rs      # `quokka analyze` — heaviest files (ratatui picker)
    ├── dashboard.rs    # pure dashboard renderer (used by status + launcher)
    └── menu.rs         # interactive launcher shown for a bare `quokka`

tests/
├── integration.rs      # commands exercised against a fake Device — no iPhone
├── e2e_smoke.rs        # real device, behind the `e2e` feature
└── e2e_enrich_bench.rs # perf sweep, behind `e2e`, #[ignore] by default

docs/
├── ARCHITECTURE.md     # how it all fits together, and the design rules
└── superpowers/specs/  # design specs for individual commands
```

**The `Device` trait in `src/device/mod.rs` is the seam that makes quokka
testable.** The real implementation talks to the `idevice` crate; tests inject
a `FakeDevice`. Please keep that seam clean — **no `idevice` type may leak
through it.** See [ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full reasoning.

## Building

```sh
cargo build
```

The repo pins the `stable` toolchain via `rust-toolchain.toml` — several
dependencies require `edition2024`. Don't change the pin without checking.

## Running tests

quokka has three test layers. **The first two are the regression net** — CI
runs them on every push and pull request, so a change that breaks the current
behaviour fails before it can merge.

1. **Unit tests** (`#[cfg(test)]` modules next to the code) cover pure logic:
   byte formatting, sorting, the dashboard renderer, battery-health heuristics.
   They never touch hardware.
2. **Integration tests** in `tests/integration.rs` exercise whole commands
   against a `FakeDevice`, so they run without an iPhone connected.
3. **End-to-end tests** live behind the `e2e` feature flag. They drive the real
   `idevice` backend against a physical iPhone over USB and are **never run in
   CI** — run them yourself when touching `device/mod.rs`'s real path.

```sh
cargo test                  # layers 1 + 2 — what CI runs
cargo test --features e2e   # layers 1 + 2 + 3 — needs an iPhone, plugged in and trusted
cargo test --test integration            # just the integration file
cargo test -- format_bytes                # filter by test-name substring
```

**When you add or change logic, write its unit and/or integration tests in the
same change** — not in a follow-up. A pull request that changes behaviour
without a test that would have caught the old behaviour will be asked for one.
This is how we keep regressions out: the test you add today is what fails the
day someone else's change breaks your feature.

## Style and checks

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
```

Both are enforced by CI. There is also a local `PostToolUse` hook in
`.claude/settings.json` that runs `fmt` + `clippy` + `test` automatically after
Rust edits made through Claude Code — but always run them yourself before
pushing if you edit by hand.

## Commits and pull requests

- Keep commits focused; messages should explain **why**, not just **what**.
- Open pull requests against `main`. Small PRs review faster than big ones.
- Fill in the pull request template, including the testing checklist.
- **If you change any command's output, include a transcript or screenshot**
  in the PR description — UX is reviewed, not just code.
- CI must be green before a PR is merged.

## Bumping the `idevice` dependency

The `idevice` crate ships breaking changes at nearly every point release until
it reaches 0.2.0, so `Cargo.toml` pins it with `=`. When you bump it:

1. Re-do the API research first — the `tools/src/` directory of the
   [upstream repo](https://github.com/jkcoxson/idevice) is the canonical
   example of current usage.
2. Update only the `mod real` submodule in `src/device/mod.rs` to match. The
   `Device` trait and everything above it should not need to change — if they
   do, the seam has sprung a leak; fix that instead.
3. Run `cargo test --features e2e` against a real device before opening the PR.
