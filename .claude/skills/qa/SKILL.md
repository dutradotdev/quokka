---
name: qa
description: >
  Run quokka's real-device E2E layer (QA strategy layer 5): drive the real qk
  binary against connected iPhone/Android devices through tmux, apply
  deterministic verifiers, and write a report. Use when the user asks to "run
  /qa", "run the device QA suite", "test against the real device", or before a
  release. Optionally finishes with an advisory exploratory UX pass (layer 6).
---

# /qa — real-device E2E runner

You are the driver for layer 5 of the QA strategy
(`docs/superpowers/specs/2026-06-01-qa-strategy-design.md`). You run the real
binary against attached hardware and decide pass/fail **deterministically**.

## Hard rule

Pass/fail is decided ONLY by the deterministic predicates below (exit code,
regex over captured text, golden-frame compare, JSON validity). You do **not**
use your own judgment to pass or fail a case. Subjective UX judgment belongs to
the optional exploratory pass at the end, which is advisory and never a gate.

## Preconditions

1. `tmux` and `jq` installed. If `tmux` is missing, stop and tell the user
   `brew install tmux`.
2. A current build: run `cargo build` (debug `qk` at `target/debug/qk`).
3. At least one device attached. Detect platforms:
   `tests/llm/lib/drive.sh platforms` → prints `ios` and/or `android`.
   If empty, stop and tell the user to connect a device.

All mechanics go through `tests/llm/lib/drive.sh` (see its header for the
subcommands). Never reimplement the tmux dance inline.

## Procedure

Read every `tests/llm/*.yaml` (not `lib/`, not `golden/`, not `reports/`). For
each case, in file order:

1. **Check `requires`** against the detected platforms.
   - `any` → at least one device present.
   - `ios` / `android` → that platform present.
   - Not met → record **SKIP** with the reason. Never fail on absence.

2. **Run it.**
   - **No `interact` block (non-interactive):**
     `drive.sh run <command>`. The output ends with `__QA_EXIT__=<code>`.
     The captured text above that line is the output.
   - **Has `interact` (interactive TUI):**
     - `drive.sh start <session> <command>` (use a unique session like
       `qa_<id>`).
     - For each step: `wait_for: X` → `drive.sh wait <session> "X" <timeout>`;
       `keys: [..]` → `drive.sh keys <session> <k1> <k2> ...`.
     - After the steps, capture the frame: `drive.sh capture <session>`.
     - Send `quit_keys` (default `q`) and `drive.sh stop <session>`.
     - If `wait` times out, that is a **FAIL** (the TUI never reached the
       expected state).

3. **Apply every `verify` predicate** against the captured output/frame:
   | Predicate | Check |
   |-----------|-------|
   | `exit_code: N` | the `__QA_EXIT__` value equals N |
   | `output_matches: R` | regex R matches the captured output |
   | `frame_matches: R` | regex R matches the captured frame |
   | `frame_excludes: R` | regex R does **not** match the frame |
   | `json_valid: true` | the captured output parses as JSON (`jq . <<<output`) |
   | `golden_frame: name` | the frame equals `tests/llm/golden/<name>.txt` exactly |
   A case PASSES only if every predicate holds. A case with no predicates and a
   clean run is a PASS but flag it as "smoke-only".

4. **Always tear the session down** even on failure (`drive.sh stop`).

## Report

Write `tests/llm/reports/<YYYY-MM-DD-HHMM>.md` containing:
- a summary line: `N passed, M failed, K skipped` and the detected platforms;
- per case: id, status (PASS/FAIL/SKIP), which predicate failed (if any), and
  the captured frame in a fenced block (the trajectory, for debugging).

Then print the summary to the user. If anything failed, lead with the failures.

## Optional: exploratory UX pass (layer 6, advisory)

Only if the user asks for it, or it is pre-release. After the deterministic
suite, drive a connected device freely beyond the scripted cases — open each
TUI, trigger an error path (e.g. unplug mid-command if the user is present),
resize, navigate. Report **UX smells** ("this error doesn't say what to do",
"the dashboard wraps awkwardly at 80 cols") as observations, NOT pass/fail.
The user decides what becomes a ticket. Never gate a release on this pass.
