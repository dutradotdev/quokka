# `qk reboot` and `qk shutdown` — design

**Status:** draft, awaiting approval
**Date:** 2026-05-25
**Owner:** Lucas Dutra

## Summary

Two destructive companion commands that drive the connected iPhone's power state through `diagnostics_relay`. `qk reboot` restarts the device; `qk shutdown` powers it off. Both gate behind explicit confirmation (or `--yes`) and exit immediately after the request is acknowledged — they do not wait for the device to come back.

One spec covers both because they are byte-for-byte symmetric: same service, same UX, same error model. Splitting into two specs would duplicate 90% of the text.

## Motivation

A connected iPhone sometimes needs a reboot — to clear stuck services, to apply a setting that requires a restart, to recover from a wedged state. Today that means pressing physical buttons. With `diagnostics_relay` already in use for battery reads, adding software-driven reboot and shutdown is a small step that removes friction for the recurring case.

## Scope

### In scope

- `qk reboot` and `qk shutdown` as destructive commands gated by confirmation.
- `--yes` / `-y` flag to skip the prompt (required for non-TTY invocation).
- Menu entries for both, relying on the same confirmation prompt as the CLI.

### Out of scope

- `qk sleep`. Diagnostics relay supports it, but the use case is thin (screen-lock is one tap on the device). Listed in future work.
- "Wait until device returns" behavior. After the request is acknowledged, the program exits. Polling for reconnection is a future optimization.
- `--cancel` / `--abort-pending`. `diagnostics_relay` does not expose a cancel path, and the device acts immediately on the request.
- Reading or printing pre-reboot state (uptime, last reboot reason). Out of scope and not reachable through lockdown anyway.

## CLI surface

### Synopsis

```
quokka reboot   [--yes | -y]
quokka shutdown [--yes | -y]
```

- `--yes` / `-y`: skip the confirmation prompt. Required for non-TTY invocations.

### Behavior

`qk reboot` flow:

1. User runs `qk reboot`.
2. Spinner: `Connecting to diagnostics service…`
3. Device name is read (via `device.status()`, already cheap) and the confirmation prompt opens:
   > Reboot **Lucas's iPhone** (iPhone 15 Pro Max)? [y/N]
4. Default is **No**. On `n` / Enter / Ctrl-C → print `Aborted.` and exit `Ok`.
5. On `y` → spinner: `Sending restart…`
6. After the diagnostics_relay call returns, print one line and exit:
   > ✓ Restart requested. The device will disconnect shortly.

`qk shutdown` is identical with `Shutdown`, `Sending shutdown…`, and `Shutdown requested. The device will power off shortly.` as the only string differences. Verbs in flags, prompts, and errors swap accordingly.

#### Non-TTY behavior

Without a TTY, the confirmation prompt cannot run. Without `--yes`, the command aborts with:

> error: refusing to run a destructive action without confirmation.
> Re-run with `--yes` or run from an interactive terminal.

(Same posture as `apps --uninstall`.)

## Architecture

### Device trait changes

Two new methods:

```rust
#[async_trait]
pub trait Device: Send + Sync {
    // ... existing methods ...

    /// Request that the device restart. Returns once `diagnostics_relay`
    /// acknowledges the request — the actual restart happens asynchronously
    /// on the device.
    async fn reboot(&self) -> Result<()>;

    /// Request that the device power off. Returns once `diagnostics_relay`
    /// acknowledges the request — the actual shutdown happens asynchronously.
    async fn shutdown(&self) -> Result<()>;
}
```

Two methods rather than a single `power(action: PowerAction)`: the trait stays close to the underlying capability, and the `idevice` calls are independent. The `PowerAction` enum lives one layer up, inside `commands::power`.

The seam rule holds: no `diagnostics_relay` types leak out. Errors are wrapped with `anyhow::Context` describing which action failed.

### Real implementation notes

- Open `DiagnosticsRelayClient::connect(provider).await` per call (no pooling — the cost is negligible for a one-shot command).
- `reboot()` calls the upstream `restart` method; `shutdown()` calls the upstream `shutdown` method.
- If the upstream API exposes a "wait for disconnect" flag, do **not** set it. The call returns immediately so the final line prints and the program exits.
- After the call, drop the client. Do not call `goodbye()` — the device is going down anyway.

### FakeDevice additions

Add a `power_calls: Arc<Mutex<Vec<PowerCall>>>` field with a small enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerCall { Reboot, Shutdown }
```

`reboot()` and `shutdown()` push the corresponding variant and return `Ok(())`. Tests assert via `fake.power_calls()`.

To exercise the failure path, add `fail_power: bool` (default `false`); when set, both methods return `Err(anyhow!("simulated diagnostics_relay failure"))`.

### Command module structure

`src/commands/power.rs` covers both verbs:

```rust
pub enum Action { Reboot, Shutdown }

