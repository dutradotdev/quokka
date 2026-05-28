//! `CardData` — the pre-formatted bundle of values [`super::render`] consumes.
//!
//! Everything time-dependent is baked into a string here, so [`super::render`]
//! is a pure function of `CardData` (no `SystemTime::now`, no IO). Tests
//! pass a fixed `now` and get byte-identical SVG output.

use std::time::Duration;

use anyhow::Result;

use crate::device::{App, BatchUpdate, Device, DeviceStatus, OldestApp, Storage, StorageBreakdown};

/// Latest released iOS major. Bumped by hand when Apple ships a new major;
/// drives the `speed_demon` badge. Apple jumped from iOS 18 to iOS 26 in
/// 2025 to align the version number with the release year.
pub const LATEST_IOS_MAJOR: u32 = 26;

/// Seconds in a year (365 days; close enough — the worst-case drift across
/// the badge thresholds is 6h every 4 years, which never flips a result).
const SECONDS_PER_YEAR: i64 = 365 * 86_400;
const SECONDS_PER_DAY: i64 = 86_400;

/// All the values the SVG renderer needs, fully formatted, in one struct.
///
/// Two `CardData` values with identical fields render to byte-identical SVG.
/// Tests guarantee this by injecting a fixed `now`; production runs are
/// idempotent within the same UTC day.
#[derive(Debug, Clone)]
pub struct CardData {
    // ---- header row ----
    /// Marketing model name (e.g. `"iPhone 14 Pro Max"`), or `None` to fall
    /// back to the raw identifier or omit the line.
    pub model_friendly: Option<String>,
    /// SoC marketing name (e.g. `"A16 Bionic"`).
    pub chip_name: Option<String>,
    /// Total storage rendered for the sub-header (e.g. `"256 GB"`). `None`
    /// drops the segment.
    pub storage_label: Option<String>,
    /// Enclosure color (e.g. `"Deep Purple"`). Raw lockdown string,
    /// untranslated.
    pub enclosure_color: Option<String>,
    /// Resolved header caption (line 3 of the header). Already applies the
    /// "vibe when high-tier badge present, identity otherwise" rule so the
    /// renderer can emit it directly. `None` hides the line.
    pub header_caption: Option<String>,

    // ---- battery section ----
    pub battery_level_percent: Option<u8>,
    pub battery_cycle_count: Option<u32>,
    pub battery_health_percent: Option<u8>,
    pub battery_health_tier: HealthTier,

    // ---- storage section ----
    /// 4-row breakdown shown in the card. `None` (no breakdown returned by
    /// iOS) collapses to a single used/free bar — see [`StorageFallback`].
    pub storage_breakdown_rows: Option<StorageBreakdownRows>,
    pub storage_fallback: Option<StorageFallback>,

    // ---- info table ----
    pub ios_label: String,
    /// `Some("· beta")` to append after `ios_label`, `None` to omit.
    pub ios_beta_suffix: Option<&'static str>,
    pub app_count: Option<usize>,
    /// `· pristine` (good) or `· jailbroken` (bad). Appended to the apps
    /// row.
    pub apps_jailbreak_label: AppsJailbreakLabel,
    /// Pre-formatted "first seen" line (e.g. `"Mar 2022 · Spotify is your oldest"`)
    /// or `None` to omit the row.
    pub first_seen_line: Option<String>,
    /// Pre-formatted "backup" line (e.g. `"12 days ago"`). `None` omits.
    pub backup_age_label: Option<String>,

    // ---- top apps ----
    /// Heaviest 5 apps (user + system) by combined static + dynamic disk
    /// usage. `None` when enrichment failed or timed out — the renderer
    /// then omits the whole section.
    pub top_apps: Option<Vec<TopApp>>,

    // ---- earned ----
    /// Already-ranked, already-clipped-to-3 badges.
    pub badges: Vec<super::badges::Badge>,
    /// One-line nudge pointing at the next reachable badge
    /// (`"→ 23 cycles to Heavy Cycle"`). `None` when nothing is close.
    pub next_badge_hint: Option<String>,

    // ---- footer ----
    pub footer_date: String,
    /// Day-rotating call-to-action that replaces the static repo URL in the
    /// footer. Cycles between cross-promotion of other `qk` subcommands and
    /// the GitHub repo (1 of 5 days), so the link still surfaces.
    pub footer_cta: &'static str,

    // ---- raw signals retained for completeness ----
    pub redact: bool,
}

/// The "good / warn / bad" colour tier a stat is rendered with. Drives the
/// battery bar tint as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthTier {
    Good,
    Warn,
    Bad,
    /// Unknown — render in `Text secondary` instead of an accent.
    Unknown,
}

/// Three categories + free for the storage block.
#[derive(Debug, Clone)]
pub struct StorageBreakdownRows {
    pub camera_label: String,
    pub apps_label: String,
    pub other_label: String,
    pub free_label: String,
    /// Fill levels (0..=11) for the mini-bars per category. Pre-clamped.
    pub camera_cells: u8,
    pub apps_cells: u8,
    pub other_cells: u8,
}

