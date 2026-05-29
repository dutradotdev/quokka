//! Formatting helpers shared by every command. Pure where possible (the
//! byte/percent/bar/optional helpers); the spinner constructor returns an
//! `indicatif::ProgressBar` configured the way every command wants.
//!
//! Colour is handled at the stream level (`anstream` strips ANSI on pipes,
//! `owo-colors` honors `NO_COLOR`). `indicatif::ProgressDrawTarget::stderr()`
//! hides spinners on non-TTY automatically.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Placeholder shown when a value is unavailable. Always one char wide.
pub const DASH: &str = "—";

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

/// Format a byte count in human-readable SI units (decimal, base 1000),
/// matching how iOS itself reports storage. Always at least one decimal place
/// above KB; bytes render as integers.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `Some(x)` → `x.to_string()`, `None` → `—`.
pub fn format_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => DASH.to_string(),
    }
}

/// `Some(73)` → `"73%"`, `None` → `—`.
pub fn format_percent(value: Option<u8>) -> String {
    match value {
        Some(v) => format!("{v}%"),
        None => DASH.to_string(),
    }
}

/// Render a fixed-width usage bar.
///
/// `percent` is clamped to `0..=100`. The output is always exactly `width`
/// glyphs of `█` (filled) followed by `░` (empty). Filled count is
/// `floor(percent * width / 100)`.
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

/// Civil (year, month, day) in UTC from a Unix-epoch day count. Uses
/// Howard Hinnant's "days from civil" algorithm so the formatters that
/// need calendar arithmetic don't have to pull in a date crate.
///
/// Three commands need this; keep it here so they don't drift apart.
pub fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

/// Convenience: civil (year, month, day, hour, minute, second) in UTC from
/// a Unix-epoch second count.
pub fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, rem / 3600, (rem / 60) % 60, rem % 60)
}

/// Current wall-clock time as Unix seconds. Falls back to 0 if the system
/// clock is somehow before the epoch.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

pub fn format_bar(percent: u8, width: usize) -> String {
    let percent = percent.min(100) as usize;
    let filled = (percent * width) / 100;
    let empty = width - filled;
    let mut s = String::with_capacity(width * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..empty {
        s.push('░');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_under_1000_renders_as_integer() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(999), "999 B");
    }

    #[test]
    fn format_bytes_steps_through_si_units() {
        assert_eq!(format_bytes(1_000), "1.0 KB");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(256_000_000_000), "256.0 GB");
        assert_eq!(format_bytes(2_500_000_000_000), "2.5 TB");
    }

    #[test]
    fn format_optional_replaces_none_with_dash() {
        assert_eq!(format_optional(Some("hi")), "hi");
        assert_eq!(format_optional::<&str>(None), DASH);
        assert_eq!(format_optional(Some(42)), "42");
    }

    #[test]
    fn format_percent_appends_sign_or_dashes() {
        assert_eq!(format_percent(Some(0)), "0%");
        assert_eq!(format_percent(Some(73)), "73%");
        assert_eq!(format_percent(None), DASH);
    }

    #[test]
    fn format_bar_respects_width_and_clamps_percent() {
        assert_eq!(format_bar(0, 10).chars().count(), 10);
        assert_eq!(format_bar(50, 10), "█████░░░░░");
        assert_eq!(format_bar(100, 4), "████");
        // Clamps above 100.
        assert_eq!(format_bar(250, 4), "████");
    }

    #[test]
    fn format_bar_floors_partial_blocks() {
        // 73% of 20 = 14.6 → 14 filled.
        let bar = format_bar(73, 20);
        assert_eq!(bar.chars().filter(|c| *c == '█').count(), 14);
        assert_eq!(bar.chars().filter(|c| *c == '░').count(), 6);
    }
}
