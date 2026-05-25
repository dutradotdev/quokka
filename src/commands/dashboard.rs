//! Welcome dashboard: a two-column block printed when the user runs
//! `quokka` with no subcommand (and re-used by `quokka status`).
//!
//! Pure renderers — no I/O — so the layout decisions are unit-testable.
//! Side-by-side on wide terminals, stacked when the terminal is narrow.

use owo_colors::{AnsiColors, OwoColorize};

use crate::device::{Battery, DeviceStatus, Storage};
use crate::ui::{format_bar, format_bytes, format_optional, DASH};

/// Width of the disk-usage bar in the storage row.
const STORAGE_BAR_WIDTH: usize = 10;
/// Visible width of the ASCII-art column (padding applied to each line).
/// Must be at least as wide as the longest line of [`QUOKKA_ART`].
const ART_WIDTH: usize = 34;
/// Visible columns between the art and status block in side-by-side mode.
const COLUMN_GAP: usize = 4;
/// Terminals narrower than this fall back to a stacked layout (art on top,
/// status under it). 76 keeps the status block readable without overlap.
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 76;

// Storage usage threshold colours. Storage is "lower is better"; battery
// thresholds are "higher is better".
const STORAGE_USAGE_WARN: u8 = 75;
const STORAGE_USAGE_CRIT: u8 = 90;
const BATTERY_LEVEL_WARN: u8 = 50;
const BATTERY_LEVEL_CRIT: u8 = 20;
const BATTERY_HEALTH_WARN: u8 = 85;
const BATTERY_HEALTH_CRIT: u8 = 70;

/// The `quokka` wordmark (figlet "Standard" font), 6 lines. Each line is
/// padded out to [`ART_WIDTH`] before colouring so the status column always
/// lands in the same place regardless of terminal theme or the applied colour.
/// Raw strings so the backslashes and backticks need no escaping.
const QUOKKA_ART: &[&str] = &[
    r"                   _    _",
    r"  __ _ _   _  ___ | | _| | ____ _",
    r" / _` | | | |/ _ \| |/ / |/ / _` |",
    r"| (_| | |_| | (_) |   <|   < (_| |",
    r" \__, |\__,_|\___/|_|\_\_|\_\__,_|",
    r"    |_|",
];

/// Top-level entry point. Picks side-by-side vs. stacked layout from the
/// terminal width and returns the full block ready to print. `now_unix` is
/// the current time, threaded in so relative-date formatting is testable.
pub fn render(status: &DeviceStatus, term_width: u16, now_unix: i64) -> String {
    let art = render_art(status.enclosure_color.as_deref());
    let status_block = render_status_block(status);

    let mut out = if term_width < SIDE_BY_SIDE_MIN_WIDTH {
        let mut stacked = art;
        if !stacked.ends_with('\n') {
            stacked.push('\n');
        }
        stacked.push('\n');
        stacked.push_str(&status_block);
        stacked
    } else {
        join_columns(&art, &status_block)
    };

    if let Some(footer) = render_footer(status, now_unix) {
        out.push_str("\n\n");
        out.push_str(&footer);
    }
    out
}

/// Coloured ASCII quokka. Each line is padded to a fixed visible width so the
/// status column lines up regardless of which row of art it sits against.
pub fn render_art(enclosure_color: Option<&str>) -> String {
    let color = pick_color(enclosure_color);
    let mut out = String::with_capacity(QUOKKA_ART.len() * (ART_WIDTH + 8));
    for (i, line) in QUOKKA_ART.iter().enumerate() {
        let padded = pad_visible(line, ART_WIDTH);
        out.push_str(&padded.color(color).to_string());
        if i + 1 < QUOKKA_ART.len() {
            out.push('\n');
        }
    }
    out
}

