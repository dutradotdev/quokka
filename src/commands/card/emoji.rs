//! Twemoji SVG bytes, bundled at compile-time, keyed by badge id.
//!
//! Each Twemoji file is the canonical `<svg xmlns viewBox="0 0 36 36">…
//! </svg>` form. To embed one inside the card we strip the outer `<svg>`
//! opener/closer and re-wrap with our own positioning. The result is a
//! nested SVG that resvg renders with full colour at the chosen size.
//!
//! Source: <https://github.com/jdecked/twemoji> (mirror of the original
//! Twitter Twemoji release, CC-BY-4.0).

use super::badges::BadgeId;

const UNTOUCHABLE: &str = include_str!("../../../assets/emoji/untouchable.svg");
const BATTERY_CHAMP: &str = include_str!("../../../assets/emoji/battery_champ.svg");
const CHARGING_WIZARD: &str = include_str!("../../../assets/emoji/charging_wizard.svg");
const OG_OWNER: &str = include_str!("../../../assets/emoji/og_owner.svg");
const DAY_ONE: &str = include_str!("../../../assets/emoji/day_one.svg");
const SURVIVOR: &str = include_str!("../../../assets/emoji/survivor.svg");
const VETERAN: &str = include_str!("../../../assets/emoji/veteran.svg");
const STORAGE_TITAN: &str = include_str!("../../../assets/emoji/storage_titan.svg");
const MAXED_OUT: &str = include_str!("../../../assets/emoji/maxed_out.svg");
const HEAVY_CYCLE: &str = include_str!("../../../assets/emoji/heavy_cycle.svg");
const BETA_TESTER: &str = include_str!("../../../assets/emoji/beta_tester.svg");
const BACKUP_OVERDUE: &str = include_str!("../../../assets/emoji/backup_overdue.svg");
const MINIMALIST: &str = include_str!("../../../assets/emoji/minimalist.svg");
const APP_COLLECTOR: &str = include_str!("../../../assets/emoji/app_collector.svg");
const PRO_MAX_CLUB: &str = include_str!("../../../assets/emoji/pro_max_club.svg");
const TIDY_HOARDER: &str = include_str!("../../../assets/emoji/tidy_hoarder.svg");
const BACKUP_FRESH: &str = include_str!("../../../assets/emoji/backup_fresh.svg");
const SPEED_DEMON: &str = include_str!("../../../assets/emoji/speed_demon.svg");

/// Raw Twemoji SVG bytes for a badge. Always returns the full
/// `<svg>…</svg>` document — the caller strips the wrapper.
pub fn raw_svg(id: BadgeId) -> &'static str {
    match id {
        BadgeId::Untouchable => UNTOUCHABLE,
        BadgeId::BatteryChamp => BATTERY_CHAMP,
        BadgeId::ChargingWizard => CHARGING_WIZARD,
        BadgeId::OgOwner => OG_OWNER,
        BadgeId::DayOne => DAY_ONE,
        BadgeId::Survivor => SURVIVOR,
        BadgeId::Veteran => VETERAN,
        BadgeId::StorageTitan => STORAGE_TITAN,
        BadgeId::MaxedOut => MAXED_OUT,
        BadgeId::HeavyCycle => HEAVY_CYCLE,
        BadgeId::BetaTester => BETA_TESTER,
        BadgeId::BackupOverdue => BACKUP_OVERDUE,
        BadgeId::Minimalist => MINIMALIST,
        BadgeId::AppCollector => APP_COLLECTOR,
        BadgeId::ProMaxClub => PRO_MAX_CLUB,
        BadgeId::TidyHoarder => TIDY_HOARDER,
        BadgeId::BackupFresh => BACKUP_FRESH,
        BadgeId::SpeedDemon => SPEED_DEMON,
    }
}

/// Build a nested `<svg>` element that draws the badge's emoji at
/// `(x, y)` sized to `size_px × size_px`. The original Twemoji viewBox
/// (`0 0 36 36`) is preserved so the inner geometry scales correctly.
///
/// Returns the inline SVG string ready to concatenate into the parent
/// card document.
pub fn render(id: BadgeId, x: i32, y: i32, size_px: u32) -> String {
    let raw = raw_svg(id);
    let inner = extract_inner(raw);
    format!(
        r#"<svg x="{x}" y="{y}" width="{size_px}" height="{size_px}" viewBox="0 0 36 36" xmlns="http://www.w3.org/2000/svg">{inner}</svg>"#,
    )
}

/// Strip a Twemoji document's outer `<svg …>` opener and the matching
/// `</svg>` closer, leaving just the paths/shapes inside. Falls back to
/// the input verbatim if the document doesn't match the expected shape
/// (no panics — a misformatted asset shows up as garbled output, not a
/// crashed renderer).
fn extract_inner(svg: &str) -> &str {
    let opening_end = match svg.find('>') {
        Some(i) => i + 1,
        None => return svg,
    };
    let body = &svg[opening_end..];
    match body.rfind("</svg>") {
        Some(i) => &body[..i],
        None => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_badge_has_a_non_empty_svg() {
        for id in [
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
        ] {
            let raw = raw_svg(id);
            assert!(raw.starts_with("<svg"), "{id:?} bundle is not an SVG");
            assert!(raw.contains("viewBox"), "{id:?} bundle missing viewBox");
            assert!(
                extract_inner(raw).contains("path"),
                "{id:?} inner has no paths"
            );
        }
    }

    #[test]
    fn render_wraps_inner_with_positioning() {
        let out = render(BadgeId::BatteryChamp, 100, 50, 32);
        assert!(out.starts_with("<svg"));
        assert!(out.contains(r#"x="100""#));
        assert!(out.contains(r#"y="50""#));
        assert!(out.contains(r#"width="32""#));
        assert!(out.contains(r#"viewBox="0 0 36 36""#));
        assert!(out.contains("path"));
        assert!(out.ends_with("</svg>"));
    }

    #[test]
    fn extract_inner_strips_outer_wrapper() {
        let svg = r#"<svg viewBox="0 0 36 36"><path d="M0 0"/></svg>"#;
        assert_eq!(extract_inner(svg), r#"<path d="M0 0"/>"#);
    }
}
