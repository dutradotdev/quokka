//! Terminal-coupled UI helpers: TTY detection, spinners, progress bars, and
//! the interactive device picker. The pure value formatters live in
//! [`crate::fmt`]; this module re-exports them so commands keep a single
//! `crate::ui::*` import path.
//!
//! Colour is handled at the stream level (`anstream` strips ANSI on pipes,
//! `owo-colors` honors `NO_COLOR`). `indicatif::ProgressDrawTarget::stderr()`
//! hides spinners on non-TTY automatically.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

// The pure formatters (byte/percent/bar/optional + civil date math + now_unix)
// are core logic shared with `--json` and the GUI; re-export them so existing
// `crate::ui::format_bytes` / `crate::ui::now_unix` call sites keep resolving.
pub use crate::fmt::*;

/// Escape hatch that forces every interactive gate off. Set it for scripts and
/// CI — and the test suite relies on it, because `cargo test` keeps the real
/// terminal attached (libtest only redirects the `print!` macros, not the file
/// descriptors), so `is_terminal()` would otherwise be true and tests would
/// launch prompts and TUIs against the developer's terminal.
const NON_INTERACTIVE_ENV: &str = "QK_NON_INTERACTIVE";

fn non_interactive_forced() -> bool {
    std::env::var_os(NON_INTERACTIVE_ENV).is_some()
}

/// Whether quokka may drive interactive prompts on stdin (e.g. `dialoguer`
/// confirmations, the `card` star prompt). False when stdin is not a terminal
/// or when `QK_NON_INTERACTIVE` is set.
pub fn stdin_is_interactive() -> bool {
    !non_interactive_forced() && std::io::stdin().is_terminal()
}

/// Whether quokka may take over the screen with a full-screen TUI. False when
/// stdout is not a terminal or when `QK_NON_INTERACTIVE` is set.
pub fn stdout_is_interactive() -> bool {
    !non_interactive_forced() && std::io::stdout().is_terminal()
}

/// Block until the user presses Enter, after a dimmed prompt. No-op when stdin
/// isn't interactive (pipes/CI) so non-TTY runs never hang. The launchers use
/// it to keep a command's printed output on screen before redrawing over it.
pub fn wait_for_enter() -> std::io::Result<()> {
    use owo_colors::OwoColorize;
    use std::io::Write;

    if !stdin_is_interactive() {
        return Ok(());
    }
    let mut out = anstream::stdout();
    writeln!(out)?;
    write!(out, "{} ", "Press Enter to continue...".dimmed())?;
    out.flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    Ok(())
}

/// Standard spinner for commands. Auto-hides on non-TTY stderr.
pub fn spinner(message: impl Into<String>) -> ProgressBar {
    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr())
        .with_message(message.into());
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    bar.enable_steady_tick(Duration::from_millis(80));
    bar
}

/// Current terminal width in columns, falling back to 80 when stdout is
/// not a terminal or the size query fails. Shared by every command that
/// needs to make a layout decision.
pub fn terminal_width() -> u16 {
    crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80)
}

/// Determinate progress bar — `pos/len` counter with a cyan bar. Auto-hides
/// on non-TTY stderr.
pub fn progress_bar(total: u64, unit: &str) -> ProgressBar {
    let pb = ProgressBar::with_draw_target(Some(total), ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::with_template(&format!(
            "{{spinner}} [{{bar:24.cyan/blue}}] {{pos}}/{{len}} {unit}"
        ))
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .progress_chars("=> "),
    );
    pb
}

/// Interactive device picker for the multi-device case — the `DeviceSelector`
/// the CLI hands to [`crate::device::connect`]. Renders one row per device with
/// `dialoguer` and returns the chosen index (or `None` on abort). The device
/// layer keeps no UI dependency and only calls this after confirming stderr is
/// a TTY.
pub fn select_device(listings: &[crate::device::DeviceListing]) -> anyhow::Result<Option<usize>> {
    let items: Vec<String> = listings.iter().map(format_listing_row).collect();
    dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Multiple devices connected — pick one")
        .items(&items)
        .default(0)
        .interact_opt()
        .map_err(|e| anyhow::anyhow!("picker failed: {e}"))
}

/// One-line label for a device in the picker: name, platform, model,
/// connection, udid. Mirrors the columns `qk devices` prints.
fn format_listing_row(d: &crate::device::DeviceListing) -> String {
    let name = d
        .name
        .as_deref()
        .unwrap_or("(untrusted — tap Trust / Allow)");
    let model = d
        .model_friendly
        .as_deref()
        .or(d.model_identifier.as_deref())
        .unwrap_or("?");
    format!(
        "{name}  ·  {platform}  ·  {model}  ·  {conn}  ·  {udid}",
        platform = d.platform.label(),
        conn = d.connection,
        udid = d.udid,
    )
}