/// The right-hand column: identity header, iOS, storage, battery.
pub fn render_status_block(status: &DeviceStatus) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(16);

    lines.push(render_identity(status));
    lines.push(render_os_line(status));
    lines.push(String::new());
    lines.extend(render_storage(status.storage.as_ref()));
    lines.push(String::new());
    lines.extend(render_battery(&status.battery));

    lines.join("\n")
}

/// Map a lockdown enclosure colour string to one of the 8 ANSI base colours.
/// Match is case-insensitive and whitespace-trimmed. Unknown / missing values
/// fall back to green (the prior default) — the dashboard never warns about
/// a colour it couldn't resolve.
pub fn pick_color(enclosure_color: Option<&str>) -> AnsiColors {
    let key = match enclosure_color {
        Some(s) => s.trim(),
        None => return AnsiColors::Green,
    };
    let eq = |needle: &str| key.eq_ignore_ascii_case(needle);

    if eq("Black") || eq("Space Black") || eq("Midnight") || eq("Graphite") {
        AnsiColors::BrightBlack
    } else if eq("White") || eq("Starlight") || eq("Silver") {
        AnsiColors::White
    } else if eq("Blue") || eq("Sierra Blue") || eq("Pacific Blue") || eq("Blue Titanium") {
        AnsiColors::Blue
    } else if eq("Red") || eq("Product Red") || eq("(PRODUCT)RED") {
        AnsiColors::Red
    } else if eq("Green") || eq("Alpine Green") || eq("Midnight Green") {
        AnsiColors::Green
    } else if eq("Gold") || eq("Yellow") || eq("Desert Titanium") || eq("Natural Titanium") {
        AnsiColors::Yellow
    } else if eq("Purple") || eq("Deep Purple") {
        AnsiColors::Magenta
    } else if eq("Pink") || eq("Rose Gold") || eq("Coral") {
        AnsiColors::BrightMagenta
    } else {
        AnsiColors::Green
    }
}

fn render_identity(status: &DeviceStatus) -> String {
    let name = status.name.as_deref();
    let model_label = status.model_friendly.as_deref().or(status.model.as_deref());
    match (name, model_label) {
        (Some(n), Some(m)) => format!("{} ({})", n.bold(), m.dimmed()),
        (Some(n), None) => n.bold().to_string(),
        (None, Some(m)) => m.dimmed().to_string(),
        (None, None) => DASH.to_string(),
    }
}

fn render_os_line(status: &DeviceStatus) -> String {
    let version = format_optional(status.ios_version.as_deref());
    let mut line = match status.ios_build.as_deref() {
        Some(b) => format!("iOS {version} ({})", format!("build {b}").dimmed()),
        None => format!("iOS {version}"),
    };
    // Locale and time zone only join the OS line when both are known —
    // a lone locale without a region reads as noise.
    if let (Some(locale), Some(tz)) = (status.locale.as_deref(), status.time_zone.as_deref()) {
        let region = tz.rsplit('/').next().unwrap_or(tz).replace('_', " ");
        line.push_str(&format!(" · {locale} · {region}").dimmed().to_string());
    }
    line
}

fn render_storage(storage: Option<&Storage>) -> Vec<String> {
    let label = "Storage";
    let Some(s) = storage else {
        return vec![format!("{label:<9} {DASH}")];
    };
    let percent = s.used_percent();
    let bar = colour_for_usage(percent, format_bar(percent, STORAGE_BAR_WIDTH));
    let summary = format!(
        "{label:<9} {bar}  {percent:>3}%  {used} / {total}",
        used = format_bytes(s.used_bytes()),
        total = format_bytes(s.total_bytes),
    );
    let mut out = vec![summary];
    // Three-line breakdown only when both new fields are present — otherwise
    // the single summary row already covers everything we know.
    if let (Some(system), Some(data)) = (s.system_bytes, s.data_used_bytes) {
        let indent = " ".repeat(label.len() + 2);
        out.push(format!("{indent}├─ System  {:>8}", format_bytes(system)));
        out.push(format!("{indent}├─ Data    {:>8}", format_bytes(data)));
        out.push(format!(
            "{indent}└─ Free    {:>8}",
            format_bytes(s.free_bytes)
        ));
    }
    out
}

