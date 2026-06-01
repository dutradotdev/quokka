# QA Strategy Design

**Date:** 2026-06-01
**Status:** Approved (design), pending implementation plan
**Goal:** Let a human + AI iterate on this codebase safely, trusting a green
CI to mean "no regression" — without manually re-testing everything each
release.

## Guiding principle

Safety to iterate comes from the **deterministic pyramid** (layers 0–4): it
gates *every commit* in CI, runs fast, costs nothing, and is reproducible.
The real-device and exploratory tiers (layers 5–6) sit on top, run
*before a release*, and are **never a CI gate** — they need a physical device
attached and (for layer 6) are non-deterministic.

A corollary that shapes every decision below: an **LLM-as-judge pass/fail
gate is explicitly rejected**. A non-deterministic gate is worse than no gate,
because flakiness trains the maintainer to ignore it. The LLM *drives* the
terminal (proven feasible — see Appendix A); it does not *decide* pass/fail for
anything that can be checked deterministically.

## The layers

| # | Layer | Owns | Runs | Tooling |
|---|-------|------|------|---------|
| 0 | Static gates *(exists)* | fmt, clippy `-D warnings`, `cargo audit`, `cargo deny` | CI, every commit | — |
| 1 | Unit | pure logic | CI | `cargo test` |
| 2 | Property *(new)* | parsers never panic on arbitrary input | CI | `proptest` |
| 3 | Snapshot *(new)* | deterministic rendered output | CI | `insta` |
| 4 | Integration *(expand)* | correctness of every feature × flag via `FakeDevice` | CI | `cargo test` |
| 5 | Real-device E2E via tmux *(new)* | interactive TUI flows + real-hardware sanity | local, pre-release | `/qa` skill + tmux |
| 6 | Exploratory LLM *(new, advisory)* | UX smell, "try to break it" | on demand | Claude drives device |

### Layer 1 — Unit

Backfill the pure-logic modules that currently have **zero** unit tests:
`src/commands/capture/parser.rs`, `src/commands/capture/hosts.rs`,
`src/commands/capture/pcap_io.rs`, `src/commands/capture/style.rs`, and
`src/commands/status.rs`. The packet parser (356 LOC, untested) is the single
biggest gap.

**Policy:** every bug fix ships with a regression test reproducing the bug, in
the same change. (Formalizes the existing CLAUDE.md "tests in the same change"
rule and extends it to bug fixes specifically.)

### Layer 2 — Property (`proptest`)

Target the parsers: Android `dumpsys` / `diskstats` parsing in
`src/device/android.rs`, and the packet parser in
`src/commands/capture/parser.rs`. Invariant under test: **arbitrary input never
panics**; tolerant parsing degrades to `None`/empty rather than crashing. This
is the executable form of the project's "tolerant parsing, not per-OEM
branching" rule — OEM output variation is fuzzed, not enumerated by hand.

### Layer 3 — Snapshot (`insta`)

Golden snapshots for deterministic rendered output:

- dashboard render (`src/commands/dashboard.rs`)
- `card` SVG output (`src/commands/card/render.rs`) — already a pure function
  of `CardData` + a fixed `now`, so output is byte-identical across runs
- `ui.rs` formatters (byte sizes, bars, blocks)
- `--json` output for `info` / `devices`

Reviewed with `cargo insta review`. This is the highest-ROI addition: it turns
"did the AI subtly break the layout?" into a one-line reviewable diff. The
`card` **PNG** is binary and out of scope for snapshotting — its visual
correctness is covered transitively by the SVG snapshot.

### Layer 4 — Integration (`FakeDevice`, expand)

Owns **feature × flag completeness** — the literal "e2e for every feature/
filter" requirement, but run fast and gated against `FakeDevice` rather than
hardware. Cover every command and flag combination: `status`, `apps`,
`analyze` (`--delete`, `--top N`, dry-run default), `media`, `card`, `power`,
`logs`, `devices`, and `--json` variants. Extends the existing ~50 integration
tests in `tests/integration.rs`.

### Layer 5 — Real-device E2E via tmux (`/qa`)

Covers the surface the deterministic pyramid **cannot** reach: interactive
ratatui TUIs (which `QK_NON_INTERACTIVE` skips entirely in CI) and real-hardware
sanity. Verification is **deterministic** — golden frames + shell assertions +
exit code. No LLM judge in the pass/fail path.

**Test-case format** — YAML in `tests/llm/`, one file per area:

