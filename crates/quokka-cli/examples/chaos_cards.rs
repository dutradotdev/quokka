//! Chaos QA harness for `qk card`.
//!
//! Builds many `CardData` scenarios with realistic but extreme/edge inputs
//! (real iPhone storage tiers, real battery limits, real iOS versions),
//! renders each to /tmp/qk-chaos/<name>.png so they can be eyeballed.

use quokka_cli::commands::card::badges::{Badge, BadgeColor, BadgeId};
use quokka_cli::commands::card::data::{
    AppsJailbreakLabel, CardData, HealthTier, StorageBreakdownRows, StorageFallback, TopApp,
};
use quokka_cli::commands::card::{png, render};

fn gb(n: u64) -> u64 {
    n * 1_000_000_000
}

fn fmt_gigs(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
}

fn fmt_app_size(bytes: u64) -> String {
    if bytes < 1_000_000_000 {
        let mb = (bytes as f64 / 1_000_000.0).round() as u64;
        format!("{mb} MB")
    } else {
        fmt_gigs(bytes)
    }
}

fn make_breakdown(camera: u64, apps: u64, other: u64, free: u64) -> StorageBreakdownRows {
    let denom = camera + apps + other;
    let cells = |part: u64| -> u8 {
        if denom == 0 {
            0
        } else {
            ((part as u128 * 11) / denom as u128).min(11) as u8
        }
    };
    StorageBreakdownRows {
        camera_label: fmt_gigs(camera),
        apps_label: fmt_gigs(apps),
        other_label: fmt_gigs(other),
        free_label: fmt_gigs(free),
        camera_cells: cells(camera),
        apps_cells: cells(apps),
        other_cells: cells(other),
    }
}

fn rank_apps_realistic(apps: Vec<(&str, u64)>) -> Vec<TopApp> {
    let max = apps.iter().map(|(_, b)| *b).max().unwrap_or(1);
    apps.into_iter()
        .map(|(name, bytes)| {
            // Mirrors data::truncate_app_name (14 chars max for the narrow col)
            let chars: Vec<char> = name.chars().collect();
            let display_name = if chars.len() <= 14 {
                name.to_string()
            } else {
                let mut s: String = chars[..13].iter().collect();
                s.push('…');
                s
            };
            let mut cells = if max == 0 {
                0u8
            } else {
                ((bytes as u128 * 11) / max as u128).min(11) as u8
            };
            if cells == 0 && bytes > 0 {
                cells = 1;
            }
            TopApp {
                display_name,
                size_label: fmt_app_size(bytes),
                bar_cells: cells,
            }
        })
        .collect()
}

fn badge(id: BadgeId, title: &'static str, sub: &'static str, c: BadgeColor) -> Badge {
    Badge {
        id,
        title,
        subtitle: sub,
        color: c,
    }
}

fn base_card() -> CardData {
    CardData {
        model_friendly: Some("iPhone 14 Pro Max".into()),
        chip_name: Some("A16 Bionic".into()),
        storage_label: Some("256 GB".into()),
        enclosure_color: Some("Deep Purple".into()),
        header_caption: Some("Veteran · Top-tier · Disciplined".into()),
        battery_level_percent: Some(78),
        battery_cycle_count: Some(412),
        battery_health_percent: Some(89),
        battery_health_tier: HealthTier::Good,
        storage_breakdown_rows: Some(make_breakdown(gb(84), gb(35), gb(18), gb(119))),
        storage_fallback: None,
        ios_label: "iOS 18.2 (22C152)".into(),
        ios_beta_suffix: None,
        app_count: Some(187),
        apps_jailbreak_label: AppsJailbreakLabel::Pristine,
        first_seen_line: Some("Aug 2023 · Spotify is your oldest".into()),
        backup_age_label: Some("4 days ago".into()),
        top_apps: Some(rank_apps_realistic(vec![
            ("WhatsApp Messenger", gb(7)),
            ("Instagram", gb(4)),
            ("TikTok", gb(3)),
            ("YouTube", gb(2)),
            ("Spotify", gb(1)),
        ])),
        badges: vec![
            badge(
                BadgeId::BatteryChamp,
                "Battery Champ",
                "90%+ after 3+ years",
                BadgeColor::Good,
            ),
            badge(
                BadgeId::Veteran,
                "Veteran",
                "3+ years in service",
                BadgeColor::Warn,
            ),
            badge(
                BadgeId::ProMaxClub,
                "Pro Max Club",
                "top-tier model",
                BadgeColor::Info,
            ),
        ],
        next_badge_hint: None,
        footer_date: "May 27".into(),
        footer_cta: "star us: github.com/dutradotdev/quokka",
        redact: false,
    }
}