pub async fn run(device: &dyn Device, action: Action, yes: bool) -> Result<()>;
```

`run` flow:

1. Read status (`device.status().await`) for the device name in the prompt. If status fails entirely, fall back to the literal string `this iPhone` and continue.
2. If `!yes && !io::stdout().is_terminal()` → return the non-TTY error.
3. If `!yes` → render the confirmation prompt via `dialoguer::Confirm::new()` with default false. On `false`, print `Aborted.` and return `Ok`.
4. Open a spinner with the action-appropriate label.
5. Dispatch to `device.reboot()` or `device.shutdown()` based on `action`.
6. Print the success line. Return `Ok`.

A single `const` table avoids `match` proliferation across prompt / spinner / success copy:

```rust
struct ActionLabels {
    confirm_verb: &'static str,   // "Reboot" / "Shutdown"
    spinner: &'static str,        // "Sending restart…" / "Sending shutdown…"
    success: &'static str,        // "Restart requested. ..." / ...
}

const REBOOT_LABELS: ActionLabels   = ActionLabels { ... };
const SHUTDOWN_LABELS: ActionLabels = ActionLabels { ... };

fn labels(action: Action) -> &'static ActionLabels { ... }
```

Renderers and the dispatcher consume `labels(action)`. Adding `Sleep` later means adding one `const` and one trait method.

### Dispatch + menu integration

`src/lib.rs` gains `Reboot { yes: bool }` and `Shutdown { yes: bool }` subcommands. Both dispatch to `power::run(device, Action::*, yes)`.

`src/commands/menu.rs` gains **Reboot** and **Shutdown** entries just before `Quit`. Full menu order after this spec plus the prior `Info` spec lands:

```
Apps · Analyze · Info · Refresh · Reboot · Shutdown · Quit
```

Both entries call `power::run(device, action, /* yes */ false)`. Because the menu always runs on a TTY, the `dialoguer::Confirm` prompt **is** the confirmation modal — default `No`, shows the device name, single Enter = abort. No extra menu-only confirmation is added; relying on the same path the CLI uses keeps the safety check in exactly one place.

The post-action wait-for-Enter behavior the menu already applies to `Apps` and `Analyze` also applies here, so the user sees `Restart requested.` and acknowledges before the menu redraws. In the reboot case the redraw immediately fails to read status as the device disconnects — that is expected and handled by the existing menu error path.

## Verification needed

1. **Upstream `idevice` method names** for `restart` and `shutdown` on `DiagnosticsRelayClient` — confirm against `tools/src/` in the upstream repo.
2. **"Wait for disconnect" flag** (if present in the upstream API) must be explicitly unset, not relied on as default.

## Error handling

- **Lockdown / diagnostics_relay connect fails**: emit the same trust-and-replug guidance the other commands use. Reuse the helper, do not duplicate copy.
- **`device.reboot()` / `device.shutdown()` returns `Err`**: print
  > error: <action> request failed: <wrapped reason>. The device did not act.
  and exit non-zero. No partial-state recovery — if diagnostics_relay refused, the device did nothing.
- **User aborts the prompt**: print `Aborted.`, exit `Ok` (zero). Aborting is the safe default, not an error.

No raw `idevice` error text reaches the user. Wrapping with `anyhow::Context` happens at the trait boundary; the command layer owns the human copy.

## Testing strategy

### Unit (`#[cfg(test)] mod tests` in `power.rs`)

- `labels(Action::Reboot)` and `labels(Action::Shutdown)` return distinct, non-empty strings for all three fields.
- The confirm prompt body builder (a pure helper) includes the device name when given a populated status, and falls back to `this iPhone` when given `None`.
- The non-TTY error message contains both `--yes` and the verb.

### Integration (`tests/integration.rs`)

- `FakeDevice::default()` + `power::run(device, Action::Reboot, /* yes */ true)` on a non-TTY → returns `Ok(())`, `fake.power_calls()` contains exactly `[PowerCall::Reboot]`.
- Same for `Action::Shutdown` → `[PowerCall::Shutdown]`.
- `power::run(device, Action::Reboot, /* yes */ false)` on a non-TTY → returns `Err`, error string mentions `--yes`, `power_calls` is empty.
- `FakeDevice { fail_power: true, .. }` + `power::run(..., yes: true)` → returns `Err`, error string mentions "request failed".

The interactive TTY prompt is **not** driven in tests — dialoguer is too stateful to fake without a full crossterm harness, same trade-off `apps::tui` already makes.

### E2E

Deliberately **not** added behind `--features e2e`. Rebooting / powering off the test phone every CI-adjacent run is too disruptive. These two commands are verified by hand against a real device when the feature lands, and again whenever the `idevice` pin moves.

Note this gap in the implementation PR description so future contributors don't assume the e2e suite covers it.

## Future work

- `qk sleep` — diagnostics_relay supports it; symmetric path through `power::run`.
- `--wait-for-reconnect` flag — poll usbmuxd until the device comes back, then print uptime delta or `back online in 23s`.
- Optional `--reason "<text>"` recorded somewhere (a local log file?) — useful when scripting reboots from a fleet harness.