```yaml
# tests/llm/apps.yaml
- id: apps-live-size-picker
  command: qk apps
  requires: [ios]            # skipped with a warning if no iPhone attached
  interact:                  # keystrokes for TUI flow (omit = non-interactive)
    - wait_for: "Loading apps"
    - keys: ["Down", "Down", "Space"]
    - wait_for: "selected"
    - keys: ["Enter"]
  verify:                    # deterministic — decides pass/fail
    - output_contains: "MB"
    - exit_code: 0
    - golden_frame: apps-picker   # optional: byte-compare vs tests/llm/golden/
```

**The `/qa` skill** (project skill under `.claude/`):

1. Discovers `tests/llm/*.yaml`.
2. Detects connected devices across both transports (usbmuxd + adb).
3. Skips any case whose `requires` platform is absent — **with a warning, never
   a failure**.
4. Runs each remaining case through the real binary inside `tmux` at a fixed
   size (`tmux new-session -x 200 -y 50`) for deterministic layout.
   - `interact.keys` → `tmux send-keys`
   - `wait_for` → poll `tmux capture-pane` until the text appears, with timeout
   - `verify` → grep on captured text + exit code + optional golden-frame
     byte-compare against `tests/llm/golden/`
5. Writes `tests/llm/reports/<date>.md`: pass/fail per case, captured frames
   (the trajectory, for debugging).

Not run in CI (needs a physical device). It is a pre-release checklist item.

**Engineering risk to handle:** streaming output (e.g. `apps` Phase 2 live
sizes). Every `interact` step between a key and a capture must go through the
`wait_for` polling loop, or the suite is flaky.

### Layer 6 — Exploratory LLM pass (advisory)

Separate from layer 5 and **never pass/fail**. On demand / pre-release, Claude
drives a connected device, pokes beyond the scripted cases, and returns a
report of UX smells ("this error doesn't say what to do"). Output is advisory;
the maintainer decides what becomes a ticket. This is the only place an LLM
judges quality, and it judges nothing that gates a build.

## Coverage gate

Add `cargo-llvm-cov` as a new CI job in **ratchet mode**: a PR may not regress
coverage versus the base branch, and new code must come with tests. No
one-shot effort to cover the legacy tree. This is what prevents AI-authored
code from landing untested without the maintainer noticing.

## CI changes

- **New job:** coverage ratchet (`cargo-llvm-cov`).
- Keep all existing jobs (fmt, clippy, test, doctest, audit, deny).
- `e2e` / `e2e-android` stay compile-checked only, never executed (unchanged).
- Layers 5 and 6 do **not** run in CI; they are documented release-checklist
  steps.

## New dependencies

- `dev-dependencies`: `insta`, `proptest`.
- CI tooling: `cargo-llvm-cov`.
- `tmux` (already installed locally) documented as a `/qa` prerequisite.

## "Safe AI iteration" workflow (the point of all this)

- **Bug** → regression test in the same commit.
- **Feature** → integration test (correctness) + snapshot if it renders + a
  tmux E2E case if it has a TUI or real-hardware aspect.
- **Pre-release** → run `/qa` against an iPhone and an Android; optionally run
  the exploratory pass.
- **Result:** a green CI is trustworthy without manual re-testing, because the
  deterministic pyramid gates every commit.

## Out of scope (YAGNI)

- ❌ LLM-as-judge as a CI/release gate.
- ❌ A custom pty harness — tmux suffices; graduate to one only if it hurts.
- ❌ An off-the-shelf eval framework (promptfoo / DeepEval) — Python toolchain
  in a pure-Rust repo.
- ❌ Real-device tests in CI.
- ❌ Pixel/visual diffing of the `card` PNG (covered by the SVG snapshot).

## Appendix A — tmux feasibility (verified 2026-06-01)

Driving the terminal and asserting on UI was proven live against a connected
Android (Redmi Note 9S), through the Bash tool alone:

- **Non-interactive:** `tmux send-keys 'qk status'` then `tmux capture-pane -p`
  returned the full rendered dashboard (storage bar, battery, app count) plus
  exit code — directly assertable.
- **Interactive TUI:** launched the ratatui sidebar, captured the rendered
  Actions menu frame (including the `↑↓ select · Tab focus · Enter run · r
  rescan · q quit` footer), sent `q`, and captured the frame after — proving
  keystroke injection and frame capture both work.

**Known limits (informing the design):** `capture-pane` yields the text grid,
not pixels — so the suite asserts layout/content/state transitions, not color
or fine animation (both already gate on TTY and aren't worth asserting). A
fixed terminal size is mandatory for stable frames. The `card` PNG cannot be
seen via tmux (binary), hence the SVG-snapshot decision above.