fn render_battery(battery: &Battery) -> Vec<String> {
    let level = match battery.level_percent {
        Some(p) => {
            let coloured = colour_for_battery_level(p, format!("{p}%"));
            match battery.is_charging {
                Some(true) => format!("{coloured} {} {}", "⚡".yellow(), charger_label(battery)),
                _ => coloured,
            }
        }
        None => DASH.to_string(),
    };
    let health = battery
        .health_percent
        .map(|p| colour_for_battery_health(p, format!("{p}%")))
        .unwrap_or_else(|| DASH.to_string());
    let temp = battery
        .temperature_celsius
        .map(|t| format!("{t:.1} °C"))
        .unwrap_or_else(|| DASH.to_string());

    vec![
        format!("Battery   level   {level}"),
        format!("          health  {health}"),
        format!("          cycles  {}", format_optional(battery.cycle_count)),
        format!("          temp    {temp}"),
    ]
}

/// Charging suffix after the bolt. Prefers wattage ("20W USB-C"), falls
/// back to the plain word when only the boolean charging signal is known.
fn charger_label(battery: &Battery) -> String {
    match (
        battery.adapter_watts,
        battery.adapter_description.as_deref(),
    ) {
        (Some(w), Some(desc)) => format!("{w}W {desc}"),
        (Some(w), None) => format!("{w}W"),
        (None, _) => "charging".to_string(),
    }
}

/// The two-line footer below the columns. Line 1 is trivia (app count,
/// last backup, pair date); line 2 lists only the flags in an unusual
/// state. Returns `None` when there is nothing to show — the caller then
/// skips the blank separator too, keeping the dashboard unchanged for a
/// device with no extras.
fn render_footer(status: &DeviceStatus, now_unix: i64) -> Option<String> {
    let mut trivia: Vec<String> = Vec::new();
    if let Some(count) = status.app_count {
        trivia.push(format!("{count} apps"));
    }
    if let Some(unix) = status.last_backup_unix {
        trivia.push(format!(
            "last backup {}",
            format_relative_date(unix, now_unix)
        ));
    }
    if let Some(unix) = status.paired_since_unix {
        trivia.push(format!(
            "paired since {}",
            format_relative_date(unix, now_unix)
        ));
    }

    let mut alerts: Vec<String> = Vec::new();
    if status.developer_mode == Some(true) {
        alerts.push("Developer Mode on".to_string());
    }
    if status.find_my == Some(false) {
        alerts.push("Find My off".to_string());
    }

    let mut lines: Vec<String> = Vec::new();
    if !trivia.is_empty() {
        lines.push(format!("  {}", trivia.join(" · ").dimmed()));
    }
    if !alerts.is_empty() {
        let body = alerts.join(" · ");
        lines.push(format!("  {} {}", "⚠".yellow(), body));
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Human-friendly relative date. Coarsens as the gap grows: recent times
/// are "N days ago", anything past a month becomes "Mon YYYY", past a year
/// just "YYYY". `now_unix` is passed in so the result is deterministic in
/// tests.
fn format_relative_date(unix: i64, now_unix: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let delta = now_unix - unix;
    if delta < MINUTE {
        return "just now".to_string();
    }
    if delta < DAY {
        let hours = delta / HOUR;
        return match hours {
            0 => "less than an hour ago".to_string(),
            1 => "1 hour ago".to_string(),
            n => format!("{n} hours ago"),
        };
    }
    if delta < 30 * DAY {
        let days = delta / DAY;
        return match days {
            1 => "1 day ago".to_string(),
            n => format!("{n} days ago"),
        };
    }
    let (year, month) = year_month_utc(unix);
    if delta < 365 * DAY {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let name = MONTHS.get((month - 1) as usize).copied().unwrap_or("");
        return format!("{name} {year}");
    }
    year.to_string()
}

/// Civil (year, month) in UTC from a Unix timestamp. Month is 1..=12.
/// Uses the well-known days-from-civil algorithm so we don't need a date
/// crate for the one place the dashboard formats absolute dates.
fn year_month_utc(unix: i64) -> (i64, i64) {
    let days = unix.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month)
}

fn colour_for_usage(percent: u8, text: String) -> String {
    if percent >= STORAGE_USAGE_CRIT {
        text.red().to_string()
    } else if percent >= STORAGE_USAGE_WARN {
        text.yellow().to_string()
    } else {
        text.green().to_string()
    }
}

fn colour_for_battery_level(percent: u8, text: String) -> String {
    if percent < BATTERY_LEVEL_CRIT {
        text.red().to_string()
    } else if percent < BATTERY_LEVEL_WARN {
        text.yellow().to_string()
    } else {
        text.green().to_string()
    }
}

fn colour_for_battery_health(percent: u8, text: String) -> String {
    if percent < BATTERY_HEALTH_CRIT {
        text.red().to_string()
    } else if percent < BATTERY_HEALTH_WARN {
        text.yellow().to_string()
    } else {
        text.green().to_string()
    }
}

/// Pad a string with trailing spaces so its visible character count reaches
/// `width`. Lines longer than `width` are returned unchanged — the art is
/// hand-tuned to fit, but defensively we don't want to ever truncate it.
fn pad_visible(s: &str, width: usize) -> String {
    let visible = s.chars().count();
    if visible >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - visible));
    out.push_str(s);
    for _ in 0..(width - visible) {
        out.push(' ');
    }
    out
}