/// One row of the TOP APPS section.
#[derive(Debug, Clone)]
pub struct TopApp {
    pub display_name: String,
    pub size_label: String,
    /// 0..=11 cells, scaled relative to the heaviest of the row set.
    pub bar_cells: u8,
}

/// Single-bar fallback when iOS did not return a breakdown.
#[derive(Debug, Clone)]
pub struct StorageFallback {
    pub used_label: String,
    pub total_label: String,
    pub free_label: String,
    pub used_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppsJailbreakLabel {
    Pristine,
    Jailbroken,
    None,
}

/// How long we'll wait for `with_dynamic_sizes` to enrich the top 10
/// before giving up on the TOP APPS section. iOS 26+ has been observed
/// to stall some Dynamic browses; the timeout keeps the card always
/// renderable.
const TOP_APPS_TIMEOUT: Duration = Duration::from_secs(10);

/// How many apps we ask the device to enrich. Anything past 10 doesn't
/// move the top-5 result (the runners-up are far enough behind) and
/// each extra app is another `BundleIDs` round-trip.
const TOP_APPS_ENRICH_BUDGET: usize = 10;

/// How many rows show in the card. Spec keeps it small so each row has
/// breathing room and the bars compare meaningfully.
const TOP_APPS_SHOWN: usize = 5;

/// Read the device once, build the `CardData` the renderer needs. Pure
/// projection on the result.
pub async fn collect(device: &dyn Device, now_unix: i64, redact: bool) -> Result<CardData> {
    let status = device.status().await?;
    let mut card = project(&status, now_unix, redact);
    card.top_apps = collect_top_apps(device).await;
    Ok(card)
}

/// Fetch all apps, sort by static size, enrich the heaviest 10 with
/// dynamic (cache/downloads) sizes, return the heaviest 5 by combined
/// size. Returns `None` on any failure path — the renderer then omits
/// the TOP APPS section entirely.
async fn collect_top_apps(device: &dyn Device) -> Option<Vec<TopApp>> {
    let apps = device.all_apps().await.ok()?;
    if apps.is_empty() {
        return None;
    }
    let mut by_static = apps;
    by_static.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    by_static.truncate(TOP_APPS_ENRICH_BUDGET);

    // No-op progress callback — the spinner in `mod.rs` already covers
    // the wait. With_dynamic_sizes fires one of these per batch.
    let on_batch: crate::device::BatchCallback = Box::new(|_: BatchUpdate| {});
    let mut enriched = match tokio::time::timeout(
        TOP_APPS_TIMEOUT,
        device.with_dynamic_sizes(by_static, on_batch),
    )
    .await
    {
        Ok(Ok(list)) => list,
        Ok(Err(_)) | Err(_) => return None,
    };
    enriched.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    enriched.truncate(TOP_APPS_SHOWN);
    Some(rank_top_apps(&enriched))
}

/// Pre-format the 5-row table — names truncated to fit the column, mini-bar
/// cells scaled against the heaviest of the row set. Non-zero apps always
/// get at least one cell so a dominant outlier (e.g. CapCut 60 GB next to
/// 1-3 GB peers) doesn't render the smaller bars as empty rails.
pub fn rank_top_apps(apps: &[App]) -> Vec<TopApp> {
    let max_bytes = apps.iter().map(|a| a.size_bytes).max().unwrap_or(0);
    apps.iter()
        .map(|app| {
            let mut cells = cells_of(app.size_bytes, max_bytes);
            if cells == 0 && app.size_bytes > 0 {
                cells = 1;
            }
            TopApp {
                display_name: truncate_app_name(&app.name),
                size_label: format_app_size(app.size_bytes),
                bar_cells: cells,
            }
        })
        .collect()
}

/// `"543 MB"` / `"1.4 GB"` — match how iOS Settings labels app sizes.
/// Anything below 1 GB renders in MB so small system apps don't all
/// collapse to `"0.0 GB"`.
pub fn format_app_size(bytes: u64) -> String {
    if bytes < 1_000_000_000 {
        let mb = (bytes as f64 / 1_000_000.0).round() as u64;
        format!("{mb} MB")
    } else {
        format_gigs(bytes)
    }
}

/// `"Instagram"` (fits) / `"Microsoft Power…"` (cut). The narrow right
/// column on the card has ~200px between the name column and the size
/// value (`bar_x - value_x - name_x`). At JetBrains Mono 22px (≈ 13.2px
/// advance) that's ~15 chars; we cap at 14 to leave a one-char buffer
/// so the trailing ellipsis never butts up against the size value.
fn truncate_app_name(name: &str) -> String {
    const MAX: usize = 14;
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= MAX {
        return name.to_string();
    }
    let mut out: String = chars[..MAX - 1].iter().collect();
    out.push('…');
    out
}

/// Pure transform `(status, now) → CardData`. Tested independently so the
/// formatting rules don't drift between renderer and badge logic.
pub fn project(status: &DeviceStatus, now_unix: i64, redact: bool) -> CardData {
    let storage_total_gb = status
        .storage
        .as_ref()
        .map(|s| s.total_bytes / 1_000_000_000);

    let storage_label = storage_total_gb.map(|g| format!("{g} GB"));
    // Enclosure colour: lockdown returns either a human-readable name
    // (e.g. "Deep Purple") or an opaque short code (e.g. "1"). Drop
    // anything that's missing a space and has fewer than 4 chars — those
    // codes don't render as anything users recognise.
    let enclosure_color = status.enclosure_color.as_deref().and_then(|s| {
        let trimmed = s.trim();
        if trimmed.len() >= 4 || trimmed.contains(' ') {
            Some(trimmed.to_string())
        } else {
            None
        }
    });
    let battery_health_tier = match status.battery.health_percent {
        Some(h) if h >= 85 => HealthTier::Good,
        Some(h) if h >= 70 => HealthTier::Warn,
        Some(_) => HealthTier::Bad,
        None => HealthTier::Unknown,
    };

    let (storage_breakdown_rows, storage_fallback) =
        build_storage_view(status.storage.as_ref(), status.storage_breakdown.as_ref());

    let ios_label = match (&status.ios_version, &status.ios_build) {
        (Some(v), Some(b)) if !redact => format!("iOS {v} ({b})"),
        (Some(v), _) => {
            if redact {
                // `--redact`: drop the build, keep only major.minor.
                let mm = v.splitn(3, '.').take(2).collect::<Vec<_>>().join(".");
                format!("iOS {mm}")
            } else {
                format!("iOS {v}")
            }
        }
        (None, _) => "iOS —".into(),
    };

    let ios_beta_suffix = if status.is_beta_build {
        Some(" · beta")
    } else {
        None
    };

    let apps_jailbreak_label = if status.app_count.is_none() {
        AppsJailbreakLabel::None
    } else if status.jailbreak_detected {
        AppsJailbreakLabel::Jailbroken
    } else {
        AppsJailbreakLabel::Pristine
    };

    let first_seen_line =
        build_first_seen_line(status.paired_since_unix, status.oldest_app.as_ref(), redact);

    let backup_age_label = status
        .last_backup_unix
        .map(|ts| format_backup_age(now_unix, ts, redact));

    let badge_inputs = BadgeInputs::from_status(status, now_unix);
    let badges_full = super::badges::evaluate_all(&badge_inputs);
    // Vibe is synthesised from the full set (sorted by priority), not
    // the clipped top-3 — so e.g. a "minimalist veteran" composes even
    // if only veteran fits in the EARNED row.
    let mut sorted_for_vibe = badges_full.clone();
    sorted_for_vibe.sort_by_key(|b| super::badges::priority_for(b.id));
    let vibe_label = synthesize_vibe(&sorted_for_vibe);
    let identity_label = build_identity_label(status);
    let earned_ids: Vec<super::badges::BadgeId> = badges_full.iter().map(|b| b.id).collect();
    let next_badge_hint = super::badges::next_badge_hint(&badge_inputs, &earned_ids);
    let badges = super::badges::top_three(badges_full);
    let header_caption = resolve_header_caption(&badges, vibe_label, identity_label);
    // TOP APPS comes from `collect()`'s post-projection enrichment.
    let top_apps = None;

    let footer_date = format_footer_date(now_unix);
    let footer_cta = pick_footer_cta(now_unix);

    CardData {
        model_friendly: status
            .model_friendly
            .clone()
            .or_else(|| status.model.clone()),
        chip_name: status.chip_name.clone(),
        storage_label,
        enclosure_color,
        header_caption,
        battery_level_percent: status.battery.level_percent,
        battery_cycle_count: status.battery.cycle_count,
        battery_health_percent: status.battery.health_percent,
        battery_health_tier,
        storage_breakdown_rows,
        storage_fallback,
        ios_label,
        ios_beta_suffix,
        app_count: status.app_count,
        apps_jailbreak_label,
        first_seen_line,
        backup_age_label,
        badges,
        next_badge_hint,
        top_apps,
        footer_date,
        footer_cta,
        redact,
    }
}

/// Apply the "vibe when high-tier badge present, identity otherwise" rule
/// to the third header line. Identity falls back to vibe when there's no
/// concrete data to render — so the slot never silently goes blank when
/// either side has something to say.
pub fn resolve_header_caption(
    badges_top_three: &[super::badges::Badge],
    vibe_label: Option<String>,
    identity_label: Option<String>,
) -> Option<String> {
    if super::badges::has_high_tier_flex(badges_top_three) {
        vibe_label
    } else {
        identity_label.or(vibe_label)
    }
}

/// Day-rotating CTAs that replace the static `github.com/dutradotdev/quokka`
/// segment in the card footer. The pool keeps cross-promotion of the rest of
/// the CLI front-and-centre while still surfacing the repo every fifth day so
/// the link doesn't disappear from shared screenshots.
const FOOTER_CTAS: &[&str] = &[
    "try: qk apps",
    "try: qk analyze",
    "try: qk media",
    "try: qk capture",
    "star us: github.com/dutradotdev/quokka",
];

/// Deterministic day-bucket pick — same UTC day → same CTA, so the card
/// stays reproducible within the day a user re-runs it.
pub fn pick_footer_cta(now_unix: i64) -> &'static str {
    let day = (now_unix.max(0) / SECONDS_PER_DAY) as usize;
    FOOTER_CTAS[day % FOOTER_CTAS.len()]
}