type Scenario = (&'static str, Box<dyn Fn() -> CardData>);

fn scenarios() -> Vec<Scenario> {
    vec![
        // ----------------- 1. happy baseline -----------------
        ("01_baseline", Box::new(base_card)),
        // ----------------- 2. fresh-out-of-box iPhone -----------------
        (
            "02_fresh_device",
            Box::new(|| CardData {
                model_friendly: Some("iPhone 16 Pro Max".into()),
                chip_name: Some("A18 Pro".into()),
                storage_label: Some("256 GB".into()),
                enclosure_color: Some("Desert Titanium".into()),
                header_caption: Some("Battery legend · Day one".into()),
                battery_level_percent: Some(98),
                battery_cycle_count: Some(3),
                battery_health_percent: Some(100),
                battery_health_tier: HealthTier::Good,
                storage_breakdown_rows: Some(make_breakdown(gb(1), gb(8), gb(12), gb(235))),
                storage_fallback: None,
                ios_label: "iOS 18.2 (22C152)".into(),
                ios_beta_suffix: None,
                app_count: Some(12),
                apps_jailbreak_label: AppsJailbreakLabel::Pristine,
                first_seen_line: None,
                backup_age_label: Some("today".into()),
                top_apps: Some(rank_apps_realistic(vec![
                    ("Messages", gb(2)),
                    ("Photos", gb(1)),
                    ("Maps", gb(1)),
                    ("Safari", gb(1)),
                    ("Mail", gb(1)),
                ])),
                badges: vec![
                    badge(
                        BadgeId::Untouchable,
                        "Untouchable",
                        "battery at 100%",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::DayOne,
                        "Day One",
                        "paired in release year",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::ProMaxClub,
                        "Pro Max Club",
                        "top-tier model",
                        BadgeColor::Info,
                    ),
                ],
                next_badge_hint: None,
                footer_date: "May 27".into(),
                footer_cta: "star us: github.com/dutradotdev/quokka",
                redact: false,
            }),
        ),
        // ----------------- 3. ancient iPhone, hammered -----------------
        (
            "03_ancient_battered",
            Box::new(|| CardData {
                model_friendly: Some("iPhone X".into()),
                chip_name: Some("A11 Bionic".into()),
                storage_label: Some("64 GB".into()),
                enclosure_color: Some("Silver".into()),
                header_caption: Some("Survivor · Heavy charger".into()),
                battery_level_percent: Some(34),
                battery_cycle_count: Some(1843),
                battery_health_percent: Some(62),
                battery_health_tier: HealthTier::Bad,
                storage_breakdown_rows: Some(make_breakdown(gb(28), gb(22), gb(8), gb(2))),
                storage_fallback: None,
                ios_label: "iOS 16.7.10".into(),
                ios_beta_suffix: None,
                app_count: Some(94),
                apps_jailbreak_label: AppsJailbreakLabel::Pristine,
                first_seen_line: Some("Feb 2018 · Spotify is your oldest".into()),
                backup_age_label: Some("8 months ago".into()),
                top_apps: Some(rank_apps_realistic(vec![
                    ("WhatsApp Messenger", gb(6)),
                    ("Photos", gb(4)),
                    ("Facebook", gb(3)),
                    ("Messenger", gb(2)),
                    ("Chrome", gb(1)),
                ])),
                badges: vec![
                    badge(
                        BadgeId::Survivor,
                        "Survivor",
                        "7+ years in service",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::HeavyCycle,
                        "Heavy Charger",
                        "1000+ cycles",
                        BadgeColor::Warn,
                    ),
                    badge(
                        BadgeId::BackupOverdue,
                        "Backup Overdue",
                        "30+ days since backup",
                        BadgeColor::Bad,
                    ),
                ],
                next_badge_hint: None,
                footer_date: "May 27".into(),
                footer_cta: "star us: github.com/dutradotdev/quokka",
                redact: false,
            }),
        ),
        // ----------------- 4. iPhone SE basic, tight storage -----------------
        (
            "04_se_maxed",
            Box::new(|| CardData {
                model_friendly: Some("iPhone SE (3rd generation)".into()),
                chip_name: Some("A15 Bionic".into()),
                storage_label: Some("64 GB".into()),
                enclosure_color: Some("Starlight".into()),
                header_caption: Some("Tidy".into()),
                battery_level_percent: Some(12),
                battery_cycle_count: Some(287),
                battery_health_percent: Some(94),
                battery_health_tier: HealthTier::Good,
                storage_breakdown_rows: Some(make_breakdown(gb(28), gb(24), gb(11), gb(1))),
                storage_fallback: None,
                ios_label: "iOS 17.6.1".into(),
                ios_beta_suffix: None,
                app_count: Some(63),
                apps_jailbreak_label: AppsJailbreakLabel::Pristine,
                first_seen_line: Some("Sep 2023 · WhatsApp Messenger is your oldest".into()),
                backup_age_label: Some("3 weeks ago".into()),
                top_apps: Some(rank_apps_realistic(vec![
                    ("WhatsApp Messenger", gb(5)),
                    ("Photos", gb(5)),
                    ("Telegram Messenger", gb(2)),
                    ("Notes", gb(1)),
                    ("Apple Music", gb(1)),
                ])),
                badges: vec![
                    badge(
                        BadgeId::MaxedOut,
                        "Maxed Out",
                        "95%+ storage used",
                        BadgeColor::Bad,
                    ),
                    badge(BadgeId::TidyHoarder, "Tidy", "<100 apps", BadgeColor::Good),
                    badge(
                        BadgeId::BackupFresh,
                        "Disciplined",
                        "backed up this week",
                        BadgeColor::Good,
                    ),
                ],
                next_badge_hint: None,
                footer_date: "May 27".into(),
                footer_cta: "star us: github.com/dutradotdev/quokka",
                redact: false,
            }),
        ),
        // ----------------- 5. one huge app dominates the bars -----------------
        (
            "05_one_dominant_app",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("CapCut", gb(62)),
                    ("Photos", gb(3)),
                    ("WhatsApp Messenger", gb(2)),
                    ("Instagram", gb(2)),
                    ("YouTube", gb(1)),
                ]));
                c
            }),
        ),
        // ----------------- 6. apps with multibyte names -----------------
        (
            "06_multibyte_apps",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("カメラ", gb(8)),
                    ("LINE", gb(4)),
                    ("쿠팡 - Coupang", gb(3)),
                    ("微信", gb(2)),
                    ("Google 翻訳", gb(1)),
                ]));
                c.first_seen_line = Some("Mar 2022 · カメラ is your oldest".into());
                c
            }),
        ),
        // ----------------- 7. extra-long app name (truncates) -----------------
        (
            "07_long_app_name",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("Microsoft PowerPoint for iOS", gb(3)),
                    ("Adobe Lightroom Photo Editor", gb(2)),
                    ("Some Very Long Banking App Name 2024", gb(2)),
                    ("Google Maps - Transit & Food", gb(1)),
                    ("Reddit: Trending Discussions", gb(1)),
                ]));
                c
            }),
        ),
        // ----------------- 8. asymmetric breakdown (camera ~99%) -----------------
        (
            "08_camera_heavy",
            Box::new(|| {
                let mut c = base_card();
                c.storage_breakdown_rows = Some(make_breakdown(gb(220), gb(2), gb(2), gb(32)));
                c
            }),
        ),
        // ----------------- 9. storage fallback (no breakdown) -----------------
        (
            "09_storage_fallback",
            Box::new(|| {
                let mut c = base_card();
                c.storage_breakdown_rows = None;
                c.storage_fallback = Some(StorageFallback {
                    used_label: fmt_gigs(gb(137)),
                    total_label: fmt_gigs(gb(256)),
                    free_label: fmt_gigs(gb(119)),
                    used_percent: 54,
                });
                c
            }),
        ),
        // ----------------- 10. jailbroken + beta + bad backup -----------------
        (
            "10_jailbroken_beta",
            Box::new(|| {
                let mut c = base_card();
                c.apps_jailbreak_label = AppsJailbreakLabel::Jailbroken;
                c.ios_beta_suffix = Some(" · beta");
                c.ios_label = "iOS 18.4 (22E5200n)".into();
                c.backup_age_label = Some("4 months ago".into());
                c.header_caption = Some("Beta tester".into());
                c.badges = vec![
                    badge(
                        BadgeId::BetaTester,
                        "Beta Tester",
                        "running iOS beta",
                        BadgeColor::Info,
                    ),
                    badge(
                        BadgeId::BackupOverdue,
                        "Backup Overdue",
                        "30+ days since backup",
                        BadgeColor::Bad,
                    ),
                    badge(
                        BadgeId::AppCollector,
                        "Power User",
                        "200+ apps",
                        BadgeColor::Info,
                    ),
                ];
                c
            }),
        ),
        // ----------------- 11. unknown battery health (None everywhere) -----------------
        (
            "11_battery_unknown",
            Box::new(|| {
                let mut c = base_card();
                c.battery_level_percent = None;
                c.battery_cycle_count = None;
                c.battery_health_percent = None;
                c.battery_health_tier = HealthTier::Unknown;
                c
            }),
        ),
        // ----------------- 12. no badges qualify -----------------
        (
            "12_no_badges",
            Box::new(|| {
                let mut c = base_card();
                c.badges = vec![];
                c.header_caption = None;
                c
            }),
        ),
        // ----------------- 13. minimal info (everything None we allow) -----------------
        (
            "13_minimal",
            Box::new(|| CardData {
                model_friendly: None,
                chip_name: None,
                storage_label: None,
                enclosure_color: None,
                header_caption: None,
                battery_level_percent: None,
                battery_cycle_count: None,
                battery_health_percent: None,
                battery_health_tier: HealthTier::Unknown,
                storage_breakdown_rows: None,
                storage_fallback: None,
                ios_label: "iOS —".into(),
                ios_beta_suffix: None,
                app_count: None,
                apps_jailbreak_label: AppsJailbreakLabel::None,
                first_seen_line: None,
                backup_age_label: None,
                top_apps: None,
                badges: vec![],
                next_badge_hint: None,
                footer_date: "May 27".into(),
                footer_cta: "star us: github.com/dutradotdev/quokka",
                redact: false,
            }),
        ),
        // ----------------- 14. redacted mode -----------------
        (
            "14_redacted",
            Box::new(|| {
                let mut c = base_card();
                c.redact = true;
                c.ios_label = "iOS 18.2".into();
                c.first_seen_line = Some("2023".into());
                c.backup_age_label = Some("a few weeks ago".into());
                c
            }),
        ),
        // ----------------- 15. XSS / special char names -----------------
        (
            "15_xml_special",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("Q&A Notebook", gb(3)),
                    ("\"Quoted\" App", gb(2)),
                    ("<script>alert</script>", gb(1)),
                    ("Tim's Notes", gb(1)),
                    ("A > B Calculator", gb(1)),
                ]));
                c.first_seen_line = Some("Mar 2022 · Q&A Notebook is your oldest".into());
                c
            }),
        ),
        // ----------------- 16. enclosure color is a short opaque code (filtered) -----------------
        (
            "16_short_enclosure",
            Box::new(|| {
                let mut c = base_card();
                // Renderer-side this would be filtered out by `project`, but we
                // already have the card built. Simulate the filtered output:
                c.enclosure_color = None;
                c
            }),
        ),
        // ----------------- 17. enclosure color with long name kept -----------------
        (
            "17_sierra_blue",
            Box::new(|| {
                let mut c = base_card();
                c.enclosure_color = Some("Sierra Blue".into());
                c.model_friendly = Some("iPhone 13 Pro".into());
                c.chip_name = Some("A15 Bionic".into());
                c
            }),
        ),
        // ----------------- 18. tiny apps + few apps minimalist -----------------
        (
            "18_minimalist",
            Box::new(|| {
                let mut c = base_card();
                c.app_count = Some(7);
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("Messages", gb(1)),
                    ("Mail", gb(1)),
                    ("Safari", gb(1)),
                    ("Notes", 700_000_000),
                    ("Calculator", 50_000_000),
                ]));
                c.header_caption = Some("Minimalist".into());
                c.badges = vec![
                    badge(
                        BadgeId::Minimalist,
                        "Minimalist",
                        "<10 apps",
                        BadgeColor::Info,
                    ),
                    badge(
                        BadgeId::BatteryChamp,
                        "Battery Champ",
                        "90%+ after 3+ years",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::BackupFresh,
                        "Disciplined",
                        "backed up this week",
                        BadgeColor::Good,
                    ),
                ];
                c
            }),
        ),
        // ----------------- 19. emoji in app name -----------------
        (
            "19_emoji_app",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("🌟 SuperFavs", gb(2)),
                    ("Pokémon GO", gb(3)),
                    ("Café Notes ☕", gb(1)),
                    ("Spotify", gb(2)),
                    ("Apple Music", gb(1)),
                ]));
                c
            }),
        ),
        // ----------------- 20. RTL / arabic app name -----------------
        (
            "20_rtl_app",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("الفجر للمواقيت", gb(2)),
                    ("Telegram Messenger", gb(3)),
                    ("WhatsApp Messenger", gb(5)),
                    ("Photos", gb(2)),
                    ("Notes", gb(1)),
                ]));
                c
            }),
        ),
        // ----------------- 21. all badges-good superuser -----------------
        (
            "21_all_good_badges",
            Box::new(|| {
                let mut c = base_card();
                c.badges = vec![
                    badge(
                        BadgeId::Untouchable,
                        "Untouchable",
                        "battery at 100%",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::BatteryChamp,
                        "Battery Champ",
                        "90%+ after 3+ years",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::ChargingWizard,
                        "Charging Wizard",
                        "200+ cycles @ 95%+",
                        BadgeColor::Good,
                    ),
                ];
                c.header_caption = Some("Battery legend · Battery champ · Charging wizard".into());
                c
            }),
        ),
        // ----------------- 22. 1TB Pro Max stuffed -----------------
        (
            "22_1tb_promax",
            Box::new(|| {
                let mut c = base_card();
                c.model_friendly = Some("iPhone 16 Pro Max".into());
                c.chip_name = Some("A18 Pro".into());
                c.storage_label = Some("1 TB".into());
                c.enclosure_color = Some("Black Titanium".into());
                c.storage_breakdown_rows = Some(make_breakdown(gb(412), gb(180), gb(95), gb(313)));
                c.app_count = Some(534);
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("CapCut", gb(48)),
                    ("Adobe Lightroom Photo Editor", gb(22)),
                    ("LumaFusion", gb(14)),
                    ("Procreate Pocket", gb(8)),
                    ("Spotify", gb(6)),
                ]));
                c
            }),
        ),
        // ----------------- 23. backup today / disciplined -----------------
        (
            "23_backup_today",
            Box::new(|| {
                let mut c = base_card();
                c.backup_age_label = Some("today".into());
                c
            }),
        ),
        // ----------------- 24. odd ios version "iOS 26.0" + speed demon -----------------
        (
            "24_ios26",
            Box::new(|| {
                let mut c = base_card();
                c.ios_label = "iOS 26.0 (23A340)".into();
                c.header_caption = Some("Up to date · Disciplined".into());
                c
            }),
        ),
        // ----------------- 25. backup overdue + maxed + jailbroken nightmare -----------------
        (
            "25_triple_bad",
            Box::new(|| {
                let mut c = base_card();
                c.apps_jailbreak_label = AppsJailbreakLabel::Jailbroken;
                c.backup_age_label = Some("11 months ago".into());
                c.storage_breakdown_rows = Some(make_breakdown(gb(118), gb(60), gb(76), gb(2)));
                c.battery_health_percent = Some(54);
                c.battery_health_tier = HealthTier::Bad;
                c.battery_level_percent = Some(3);
                c.battery_cycle_count = Some(1402);
                c.badges = vec![
                    badge(
                        BadgeId::MaxedOut,
                        "Maxed Out",
                        "95%+ storage used",
                        BadgeColor::Bad,
                    ),
                    badge(
                        BadgeId::BackupOverdue,
                        "Backup Overdue",
                        "30+ days since backup",
                        BadgeColor::Bad,
                    ),
                    badge(
                        BadgeId::HeavyCycle,
                        "Heavy Charger",
                        "1000+ cycles",
                        BadgeColor::Warn,
                    ),
                ];
                c.header_caption = None;
                c
            }),
        ),
        // ----------------- 26. top_apps Some(vec![]) — empty enrichment -----------------
        (
            "26_empty_top_apps",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(vec![]);
                c
            }),
        ),
        // ----------------- 27. battery exactly 0%, dead -----------------
        (
            "27_battery_zero",
            Box::new(|| {
                let mut c = base_card();
                c.battery_level_percent = Some(0);
                c.battery_cycle_count = Some(0);
                c.battery_health_percent = Some(100);
                c.battery_health_tier = HealthTier::Good;
                c
            }),
        ),
        // ----------------- 28. very long oldest_app + bottom-row -----------------
        (
            "28_long_oldest",
            Box::new(|| {
                let mut c = base_card();
                c.first_seen_line =
                    Some("Mar 2022 · Microsoft PowerPoint for iOS is your oldest".into());
                c
            }),
        ),
        // ----------------- 29. jailbroken without app_count (None) -----------------
        (
            "29_jb_no_count",
            Box::new(|| {
                let mut c = base_card();
                c.app_count = None;
                c.apps_jailbreak_label = AppsJailbreakLabel::None;
                c
            }),
        ),
        // ----------------- 30. only 1 top app + long iOS label -----------------
        (
            "30_one_app_long_ios",
            Box::new(|| {
                let mut c = base_card();
                c.ios_label = "iOS 18.5.1 (22F5076a) [Public Beta]".into();
                c.ios_beta_suffix = Some(" · beta");
                c.top_apps = Some(rank_apps_realistic(vec![("Messages", gb(2))]));
                c
            }),
        ),
        // ----------------- 31. weird unicode in enclosure_color -----------------
        (
            "31_weird_color",
            Box::new(|| {
                let mut c = base_card();
                c.enclosure_color = Some("Midnight—Black".into());
                c
            }),
        ),
        // ----------------- 32. huge cycle count (5-digit) -----------------
        (
            "32_huge_cycles",
            Box::new(|| {
                let mut c = base_card();
                c.battery_cycle_count = Some(2847);
                c.battery_health_percent = Some(58);
                c.battery_health_tier = HealthTier::Bad;
                c.battery_level_percent = Some(46);
                c
            }),
        ),
        // ----------------- 33. all 5 top apps tied at same size -----------------
        (
            "33_tied_apps",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("Spotify", gb(2)),
                    ("YouTube", gb(2)),
                    ("Photos", gb(2)),
                    ("Notes", gb(2)),
                    ("Mail", gb(2)),
                ]));
                c
            }),
        ),
        // ----------------- 34. backup today + cycle 1 + 100% -----------------
        (
            "34_pristine",
            Box::new(|| {
                let mut c = base_card();
                c.battery_health_percent = Some(100);
                c.battery_health_tier = HealthTier::Good;
                c.battery_level_percent = Some(100);
                c.battery_cycle_count = Some(1);
                c.backup_age_label = Some("today".into());
                c.app_count = Some(0);
                c.top_apps = None;
                c
            }),
        ),
        // ----------------- 35. enclosure_color = "" empty string -----------------
        (
            "35_empty_color",
            Box::new(|| {
                let mut c = base_card();
                c.enclosure_color = Some("".into());
                c.chip_name = None;
                c.storage_label = None;
                c
            }),
        ),
        // ----------------- 36. all 4 badge color tones in one card -----------------
        (
            "36_mixed_badge_colors",
            Box::new(|| {
                let mut c = base_card();
                c.badges = vec![
                    badge(
                        BadgeId::Untouchable,
                        "Untouchable",
                        "battery at 100%",
                        BadgeColor::Good,
                    ),
                    badge(
                        BadgeId::HeavyCycle,
                        "Heavy Charger",
                        "1000+ cycles",
                        BadgeColor::Warn,
                    ),
                    badge(
                        BadgeId::MaxedOut,
                        "Maxed Out",
                        "95%+ storage used",
                        BadgeColor::Bad,
                    ),
                ];
                c
            }),
        ),
        // ----------------- 37. middle Warn battery tier -----------------
        (
            "37_warn_battery",
            Box::new(|| {
                let mut c = base_card();
                c.battery_health_percent = Some(78);
                c.battery_health_tier = HealthTier::Warn;
                c.battery_level_percent = Some(42);
                c.battery_cycle_count = Some(680);
                c
            }),
        ),
        // ----------------- 38. category=0 breakdown row (apps = 0) -----------------
        (
            "38_zero_category",
            Box::new(|| {
                let mut c = base_card();
                c.storage_breakdown_rows = Some(make_breakdown(gb(180), 0, gb(15), gb(61)));
                c
            }),
        ),
        // ----------------- 39. vibe of just 1 trait word -----------------
        (
            "39_single_vibe",
            Box::new(|| {
                let mut c = base_card();
                c.header_caption = Some("Minimalist".into());
                c
            }),
        ),
        // ----------------- 40. very long backup label (rare months) -----------------
        (
            "40_long_backup",
            Box::new(|| {
                let mut c = base_card();
                c.backup_age_label = Some("18 months ago".into());
                c
            }),
        ),
        // ----------------- 41. brand new device, no storage data at all -----------------
        (
            "41_no_storage_info",
            Box::new(|| {
                let mut c = base_card();
                c.storage_label = None;
                c.storage_breakdown_rows = None;
                c.storage_fallback = None;
                c.top_apps = None;
                c
            }),
        ),
        // ----------------- 42. all top apps with 0 bytes (weird) -----------------
        (
            "42_zero_bytes_apps",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("Calculator", 0),
                    ("Stocks", 0),
                    ("Tips", 0),
                    ("Find My", 0),
                    ("Clock", 0),
                ]));
                c
            }),
        ),
        // ----------------- 43. iPhone Mini 13 with verbose info -----------------
        (
            "43_mini_full",
            Box::new(|| {
                let mut c = base_card();
                c.model_friendly = Some("iPhone 13 mini".into());
                c.chip_name = Some("A15 Bionic".into());
                c.storage_label = Some("128 GB".into());
                c.enclosure_color = Some("(PRODUCT)RED".into());
                c.app_count = Some(243);
                c
            }),
        ),
        // ----------------- 44. only Storage Titan + Pro Max Club (Info+Info) -----------------
        (
            "44_two_info_badges",
            Box::new(|| {
                let mut c = base_card();
                c.badges = vec![
                    badge(
                        BadgeId::StorageTitan,
                        "Storage Titan",
                        "1 TB tier",
                        BadgeColor::Info,
                    ),
                    badge(
                        BadgeId::ProMaxClub,
                        "Pro Max Club",
                        "top-tier model",
                        BadgeColor::Info,
                    ),
                ];
                c
            }),
        ),
        // ----------------- 46. realistic MB+GB mix in top apps -----------------
        (
            "46_mb_gb_mix",
            Box::new(|| {
                let mut c = base_card();
                c.top_apps = Some(rank_apps_realistic(vec![
                    ("WhatsApp Messenger", gb(7) + 200_000_000),
                    ("Instagram", 1_400_000_000),
                    ("Photos", 543_000_000),
                    ("Spotify", 287_000_000),
                    ("Calculator", 5_000_000),
                ]));
                c
            }),
        ),
        // ----------------- 45. backup yesterday + 4yr device -----------------
        (
            "45_yesterday_backup",
            Box::new(|| {
                let mut c = base_card();
                c.backup_age_label = Some("yesterday".into());
                c.battery_cycle_count = Some(1);
                c
            }),
        ),
    ]
}

fn main() {
    let out_dir = std::path::Path::new("/tmp/qk-chaos");
    std::fs::create_dir_all(out_dir).expect("create out dir");
    let scenarios = scenarios();
    println!(
        "Rendering {} scenarios to {}",
        scenarios.len(),
        out_dir.display()
    );
    let mut failed = 0;
    for (name, build) in scenarios {
        let card = build();
        let svg = render::render_svg(&card);
        match png::svg_to_png(&svg) {
            Ok(bytes) => {
                let path = out_dir.join(format!("{name}.png"));
                std::fs::write(&path, &bytes).expect("write png");
                println!("  ok  {}", path.display());
            }
            Err(e) => {
                failed += 1;
                let svg_path = out_dir.join(format!("{name}.failed.svg"));
                let _ = std::fs::write(&svg_path, svg);
                eprintln!("  ERR {name}: {e} (svg dumped to {})", svg_path.display());
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
