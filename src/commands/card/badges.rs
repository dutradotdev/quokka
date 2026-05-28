//! Badge catalog for `qk card`.
//!
//! 18 entries, each a pure `fn(&BadgeInputs) -> Option<Badge>`. Evaluate all,
//! sort by priority (lower wins), take the top 3.

use super::data::BadgeInputs;

/// All 18 badge ids. The variant order is **not** the priority order —
/// that lives in [`priority_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeId {
    Untouchable,
    BatteryChamp,
    ChargingWizard,
    OgOwner,
    DayOne,
    Survivor,
    Veteran,
    StorageTitan,
    MaxedOut,
    HeavyCycle,
    BetaTester,
    BackupOverdue,
    Minimalist,
    AppCollector,
    ProMaxClub,
    TidyHoarder,
    BackupFresh,
    SpeedDemon,
}

/// Colour family of a badge — drives fill / stroke / title / subtitle
/// colours when rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeColor {
    Good,
    Warn,
    Bad,
    Info,
}

/// A single renderable badge.
#[derive(Debug, Clone)]
pub struct Badge {
    pub id: BadgeId,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub color: BadgeColor,
}

/// Lower numbers win. See the spec for rationale (rarer / harder-to-fake
/// badges first; `beta_tester` is high because beta users amplify dev
/// tools). The 3 new badges slot in by rarity: `Untouchable` (health =
/// 100, very rare), `ChargingWizard` (200+ cycles AT 95%+ health,
/// hardcore flex), `DayOne` (paired in release year, early adopter).
pub fn priority_for(id: BadgeId) -> u8 {
    match id {
        BadgeId::Untouchable => 1,
        BadgeId::BatteryChamp => 2,
        BadgeId::ChargingWizard => 3,
        BadgeId::OgOwner => 4,
        BadgeId::DayOne => 5,
        BadgeId::Survivor => 6,
        BadgeId::Veteran => 7,
        BadgeId::StorageTitan => 8,
        BadgeId::MaxedOut => 9,
        BadgeId::HeavyCycle => 10,
        BadgeId::BetaTester => 11,
        BadgeId::BackupOverdue => 12,
        BadgeId::Minimalist => 13,
        BadgeId::AppCollector => 14,
        BadgeId::ProMaxClub => 15,
        BadgeId::TidyHoarder => 16,
        BadgeId::BackupFresh => 17,
        BadgeId::SpeedDemon => 18,
    }
}