/// "Receipts" line for the header — used when the badge mix doesn't include
/// a heavy-hitter flex, so the third line still reads as identity instead of
/// going blank. Returns `None` when nothing concrete is available.
pub fn build_identity_label(status: &DeviceStatus) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(app) = status.oldest_app.as_ref() {
        let (y, _, _, _, _, _) = crate::ui::civil_from_unix(app.install_date_unix);
        parts.push(format!("{} since {y}", app.display_name));
    }
    if let Some(n) = status.app_count {
        parts.push(format!("{n} apps"));
    }
    if let Some(major) = status
        .ios_version
        .as_deref()
        .and_then(|v| v.split('.').next())
        .and_then(|s| s.parse::<u32>().ok())
    {
        parts.push(format!("iOS {major}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// The pre-derived inputs each badge eligibility check needs. Decoupling
/// these from `DeviceStatus` keeps badge logic testable with hand-built
/// scenarios.
#[derive(Debug, Clone)]
pub struct BadgeInputs {
    pub battery_health_percent: Option<u8>,
    pub battery_cycle_count: Option<u32>,
    pub age_years: Option<u32>,
    pub paired_year: Option<i32>,
    pub app_count: Option<usize>,
    pub storage_used_percent: Option<u8>,
    pub storage_total_gb: Option<u64>,
    pub backup_age_days: Option<i64>,
    pub is_beta_build: bool,
    pub ios_major: Option<u32>,
    pub model_friendly: Option<String>,
    /// Calendar year the iPhone model first shipped (e.g. iPhone 14 Pro
    /// Max → 2022). Used by the `DayOne` badge.
    pub model_release_year: Option<u32>,
}

impl BadgeInputs {
    pub fn from_status(status: &DeviceStatus, now_unix: i64) -> Self {
        let age_years = status
            .paired_since_unix
            .map(|ts| ((now_unix - ts).max(0) / SECONDS_PER_YEAR) as u32);
        let paired_year = status
            .paired_since_unix
            .map(|ts| crate::ui::civil_from_unix(ts).0);
        let storage_used_percent = status.storage.map(Storage::used_percent);
        let storage_total_gb = status.storage.map(|s| s.total_bytes / 1_000_000_000);
        let backup_age_days = status
            .last_backup_unix
            .map(|ts| (now_unix - ts).max(0) / SECONDS_PER_DAY);
        let ios_major = status
            .ios_version
            .as_deref()
            .and_then(|v| v.split('.').next())
            .and_then(|s| s.parse::<u32>().ok());
        Self {
            battery_health_percent: status.battery.health_percent,
            battery_cycle_count: status.battery.cycle_count,
            age_years,
            paired_year,
            app_count: status.app_count,
            storage_used_percent,
            storage_total_gb,
            backup_age_days,
            is_beta_build: status.is_beta_build,
            ios_major,
            model_friendly: status.model_friendly.clone(),
            model_release_year: status
                .model
                .as_deref()
                .and_then(crate::device::model_release_years::release_year),
        }
    }
}

/// Build the 4-row storage view or fall back to a single bar. Both rows are
/// pre-formatted strings so the renderer is text-only.
pub fn build_storage_view(
    storage: Option<&Storage>,
    breakdown: Option<&StorageBreakdown>,
) -> (Option<StorageBreakdownRows>, Option<StorageFallback>) {
    let Some(storage) = storage else {
        return (None, None);
    };
    let free_label = format_gigs(storage.free_bytes);
    match breakdown {
        Some(b) => {
            // Sum the three categories so each gets a fair share of the
            // 11-cell mini-bar — using `total` as denominator would leave
            // the bars short when iOS withholds some bytes from the
            // breakdown (which it always does on iOS 18+).
            let denom = b
                .camera_bytes
                .saturating_add(b.apps_bytes)
                .saturating_add(b.other_bytes);
            let camera_cells = cells_of(b.camera_bytes, denom);
            let apps_cells = cells_of(b.apps_bytes, denom);
            let other_cells = cells_of(b.other_bytes, denom);
            (
                Some(StorageBreakdownRows {
                    camera_label: format_gigs(b.camera_bytes),
                    apps_label: format_gigs(b.apps_bytes),
                    other_label: format_gigs(b.other_bytes),
                    free_label,
                    camera_cells,
                    apps_cells,
                    other_cells,
                }),
                None,
            )
        }
        None => (
            None,
            Some(StorageFallback {
                used_label: format_gigs(storage.used_bytes()),
                total_label: format_gigs(storage.total_bytes),
                free_label,
                used_percent: storage.used_percent(),
            }),
        ),
    }
}

const BAR_CELLS: u8 = 11;

fn cells_of(part: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let cells = (part as u128 * BAR_CELLS as u128) / total as u128;
    cells.min(BAR_CELLS as u128) as u8
}

/// `"84.0 GB"` — one decimal place, base-1000. Matches how iOS Settings
/// reports storage.
pub fn format_gigs(bytes: u64) -> String {
    let g = bytes as f64 / 1_000_000_000.0;
    format!("{g:.1} GB")
}

/// Compose a 2-3 word vibe tagline from the user's qualifying badges,
/// in priority order. Negative badges (`BackupOverdue`, `MaxedOut`) are
/// excluded — the tagline is meant to be the user's "claim", not a
/// problem list. Falls back to `None` when nothing qualifies.
pub fn synthesize_vibe(badges_sorted_by_priority: &[super::badges::Badge]) -> Option<String> {
    use super::badges::BadgeId;
    let mut traits: Vec<&'static str> = Vec::new();
    for badge in badges_sorted_by_priority {
        let word = match badge.id {
            BadgeId::Untouchable => "Battery legend",
            BadgeId::BatteryChamp => "Battery champ",
            BadgeId::ChargingWizard => "Charging wizard",
            BadgeId::OgOwner => "OG owner",
            BadgeId::DayOne => "Day one",
            BadgeId::Survivor => "Survivor",
            BadgeId::Veteran => "Veteran",
            BadgeId::StorageTitan => "Storage titan",
            BadgeId::HeavyCycle => "Heavy charger",
            BadgeId::BetaTester => "Beta tester",
            BadgeId::Minimalist => "Minimalist",
            BadgeId::AppCollector => "Power user",
            BadgeId::ProMaxClub => "Top-tier",
            BadgeId::TidyHoarder => "Tidy",
            BadgeId::BackupFresh => "Disciplined",
            BadgeId::SpeedDemon => "Up to date",
            // Negative / problem-y badges — don't compose into vibe.
            BadgeId::MaxedOut | BadgeId::BackupOverdue => continue,
        };
        if !traits.contains(&word) {
            traits.push(word);
            if traits.len() == 3 {
                break;
            }
        }
    }
    if traits.is_empty() {
        None
    } else {
        Some(traits.join(" · "))
    }
}

/// `"paired 4 yr 2 mo ago"` — honest copy: this is the **pair-record age on
/// this Mac**, which is not the same as the device's lifetime. The string
/// reflects that so a fresh re-pair doesn't claim the iPhone is brand-new.
/// Returns `None` (caller omits the line) when the pair record is less
/// than a month old — saying "paired this month" on a card that's about
/// device-flexing reads like a bug.
pub fn format_pair_age(now_unix: i64, paired_unix: i64) -> String {
    let diff = (now_unix - paired_unix).max(0);
    let total_months = diff / (30 * SECONDS_PER_DAY); // 30-day months — coarse on purpose
    let years = total_months / 12;
    let months = total_months % 12;
    match (years, months) {
        (0, 0) => "paired with this Mac recently".to_string(),
        (0, m) => format!("paired {m} mo ago"),
        (y, 0) => format!("paired {y} yr ago"),
        (y, m) => format!("paired {y} yr {m} mo ago"),
    }
}

/// Either `"Mar 2022 · Spotify is your oldest"` or `"Mar 2022"` (no oldest
/// app) or `"2022"` (`--redact`) or `None` (no pair date).
pub fn build_first_seen_line(
    paired_unix: Option<i64>,
    oldest_app: Option<&OldestApp>,
    redact: bool,
) -> Option<String> {
    let paired = paired_unix?;
    let (y, m, _, _, _, _) = crate::ui::civil_from_unix(paired);
    let base = if redact {
        format!("{y}")
    } else {
        format!("{} {y}", month_name(m))
    };
    match oldest_app {
        Some(app) if !redact => Some(format!("{base} · {} is your oldest", app.display_name)),
        _ => Some(base),
    }
}

/// `"12 days ago"` / `"3 weeks ago"` / `"recent"` (`--redact`).
pub fn format_backup_age(now_unix: i64, backup_unix: i64, redact: bool) -> String {
    let days = ((now_unix - backup_unix).max(0)) / SECONDS_PER_DAY;
    if redact {
        return match days {
            0..=7 => "recent".into(),
            8..=30 => "a few weeks ago".into(),
            _ => "a while ago".into(),
        };
    }
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=13 => format!("{days} days ago"),
        14..=59 => format!("{} weeks ago", days / 7),
        _ => format!("{} months ago", days / 30),
    }
}

/// `"May 27"` — abbreviated month + day, no year.
pub fn format_footer_date(now_unix: i64) -> String {
    let (_, m, d, _, _, _) = crate::ui::civil_from_unix(now_unix);
    format!("{} {d}", month_name(m))
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Battery;

    const NOW: i64 = 1_716_854_400; // 2024-05-28 UTC

    fn base_status() -> DeviceStatus {
        DeviceStatus {
            model_friendly: Some("iPhone 14 Pro Max".into()),
            chip_name: Some("A16 Bionic".into()),
            enclosure_color: Some("Deep Purple".into()),
            ios_version: Some("18.2".into()),
            ios_build: Some("22C152".into()),
            storage: Some(Storage {
                total_bytes: 256_000_000_000,
                free_bytes: 148_500_000_000,
                ..Storage::default()
            }),
            storage_breakdown: Some(StorageBreakdown {
                camera_bytes: 84_000_000_000,
                apps_bytes: 18_700_000_000,
                other_bytes: 4_800_000_000,
            }),
            battery: Battery {
                level_percent: Some(91),
                cycle_count: Some(142),
                health_percent: Some(91),
                ..Battery::default()
            },
            app_count: Some(47),
            paired_since_unix: Some(NOW - 4 * SECONDS_PER_YEAR - 2 * 30 * SECONDS_PER_DAY),
            last_backup_unix: Some(NOW - 12 * SECONDS_PER_DAY),
            oldest_app: Some(OldestApp {
                bundle_id: "com.spotify.client".into(),
                display_name: "Spotify".into(),
                install_date_unix: NOW - 2 * SECONDS_PER_YEAR,
            }),
            jailbreak_detected: false,
            is_beta_build: false,
            ..DeviceStatus::default()
        }
    }

    #[test]
    fn format_gigs_one_decimal_with_unit() {
        assert_eq!(format_gigs(84_000_000_000), "84.0 GB");
        assert_eq!(format_gigs(1_000_000_000), "1.0 GB");
        assert_eq!(format_gigs(0), "0.0 GB");
    }

    #[test]
    fn format_pair_age_picks_y_m_components() {
        let paired = NOW - 4 * SECONDS_PER_YEAR - 2 * 30 * SECONDS_PER_DAY;
        assert_eq!(format_pair_age(NOW, paired), "paired 4 yr 2 mo ago");
        let paired = NOW - 9 * 30 * SECONDS_PER_DAY;
        assert_eq!(format_pair_age(NOW, paired), "paired 9 mo ago");
        let paired = NOW - 2 * SECONDS_PER_YEAR;
        assert_eq!(format_pair_age(NOW, paired), "paired 2 yr ago");
    }

    #[test]
    fn format_backup_age_default_buckets() {
        assert_eq!(format_backup_age(NOW, NOW, false), "today");
        assert_eq!(format_backup_age(NOW, NOW - 86_400, false), "yesterday");
        assert_eq!(
            format_backup_age(NOW, NOW - 12 * 86_400, false),
            "12 days ago"
        );
        assert_eq!(
            format_backup_age(NOW, NOW - 21 * 86_400, false),
            "3 weeks ago"
        );
        assert_eq!(
            format_backup_age(NOW, NOW - 90 * 86_400, false),
            "3 months ago"
        );
    }

    #[test]
    fn format_backup_age_redact_buckets_coarsely() {
        assert_eq!(format_backup_age(NOW, NOW - 3 * 86_400, true), "recent");
        assert_eq!(
            format_backup_age(NOW, NOW - 20 * 86_400, true),
            "a few weeks ago"
        );
        assert_eq!(
            format_backup_age(NOW, NOW - 90 * 86_400, true),
            "a while ago"
        );
    }

    #[test]
    fn first_seen_line_includes_oldest_app_by_default() {
        let status = base_status();
        let line =
            build_first_seen_line(status.paired_since_unix, status.oldest_app.as_ref(), false)
                .expect("paired present");
        assert!(line.contains("Spotify is your oldest"), "got `{line}`");
        assert!(line.starts_with(month_name(
            crate::ui::civil_from_unix(status.paired_since_unix.unwrap()).1
        )));
    }

    #[test]
    fn first_seen_line_redact_shows_year_only_and_drops_oldest_app() {
        let status = base_status();
        let line =
            build_first_seen_line(status.paired_since_unix, status.oldest_app.as_ref(), true)
                .expect("paired present");
        assert!(!line.contains("Spotify"));
        assert!(!line.contains("Mar"));
        // 2024 - 4y 2mo ≈ early 2020.
        assert!(
            line.parse::<i32>().is_ok(),
            "redacted line should be a bare year, got `{line}`"
        );
    }

    #[test]
    fn ios_label_redact_keeps_major_minor_only() {
        let mut status = base_status();
        status.ios_build = Some("22C152".into());
        let normal = project(&status, NOW, false);
        let redacted = project(&status, NOW, true);
        assert!(normal.ios_label.contains("(22C152)"));
        assert_eq!(redacted.ios_label, "iOS 18.2");
    }

    #[test]
    fn storage_breakdown_falls_back_to_single_bar_when_ios_did_not_return_categories() {
        let mut status = base_status();
        status.storage_breakdown = None;
        let card = project(&status, NOW, false);
        assert!(card.storage_breakdown_rows.is_none());
        assert!(card.storage_fallback.is_some());
    }

    #[test]
    fn cells_of_clamps_and_handles_zero() {
        assert_eq!(cells_of(0, 100), 0);
        assert_eq!(cells_of(50, 100), 5);
        assert_eq!(cells_of(100, 100), 11);
        assert_eq!(cells_of(99, 0), 0, "zero denominator must not panic");
    }

    #[test]
    fn battery_health_tier_thresholds() {
        let mut s = base_status();
        s.battery.health_percent = Some(85);
        assert_eq!(
            project(&s, NOW, false).battery_health_tier,
            HealthTier::Good
        );
        s.battery.health_percent = Some(84);
        assert_eq!(
            project(&s, NOW, false).battery_health_tier,
            HealthTier::Warn
        );
        s.battery.health_percent = Some(69);
        assert_eq!(project(&s, NOW, false).battery_health_tier, HealthTier::Bad);
        s.battery.health_percent = None;
        assert_eq!(
            project(&s, NOW, false).battery_health_tier,
            HealthTier::Unknown
        );
    }

    // ---- regression-protection tests added in the post-implementation review ----

    #[test]
    fn synthesize_vibe_skips_negative_badges() {
        use super::super::badges::{Badge, BadgeColor, BadgeId};
        let make = |id: BadgeId| Badge {
            id,
            title: "t",
            subtitle: "s",
            color: BadgeColor::Good,
        };
        // BackupOverdue + MaxedOut are both "negative" — must not enter the vibe.
        let badges = vec![
            make(BadgeId::BackupOverdue),
            make(BadgeId::MaxedOut),
            make(BadgeId::Veteran),
        ];
        let vibe = synthesize_vibe(&badges).expect("Veteran qualifies");
        assert_eq!(vibe, "Veteran");
        assert!(!vibe.contains("Backup"));
        assert!(!vibe.contains("Maxed"));

        // Only-negative input → no vibe.
        let only_negative = vec![make(BadgeId::BackupOverdue), make(BadgeId::MaxedOut)];
        assert_eq!(synthesize_vibe(&only_negative), None);
    }

    #[test]
    fn synthesize_vibe_clips_to_three_traits() {
        use super::super::badges::{Badge, BadgeColor, BadgeId};
        let make = |id: BadgeId| Badge {
            id,
            title: "t",
            subtitle: "s",
            color: BadgeColor::Good,
        };
        // Five qualifying badges — vibe must take exactly the first 3 by order.
        let badges = vec![
            make(BadgeId::Untouchable),  // "Battery legend"
            make(BadgeId::OgOwner),      // "OG owner"
            make(BadgeId::StorageTitan), // "Storage titan"
            make(BadgeId::ProMaxClub),   // "Top-tier"
            make(BadgeId::BetaTester),   // "Beta tester"
        ];
        let vibe = synthesize_vibe(&badges).expect("qualifies");
        assert_eq!(vibe.matches(" · ").count(), 2, "exactly 3 traits joined");
    }

    #[test]
    fn rank_top_apps_gives_nonzero_outlier_at_least_one_cell() {
        // One dominant app + a tiny peer. The peer should still show a cell.
        let apps = vec![
            App {
                bundle_id: "a".into(),
                name: "Big".into(),
                size_bytes: 60_000_000_000,
                is_system: false,
                install_date_unix: None,
            },
            App {
                bundle_id: "b".into(),
                name: "Tiny".into(),
                size_bytes: 100_000_000, // ~0.17% of Big — would round to 0 cells naively
                is_system: false,
                install_date_unix: None,
            },
        ];
        let ranked = rank_top_apps(&apps);
        assert_eq!(ranked[0].bar_cells, 11, "heaviest fills the bar");
        assert!(
            ranked[1].bar_cells >= 1,
            "non-zero outlier must keep ≥1 cell, got {}",
            ranked[1].bar_cells
        );
    }

    #[test]
    fn truncate_app_name_caps_at_14_with_ellipsis() {
        // ≤14 → unchanged
        assert_eq!(truncate_app_name("Instagram"), "Instagram");
        assert_eq!(truncate_app_name("12345678901234"), "12345678901234"); // exactly 14
                                                                           // >14 → 13 chars + ellipsis (1 char), total 14 visible glyphs
        let long = "Microsoft PowerPoint";
        let out = truncate_app_name(long);
        assert_eq!(out.chars().count(), 14);
        assert!(out.ends_with('…'));
        assert!(out.starts_with("Microsoft Pow"));
    }

    #[test]
    fn enclosure_color_filter_drops_opaque_short_codes() {
        let mut s = base_status();
        // Short opaque code with no space → dropped.
        s.enclosure_color = Some("1".into());
        assert_eq!(project(&s, NOW, false).enclosure_color, None);
        s.enclosure_color = Some("abc".into());
        assert_eq!(project(&s, NOW, false).enclosure_color, None);
        // ≥4 chars or contains a space → kept (and trimmed).
        s.enclosure_color = Some("Black".into());
        assert_eq!(
            project(&s, NOW, false).enclosure_color.as_deref(),
            Some("Black")
        );
        // Contains a space → kept regardless of length.
        s.enclosure_color = Some("Jet Black".into());
        assert_eq!(
            project(&s, NOW, false).enclosure_color.as_deref(),
            Some("Jet Black")
        );
    }

    #[test]
    fn pick_footer_cta_rotates_by_day_bucket() {
        // Day 0 → first slot; day +1 → second; full cycle wraps cleanly.
        let day0 = 0_i64;
        let day1 = SECONDS_PER_DAY;
        let day_n = SECONDS_PER_DAY * FOOTER_CTAS.len() as i64;
        assert_eq!(pick_footer_cta(day0), FOOTER_CTAS[0]);
        assert_eq!(pick_footer_cta(day1), FOOTER_CTAS[1]);
        assert_eq!(pick_footer_cta(day_n), FOOTER_CTAS[0]);
    }

    #[test]
    fn footer_cta_pool_keeps_repo_link_in_rotation() {
        // The star CTA is the only one that surfaces the repo URL in the
        // shared PNG — removing it accidentally would erase a discovery hook.
        assert!(
            FOOTER_CTAS
                .iter()
                .any(|c| c.contains("github.com/dutradotdev/quokka")),
            "footer rotation must include at least one repo-link CTA"
        );
    }

    #[test]
    fn build_identity_label_skips_missing_segments() {
        let mut s = base_status();
        // Drop app_count — should still render with the other two parts.
        s.app_count = None;
        let label = build_identity_label(&s).expect("oldest_app + iOS present");
        assert!(label.contains("Spotify since"));
        assert!(label.contains("iOS"));
        assert!(!label.contains("apps"));
    }

    #[test]
    fn resolve_header_caption_picks_vibe_when_high_tier_flex_present() {
        use super::super::badges::{Badge, BadgeColor, BadgeId};
        let badge = |id| Badge {
            id,
            title: "t",
            subtitle: "s",
            color: BadgeColor::Good,
        };
        // Untouchable → vibe wins, identity is suppressed (the flex badge
        // already tells a stronger story).
        let caption = resolve_header_caption(
            &[badge(BadgeId::Untouchable), badge(BadgeId::Veteran)],
            Some("Battery legend".into()),
            Some("Spotify since 2022".into()),
        );
        assert_eq!(caption.as_deref(), Some("Battery legend"));
    }

    #[test]
    fn resolve_header_caption_falls_back_to_vibe_when_identity_missing() {
        use super::super::badges::{Badge, BadgeColor, BadgeId};
        let badge = |id| Badge {
            id,
            title: "t",
            subtitle: "s",
            color: BadgeColor::Good,
        };
        // Bug D protection: no high-tier badge AND identity unavailable
        // (e.g. no oldest app + no iOS version) must NOT leave the slot
        // blank — vibe is the last-resort fallback.
        let caption = resolve_header_caption(
            &[badge(BadgeId::Veteran)],
            Some("Veteran · Tidy".into()),
            None,
        );
        assert_eq!(caption.as_deref(), Some("Veteran · Tidy"));
    }

    #[test]
    fn build_identity_label_is_none_when_no_signals() {
        let mut s = base_status();
        s.oldest_app = None;
        s.app_count = None;
        s.ios_version = None;
        assert!(build_identity_label(&s).is_none());
    }

    #[test]
    fn build_storage_view_denominator_is_sum_of_categories_not_total() {
        // iOS withholds bytes from the breakdown — total > sum(camera+apps+other).
        // If we used `total` as denominator the bars would all under-fill.
        let storage = Storage {
            total_bytes: 256_000_000_000,
            free_bytes: 100_000_000_000,
            ..Storage::default()
        };
        let breakdown = StorageBreakdown {
            camera_bytes: 80_000_000_000,
            apps_bytes: 40_000_000_000,
            other_bytes: 20_000_000_000,
            // sum = 140 GB; total is 256 GB — 116 GB withheld
        };
        let (rows, _) = build_storage_view(Some(&storage), Some(&breakdown));
        let rows = rows.expect("breakdown path");
        // 80 / 140 ≈ 0.571 → 6 cells out of 11. Using `total` would give 80/256 → 3 cells.
        assert_eq!(rows.camera_cells, 6);
        assert_eq!(rows.apps_cells, 3);
        assert_eq!(rows.other_cells, 1);
    }
}
