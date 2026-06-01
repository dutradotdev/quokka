# `/qa` — real-device E2E layer

This directory holds **layer 5** of quokka's QA strategy: end-to-end cases that
run the real `qk` binary against a physically connected iPhone or Android,
driven through `tmux`. It covers the surface the deterministic pyramid (unit /
property / snapshot / integration, all gated in CI) **cannot** reach:
interactive ratatui TUIs and real-hardware behavior.

It is **not** run in CI — it needs a device attached. It is a pre-release
checklist step. See the design at
`docs/superpowers/specs/2026-06-01-qa-strategy-design.md`.

## Key principle: deterministic verification, no LLM judge

Pass/fail is decided by deterministic checks only — exit codes, regex over
captured text, and golden-frame comparison. An LLM never decides pass/fail
here. (Semantic "does this read well?" judgment is layer 6, the advisory
exploratory pass — separate, and never a gate.)

## How to run

The intended driver is **Claude Code** via the `/qa` skill — it reads these
YAML cases, detects connected devices, drives each through `tmux`, applies the
verifiers, and writes a report to `reports/`. The skill lives at
`.claude/skills/qa/SKILL.md`.

Prerequisites: `tmux` (`brew install tmux`), `jq`, and a built binary
(`cargo build`). The mechanics live in `lib/drive.sh`, which Claude (or a
future standalone runner) calls:

```sh
tests/llm/lib/drive.sh platforms              # which platforms are attached
tests/llm/lib/drive.sh run qk status          # non-interactive: prints frame + __QA_EXIT__=<code>
tests/llm/lib/drive.sh start qa_apps qk apps  # interactive: launch detached
tests/llm/lib/drive.sh wait qa_apps "space toggle" 10
tests/llm/lib/drive.sh capture qa_apps        # the rendered frame as text
tests/llm/lib/drive.sh keys qa_apps Down Down q
tests/llm/lib/drive.sh stop qa_apps
```

`drive.sh` owns only mechanics; it makes no pass/fail decision.

## Case format

One YAML file per command area. Each case:

```yaml
- id: apps-picker-opens-lists-and-quits   # unique, kebab-case
  description: human-readable intent
  command: qk apps                        # the command to run (qk/quokka prefix optional)
  requires: [any]                         # [any] | [ios] | [android]; missing → skip with a warning
  interact:                               # OMIT for non-interactive commands
    - wait_for: "space toggle"            # poll the frame until this regex appears
    - keys: ["Down", "Down"]             # tmux key names: Down/Up/Enter/Space/Tab/Escape, or literal text
  verify:                                 # deterministic predicates (see below)
    - frame_matches: "bundle"
  quit_keys: ["q"]                        # keys to exit an interactive TUI cleanly
```

### `requires`

- `any` — at least one device of any platform is attached.
- `ios` / `android` — that platform specifically.
- If the requirement is not met, the case is **skipped with a warning**, never
  failed. A run on Android-only hardware skips iOS-only cases and vice versa.

### `verify` predicates

| Predicate | Meaning |
|-----------|---------|
| `exit_code: N` | process exited with code N (non-interactive `run` only) |
| `output_matches: "regex"` | regex matches the captured output |
| `frame_matches: "regex"` | regex matches the frame captured at the end of `interact` |
| `frame_excludes: "regex"` | regex must **not** match the captured frame |
| `json_valid: true` | the captured output parses as JSON |
| `golden_frame: name` | the captured frame equals `golden/<name>.txt` byte-for-byte |

Golden frames live in `golden/`. To (re)capture one, run the case and save the
captured frame to `golden/<name>.txt`; review the diff like any snapshot.

## What is intentionally excluded

- Destructive actions (`qk analyze --delete`, `qk power reboot/shutdown`) — they
  mutate the device. Exercise by hand or behind an explicit opt-in tier.
- Color/animation assertions — `tmux capture-pane` yields the text grid, not
  pixels. Cases assert layout, content, and state transitions.
- The `qk card` PNG — binary output `tmux` can't see; its visual correctness is
  covered by the SVG snapshot in the deterministic layer.