/// Stitch two multi-line blocks together row-by-row. Each line of `left`
/// is already padded to a fixed visible width by [`render_art`]; whichever
/// side has more lines gets blank rows from the other.
fn join_columns(left: &str, right: &str) -> String {
    let lefts: Vec<&str> = left.split('\n').collect();
    let rights: Vec<&str> = right.split('\n').collect();
    let rows = lefts.len().max(rights.len());
    let gap = " ".repeat(COLUMN_GAP);
    let blank_left = " ".repeat(ART_WIDTH);
    let mut out = String::new();
    for i in 0..rows {
        let l = lefts.get(i).copied().unwrap_or(blank_left.as_str());
        let r = rights.get(i).copied().unwrap_or("");
        out.push_str(l);
        out.push_str(&gap);
        out.push_str(r);
        if i + 1 < rows {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> DeviceStatus {
        DeviceStatus {
            name: Some("Lucas's iPhone".into()),
            model: Some("iPhone15,3".into()),
            model_friendly: Some("iPhone 14 Pro Max".into()),
            ios_version: Some("18.2".into()),
            ios_build: Some("22C152".into()),
            enclosure_color: Some("Deep Purple".into()),
            storage: Some(Storage {
                total_bytes: 256_000_000_000,
                free_bytes: 95_400_000_000,
                system_bytes: Some(12_400_000_000),
                data_used_bytes: Some(148_200_000_000),
            }),
            battery: Battery {
                level_percent: Some(87),
                cycle_count: Some(142),
                health_percent: Some(91),
                temperature_celsius: Some(27.4),
                is_charging: Some(true),
                adapter_watts: Some(20),
                adapter_description: Some("USB-C".into()),
            },
            locale: Some("pt-BR".into()),
            time_zone: Some("America/Sao_Paulo".into()),
            app_count: Some(47),
            developer_mode: Some(false),
            find_my: Some(true),
            last_backup_unix: Some(NOW_UNIX - 3 * 86_400),
            paired_since_unix: Some(NOW_UNIX - 800 * 86_400),
        }
    }

    /// Fixed clock anchor for tests so relative dates are deterministic.
    /// 2026-05-19 12:00 UTC.
    const NOW_UNIX: i64 = 1_779_793_200;

    /// Status with every new field cleared — used to prove the footer and
    /// extras vanish without disturbing the original dashboard.
    fn bare() -> DeviceStatus {
        let mut s = healthy();
        s.locale = None;
        s.time_zone = None;
        s.app_count = None;
        s.developer_mode = None;
        s.find_my = None;
        s.last_backup_unix = None;
        s.paired_since_unix = None;
        s
    }

    #[test]
    fn full_dashboard_contains_every_extra() {
        let out = render(&healthy(), 100, NOW_UNIX);
        assert!(out.contains("Lucas's iPhone"));
        assert!(out.contains("iPhone 14 Pro Max"));
        assert!(out.contains("iOS 18.2"));
        assert!(out.contains("build 22C152"));
        assert!(out.contains("├─ System"));
        assert!(out.contains("├─ Data"));
        assert!(out.contains("└─ Free"));
        assert!(out.contains("⚡"));
        assert!(out.contains("27.4 °C"));
        // Footer fragments.
        assert!(out.contains("47 apps"));
        assert!(out.contains("last backup 3 days ago"));
        assert!(out.contains("paired since"));
        // Locale joins the OS line; time zone shows its last component.
        assert!(out.contains("pt-BR"));
        assert!(out.contains("Sao Paulo"));
    }

    #[test]
    fn missing_model_friendly_falls_back_to_raw_identifier() {
        let mut s = healthy();
        s.model_friendly = None;
        let out = render(&s, 100, NOW_UNIX);
        assert!(out.contains("iPhone15,3"));
        assert!(!out.contains("iPhone 14 Pro Max"));
    }

    #[test]
    fn missing_build_drops_build_suffix() {
        let mut s = healthy();
        s.ios_build = None;
        let out = render(&s, 100, NOW_UNIX);
        assert!(out.contains("iOS 18.2"));
        assert!(!out.contains("build"));
    }

    #[test]
    fn missing_storage_breakdown_keeps_single_row() {
        let mut s = healthy();
        if let Some(st) = s.storage.as_mut() {
            st.system_bytes = None;
            st.data_used_bytes = None;
        }
        let out = render(&s, 100, NOW_UNIX);
        assert!(!out.contains("├─ System"));
        assert!(out.contains("Storage"));
    }

    #[test]
    fn not_charging_drops_bolt() {
        let mut s = healthy();
        s.battery.is_charging = Some(false);
        let out = render(&s, 100, NOW_UNIX);
        assert!(!out.contains("⚡"));
    }

    #[test]
    fn unknown_is_charging_drops_bolt() {
        let mut s = healthy();
        s.battery.is_charging = None;
        let out = render(&s, 100, NOW_UNIX);
        assert!(!out.contains("⚡"));
    }

    #[test]
    fn charging_shows_wattage_when_known() {
        let out = render(&healthy(), 100, NOW_UNIX);
        assert!(out.contains("⚡"));
        assert!(out.contains("20W USB-C"));
    }

    #[test]
    fn charging_without_wattage_falls_back_to_word() {
        let mut s = healthy();
        s.battery.adapter_watts = None;
        s.battery.adapter_description = None;
        let out = render(&s, 100, NOW_UNIX);
        assert!(out.contains("⚡"));
        assert!(out.contains("charging"));
        assert!(!out.contains("20W"));
    }

    #[test]
    fn os_line_omits_locale_when_time_zone_missing() {
        let mut s = healthy();
        s.time_zone = None;
        let out = render(&s, 100, NOW_UNIX);
        assert!(!out.contains("pt-BR"));
    }

    #[test]
    fn footer_omitted_when_no_extras() {
        let out = render(&bare(), 100, NOW_UNIX);
        assert!(!out.contains("apps"));
        assert!(!out.contains("paired"));
        assert!(!out.contains("⚠"));
    }

    #[test]
    fn footer_joins_only_known_trivia_segments() {
        let mut s = bare();
        s.app_count = Some(12);
        let footer = render_footer(&s, NOW_UNIX).expect("app count alone yields a footer");
        assert!(footer.contains("12 apps"));
        assert!(!footer.contains("·"));
    }

    #[test]
    fn footer_alert_line_only_for_abnormal_flags() {
        // Expected values → no alert line.
        let mut ok = bare();
        ok.developer_mode = Some(false);
        ok.find_my = Some(true);
        assert!(render_footer(&ok, NOW_UNIX).is_none());

        // Developer Mode on is abnormal.
        let mut dev = bare();
        dev.developer_mode = Some(true);
        let footer = render_footer(&dev, NOW_UNIX).expect("dev mode triggers footer");
        assert!(footer.contains("Developer Mode on"));

        // Find My off is abnormal.
        let mut fmip = bare();
        fmip.find_my = Some(false);
        let footer = render_footer(&fmip, NOW_UNIX).expect("find my off triggers footer");
        assert!(footer.contains("Find My off"));
    }

    #[test]
    fn relative_date_coarsens_with_distance() {
        assert_eq!(format_relative_date(NOW_UNIX - 30, NOW_UNIX), "just now");
        assert_eq!(
            format_relative_date(NOW_UNIX - 3 * 3600, NOW_UNIX),
            "3 hours ago"
        );
        assert_eq!(
            format_relative_date(NOW_UNIX - 5 * 86_400, NOW_UNIX),
            "5 days ago"
        );
        // ~4 months back lands in the "Mon YYYY" branch.
        assert_eq!(
            format_relative_date(NOW_UNIX - 120 * 86_400, NOW_UNIX),
            "Jan 2026"
        );
        // ~2 years back collapses to the bare year.
        assert_eq!(
            format_relative_date(NOW_UNIX - 800 * 86_400, NOW_UNIX),
            "2024"
        );
    }

    #[test]
    fn narrow_terminal_stacks_layout() {
        let out_wide = render(&healthy(), 100, NOW_UNIX);
        let out_narrow = render(&healthy(), 40, NOW_UNIX);
        // Stacked output has more total lines than side-by-side (art lines
        // are no longer overlapped with status lines).
        let wide_lines = out_wide.lines().count();
        let narrow_lines = out_narrow.lines().count();
        assert!(narrow_lines > wide_lines);
    }

    #[test]
    fn color_mapping_covers_known_names() {
        assert_eq!(pick_color(Some("Midnight")), AnsiColors::BrightBlack);
        assert_eq!(pick_color(Some("Starlight")), AnsiColors::White);
        assert_eq!(pick_color(Some("Sierra Blue")), AnsiColors::Blue);
        assert_eq!(pick_color(Some("(PRODUCT)RED")), AnsiColors::Red);
        assert_eq!(pick_color(Some("Alpine Green")), AnsiColors::Green);
        assert_eq!(pick_color(Some("Natural Titanium")), AnsiColors::Yellow);
        assert_eq!(pick_color(Some("Deep Purple")), AnsiColors::Magenta);
        assert_eq!(pick_color(Some("Rose Gold")), AnsiColors::BrightMagenta);
    }

    #[test]
    fn color_mapping_is_case_insensitive_and_trims() {
        assert_eq!(pick_color(Some("  midnight  ")), AnsiColors::BrightBlack);
        assert_eq!(pick_color(Some("DEEP PURPLE")), AnsiColors::Magenta);
    }

    #[test]
    fn color_mapping_unknown_falls_back_to_green() {
        assert_eq!(pick_color(Some("#3a3a3c")), AnsiColors::Green);
        assert_eq!(pick_color(Some("1")), AnsiColors::Green);
        assert_eq!(pick_color(None), AnsiColors::Green);
    }
}