pub fn evaluate_all(input: &BadgeInputs) -> Vec<Badge> {
    [
        untouchable(input),
        battery_champ(input),
        charging_wizard(input),
        og_owner(input),
        day_one(input),
        survivor(input),
        veteran(input),
        storage_titan(input),
        maxed_out(input),
        heavy_cycle(input),
        beta_tester(input),
        backup_overdue(input),
        minimalist(input),
        app_collector(input),
        pro_max_club(input),
        tidy_hoarder(input),
        backup_fresh(input),
        speed_demon(input),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub fn top_three(mut badges: Vec<Badge>) -> Vec<Badge> {
    badges.sort_by_key(|b| priority_for(b.id));
    badges.truncate(3);
    badges
}

/// Badges that drive the abstract "vibe" header caption. When one of these
/// is on the card the vibe tagline wins; otherwise the header shows the
/// concrete identity line. Listed here (not in render) so the rule is
/// testable as a pure function of the badge set.
pub const HIGH_TIER_FLEX: &[BadgeId] = &[
    BadgeId::Untouchable,
    BadgeId::ChargingWizard,
    BadgeId::OgOwner,
];

pub fn has_high_tier_flex(badges: &[Badge]) -> bool {
    badges.iter().any(|b| HIGH_TIER_FLEX.contains(&b.id))
}

/// Suggest the next badge the user could plausibly unlock soon, as a
/// pre-formatted hint string (`"→ 23 cycles to Heavy Cycle"`). Returns
/// `None` when nothing near-reach qualifies.
///
/// Priority order is "smallest-effort first": single-action nudges (back up,
/// update iOS) before count-based grinds. "Reachable" thresholds are chosen
/// so the hint reads as motivational, not discouraging — e.g. "say 80 cycles
/// to Heavy Cycle" would just nag.
pub fn next_badge_hint(input: &BadgeInputs, earned: &[BadgeId]) -> Option<String> {
    let already_has = |id: BadgeId| earned.contains(&id);

    // Single-action nudges first.
    if !already_has(BadgeId::BackupFresh) {
        if let Some(days) = input.backup_age_days {
            if (8..=30).contains(&days) {
                return Some("→ back up to earn Backup Fresh".to_string());
            }
        }
    }
    // Count-based: battery cycles.
    if !already_has(BadgeId::HeavyCycle) {
        if let Some(c) = input.battery_cycle_count {
            if (250..300).contains(&c) {
                return Some(format!("→ {} cycles to Heavy Cycle", 300 - c));
            }
        }
    }
    // Count-based: installed apps.
    if !already_has(BadgeId::AppCollector) {
        if let Some(n) = input.app_count {
            if (125..150).contains(&n) {
                return Some(format!("→ {} apps to App Collector", 150 - n));
            }
        }
    }
    // Combined: ChargingWizard needs both cycles AND high health.
    if !already_has(BadgeId::ChargingWizard) {
        if let (Some(c), Some(h)) = (input.battery_cycle_count, input.battery_health_percent) {
            if (150..200).contains(&c) && h >= 95 {
                return Some(format!("→ {} cycles to Charging Wizard", 200 - c));
            }
        }
    }
    // Single-action: one major behind the latest iOS.
    if !already_has(BadgeId::SpeedDemon) {
        if let Some(m) = input.ios_major {
            if m + 1 == super::data::LATEST_IOS_MAJOR {
                return Some("→ update iOS to earn Speed Demon".to_string());
            }
        }
    }
    None
}

// ---- individual checks ----

fn untouchable(i: &BadgeInputs) -> Option<Badge> {
    if i.battery_health_percent? == 100 {
        return Some(Badge {
            id: BadgeId::Untouchable,
            title: "Untouchable",
            subtitle: "battery at 100%",
            color: BadgeColor::Good,
        });
    }
    None
}

fn charging_wizard(i: &BadgeInputs) -> Option<Badge> {
    if i.battery_cycle_count? >= 200 && i.battery_health_percent? >= 95 {
        return Some(Badge {
            id: BadgeId::ChargingWizard,
            title: "Charging Wizard",
            subtitle: "200+ cycles · 95% health",
            color: BadgeColor::Good,
        });
    }
    None
}

fn day_one(i: &BadgeInputs) -> Option<Badge> {
    let paired = i.paired_year?;
    let released = i.model_release_year?;
    if paired == released as i32 {
        return Some(Badge {
            id: BadgeId::DayOne,
            title: "Day One",
            subtitle: "paired in release year",
            color: BadgeColor::Info,
        });
    }
    None
}

fn battery_champ(i: &BadgeInputs) -> Option<Badge> {
    if i.battery_health_percent? >= 90 && i.age_years? >= 3 {
        return Some(Badge {
            id: BadgeId::BatteryChamp,
            title: "Battery Champ",
            subtitle: "90%+ after 3+ years",
            color: BadgeColor::Good,
        });
    }
    None
}

fn og_owner(i: &BadgeInputs) -> Option<Badge> {
    if i.paired_year? <= 2020 {
        return Some(Badge {
            id: BadgeId::OgOwner,
            title: "OG Owner",
            subtitle: "first paired in 2020 or earlier",
            color: BadgeColor::Warn,
        });
    }
    None
}

fn survivor(i: &BadgeInputs) -> Option<Badge> {
    if i.age_years? >= 4 {
        return Some(Badge {
            id: BadgeId::Survivor,
            title: "Survivor",
            subtitle: "4+ years, going strong",
            color: BadgeColor::Warn,
        });
    }
    None
}

fn veteran(i: &BadgeInputs) -> Option<Badge> {
    let y = i.age_years?;
    if (3..4).contains(&y) {
        return Some(Badge {
            id: BadgeId::Veteran,
            title: "Veteran",
            subtitle: "3+ years in service",
            color: BadgeColor::Warn,
        });
    }
    None
}

fn storage_titan(i: &BadgeInputs) -> Option<Badge> {
    if i.storage_total_gb? >= 1_000 {
        return Some(Badge {
            id: BadgeId::StorageTitan,
            title: "Storage Titan",
            subtitle: "1TB device",
            color: BadgeColor::Info,
        });
    }
    None
}

fn maxed_out(i: &BadgeInputs) -> Option<Badge> {
    if i.storage_used_percent? >= 90 {
        return Some(Badge {
            id: BadgeId::MaxedOut,
            title: "Maxed Out",
            subtitle: "over 90% storage used",
            color: BadgeColor::Bad,
        });
    }
    None
}

fn heavy_cycle(i: &BadgeInputs) -> Option<Badge> {
    if i.battery_cycle_count? >= 300 {
        return Some(Badge {
            id: BadgeId::HeavyCycle,
            title: "Heavy Cycle",
            subtitle: "300+ battery cycles",
            color: BadgeColor::Warn,
        });
    }
    None
}

fn beta_tester(i: &BadgeInputs) -> Option<Badge> {
    if i.is_beta_build {
        return Some(Badge {
            id: BadgeId::BetaTester,
            title: "Beta Tester",
            subtitle: "running iOS beta",
            color: BadgeColor::Info,
        });
    }
    None
}

fn backup_overdue(i: &BadgeInputs) -> Option<Badge> {
    if i.backup_age_days? > 30 {
        return Some(Badge {
            id: BadgeId::BackupOverdue,
            title: "Backup Overdue",
            subtitle: "last backup > 30 days",
            color: BadgeColor::Warn,
        });
    }
    None
}

fn minimalist(i: &BadgeInputs) -> Option<Badge> {
    if i.app_count? < 25 {
        return Some(Badge {
            id: BadgeId::Minimalist,
            title: "Minimalist",
            subtitle: "fewer than 25 apps",
            color: BadgeColor::Info,
        });
    }
    None
}

fn app_collector(i: &BadgeInputs) -> Option<Badge> {
    if i.app_count? >= 150 {
        return Some(Badge {
            id: BadgeId::AppCollector,
            title: "App Collector",
            subtitle: "150+ apps installed",
            color: BadgeColor::Info,
        });
    }
    None
}

fn pro_max_club(i: &BadgeInputs) -> Option<Badge> {
    if i.model_friendly.as_deref()?.contains("Pro Max") {
        return Some(Badge {
            id: BadgeId::ProMaxClub,
            title: "Pro Max Club",
            subtitle: "top-tier model",
            color: BadgeColor::Info,
        });
    }
    None
}

fn tidy_hoarder(i: &BadgeInputs) -> Option<Badge> {
    if i.storage_used_percent? < 50 && i.storage_total_gb? >= 256 {
        return Some(Badge {
            id: BadgeId::TidyHoarder,
            title: "Tidy Hoarder",
            subtitle: "under 50% storage used",
            color: BadgeColor::Good,
        });
    }
    None
}

fn backup_fresh(i: &BadgeInputs) -> Option<Badge> {
    if i.backup_age_days? <= 7 {
        return Some(Badge {
            id: BadgeId::BackupFresh,
            title: "Backup Fresh",
            subtitle: "backed up this week",
            color: BadgeColor::Good,
        });
    }
    None
}

fn speed_demon(i: &BadgeInputs) -> Option<Badge> {
    if i.ios_major? == super::data::LATEST_IOS_MAJOR {
        return Some(Badge {
            id: BadgeId::SpeedDemon,
            title: "Speed Demon",
            subtitle: "latest iOS major",
            color: BadgeColor::Info,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> BadgeInputs {
        BadgeInputs {
            battery_health_percent: Some(91),
            battery_cycle_count: Some(142),
            age_years: Some(2),
            paired_year: Some(2022),
            app_count: Some(47),
            storage_used_percent: Some(42),
            storage_total_gb: Some(256),
            backup_age_days: Some(12),
            is_beta_build: false,
            ios_major: Some(super::super::data::LATEST_IOS_MAJOR),
            model_friendly: Some("iPhone 14 Pro Max".into()),
            model_release_year: Some(2022),
        }
    }

    #[test]
    fn battery_champ_requires_both_thresholds() {
        let mut i = base_input();
        i.battery_health_percent = Some(90);
        i.age_years = Some(3);
        assert!(battery_champ(&i).is_some());
        // 89 health misses.
        i.battery_health_percent = Some(89);
        assert!(battery_champ(&i).is_none());
        // Right health, 2 years misses.
        i.battery_health_percent = Some(90);
        i.age_years = Some(2);
        assert!(battery_champ(&i).is_none());
    }

    #[test]
    fn survivor_and_veteran_are_disjoint() {
        let mut i = base_input();
        i.age_years = Some(3);
        assert!(veteran(&i).is_some());
        assert!(survivor(&i).is_none());
        i.age_years = Some(4);
        assert!(veteran(&i).is_none());
        assert!(survivor(&i).is_some());
    }

    #[test]
    fn og_owner_threshold_is_2020() {
        let mut i = base_input();
        i.paired_year = Some(2020);
        assert!(og_owner(&i).is_some());
        i.paired_year = Some(2021);
        assert!(og_owner(&i).is_none());
    }

    #[test]
    fn storage_titan_at_1tb() {
        let mut i = base_input();
        i.storage_total_gb = Some(1_000);
        assert!(storage_titan(&i).is_some());
        i.storage_total_gb = Some(999);
        assert!(storage_titan(&i).is_none());
    }

    #[test]
    fn maxed_out_at_90_percent() {
        let mut i = base_input();
        i.storage_used_percent = Some(90);
        assert!(maxed_out(&i).is_some());
        i.storage_used_percent = Some(89);
        assert!(maxed_out(&i).is_none());
    }

    #[test]
    fn heavy_cycle_at_300() {
        let mut i = base_input();
        i.battery_cycle_count = Some(300);
        assert!(heavy_cycle(&i).is_some());
        i.battery_cycle_count = Some(299);
        assert!(heavy_cycle(&i).is_none());
    }

    #[test]
    fn beta_tester_follows_flag() {
        let mut i = base_input();
        i.is_beta_build = true;
        assert!(beta_tester(&i).is_some());
        i.is_beta_build = false;
        assert!(beta_tester(&i).is_none());
    }

    #[test]
    fn backup_buckets() {
        let mut i = base_input();
        i.backup_age_days = Some(7);
        assert!(backup_fresh(&i).is_some());
        assert!(backup_overdue(&i).is_none());
        i.backup_age_days = Some(8);
        assert!(backup_fresh(&i).is_none());
        assert!(backup_overdue(&i).is_none());
        i.backup_age_days = Some(31);
        assert!(backup_overdue(&i).is_some());
        assert!(backup_fresh(&i).is_none());
    }

    #[test]
    fn minimalist_and_collector_are_disjoint() {
        let mut i = base_input();
        i.app_count = Some(24);
        assert!(minimalist(&i).is_some());
        assert!(app_collector(&i).is_none());
        i.app_count = Some(25);
        assert!(minimalist(&i).is_none());
        assert!(app_collector(&i).is_none());
        i.app_count = Some(150);
        assert!(app_collector(&i).is_some());
    }

    #[test]
    fn pro_max_club_matches_substring() {
        let mut i = base_input();
        i.model_friendly = Some("iPhone 14 Pro Max".into());
        assert!(pro_max_club(&i).is_some());
        i.model_friendly = Some("iPhone 14 Pro".into());
        assert!(pro_max_club(&i).is_none());
    }

    #[test]
    fn tidy_hoarder_requires_both() {
        let mut i = base_input();
        i.storage_used_percent = Some(49);
        i.storage_total_gb = Some(256);
        assert!(tidy_hoarder(&i).is_some());
        // 50% misses.
        i.storage_used_percent = Some(50);
        assert!(tidy_hoarder(&i).is_none());
        // Total below threshold misses even at low usage.
        i.storage_used_percent = Some(20);
        i.storage_total_gb = Some(128);
        assert!(tidy_hoarder(&i).is_none());
    }

    #[test]
    fn speed_demon_at_latest_major() {
        let mut i = base_input();
        i.ios_major = Some(super::super::data::LATEST_IOS_MAJOR);
        assert!(speed_demon(&i).is_some());
        i.ios_major = Some(super::super::data::LATEST_IOS_MAJOR - 1);
        assert!(speed_demon(&i).is_none());
    }

    /// All 18 priorities are distinct; sorting must be total.
    #[test]
    fn priorities_are_unique_and_dense_1_to_18() {
        use std::collections::BTreeSet;
        let all = [
            BadgeId::Untouchable,
            BadgeId::BatteryChamp,
            BadgeId::ChargingWizard,
            BadgeId::OgOwner,
            BadgeId::DayOne,
            BadgeId::Survivor,
            BadgeId::Veteran,
            BadgeId::StorageTitan,
            BadgeId::MaxedOut,
            BadgeId::HeavyCycle,
            BadgeId::BetaTester,
            BadgeId::BackupOverdue,
            BadgeId::Minimalist,
            BadgeId::AppCollector,
            BadgeId::ProMaxClub,
            BadgeId::TidyHoarder,
            BadgeId::BackupFresh,
            BadgeId::SpeedDemon,
        ];
        let priorities: BTreeSet<u8> = all.iter().copied().map(priority_for).collect();
        assert_eq!(priorities.len(), 18, "priorities must all be distinct");
        assert_eq!(*priorities.iter().min().unwrap(), 1);
        assert_eq!(*priorities.iter().max().unwrap(), 18);
    }

    #[test]
    fn untouchable_only_fires_at_100() {
        let mut i = base_input();
        i.battery_health_percent = Some(100);
        assert!(untouchable(&i).is_some());
        i.battery_health_percent = Some(99);
        assert!(untouchable(&i).is_none());
    }

    #[test]
    fn charging_wizard_needs_cycles_and_health() {
        let mut i = base_input();
        i.battery_cycle_count = Some(200);
        i.battery_health_percent = Some(95);
        assert!(charging_wizard(&i).is_some());
        i.battery_cycle_count = Some(199);
        assert!(charging_wizard(&i).is_none());
        i.battery_cycle_count = Some(200);
        i.battery_health_percent = Some(94);
        assert!(charging_wizard(&i).is_none());
    }

    #[test]
    fn day_one_requires_paired_year_equals_release_year() {
        let mut i = base_input();
        i.paired_year = Some(2023);
        i.model_release_year = Some(2023);
        assert!(day_one(&i).is_some());
        i.paired_year = Some(2024);
        assert!(day_one(&i).is_none());
        i.model_release_year = None;
        assert!(day_one(&i).is_none());
    }

    #[test]
    fn next_badge_hint_prefers_single_action_over_count_grind() {
        // Both BackupFresh (action: back up) and HeavyCycle (count: cycles)
        // are reachable. The single-action one must win — locks in the
        // "smallest-effort first" priority order.
        let mut i = base_input();
        i.backup_age_days = Some(10); // 8..=30 → action
        i.battery_cycle_count = Some(270); // 250..300 → count
        let hint = next_badge_hint(&i, &[]).expect("both reachable");
        assert!(hint.contains("Backup Fresh"), "got `{hint}`");
        assert!(!hint.contains("Heavy Cycle"));
    }

    #[test]
    fn next_badge_hint_skips_already_earned() {
        let mut i = base_input();
        i.battery_cycle_count = Some(270); // would normally trigger HeavyCycle
                                           // Pretend HeavyCycle is already earned — hint must look elsewhere.
        let hint = next_badge_hint(&i, &[BadgeId::HeavyCycle]);
        assert!(
            hint.as_deref().is_none_or(|h| !h.contains("Heavy Cycle")),
            "got `{hint:?}`"
        );
    }

    #[test]
    fn next_badge_hint_respects_reachability_thresholds() {
        // 249 cycles is below the "near reach" window for HeavyCycle (250..300).
        // Far-away grinds shouldn't nag.
        let mut i = base_input();
        i.battery_cycle_count = Some(249);
        i.backup_age_days = None;
        i.app_count = None;
        i.battery_health_percent = None;
        i.ios_major = None;
        assert!(next_badge_hint(&i, &[]).is_none());
        // 299 sits at the top of the window — must still fire.
        i.battery_cycle_count = Some(299);
        let hint = next_badge_hint(&i, &[]).expect("299 is reachable");
        assert!(hint.contains("1 cycles to Heavy Cycle"), "got `{hint}`");
    }

    #[test]
    fn ranking_picks_top_three_by_priority() {
        // A "veteran fan with 1TB" device qualifying for: battery_champ,
        // og_owner, survivor (covered by veteran logic if we made age 3),
        // storage_titan, pro_max_club. Expect priority 1, 2, 3 (or 5 if 3 missing).
        let i = BadgeInputs {
            battery_health_percent: Some(95),
            battery_cycle_count: Some(50),
            age_years: Some(5),
            paired_year: Some(2019),
            app_count: Some(100),
            storage_used_percent: Some(40),
            storage_total_gb: Some(1_024),
            backup_age_days: Some(3),
            is_beta_build: false,
            ios_major: Some(super::super::data::LATEST_IOS_MAJOR),
            model_friendly: Some("iPhone 15 Pro Max".into()),
            model_release_year: Some(2022),
        };
        let all = evaluate_all(&i);
        assert!(
            all.len() >= 4,
            "fixture should qualify for ≥4 badges, got {}",
            all.len()
        );
        let top = top_three(all);
        assert_eq!(top.len(), 3);
        let ids: Vec<_> = top.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            vec![BadgeId::BatteryChamp, BadgeId::OgOwner, BadgeId::Survivor],
            "top 3 by priority should be 1, 2, 3"
        );
    }
}
