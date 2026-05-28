//! Static lookup from lockdown `HardwarePlatform` codename (e.g. `t8120`) to
//! the marketing chip name (e.g. `"A16 Bionic"`).
//!
//! Apple ships the platform codename in `HardwarePlatform`; the marketing
//! name only appears in keynotes and marketing material. Used by `qk card`
//! to print "iPhone 14 Pro Max · A16 Bionic" in the header.
//!
//! Sources: cross-reference with the model→identifier mapping in
//! [`super::model_names`]. When Apple ships a new SoC, append a row.

/// Resolve `"t8120"` → `Some("A16 Bionic")`. Returns `None` for unknown
/// platforms — callers omit the chip line rather than fall back to the raw
/// codename (which means nothing to a non-Apple-employee).
pub fn chip_name(platform: &str) -> Option<&'static str> {
    let key = platform.trim();
    TABLE
        .iter()
        .find(|(p, _)| p.eq_ignore_ascii_case(key))
        .map(|(_, name)| *name)
}

static TABLE: &[(&str, &str)] = &[
    ("t8015", "A11 Bionic"), // iPhone 8 / 8 Plus / X
    ("t8020", "A12 Bionic"), // iPhone XS / XR
    ("t8027", "A12 Bionic"), // iPhone XR (alt rev)
    ("t8030", "A13 Bionic"), // iPhone 11 / 11 Pro / SE 2
    ("t8101", "A14 Bionic"), // iPhone 12 / 12 mini / 12 Pro / 12 Pro Max
    ("t8110", "A15 Bionic"), // iPhone 13 / 13 mini / 13 Pro / SE 3 / 14 / 14 Plus
    ("t8120", "A16 Bionic"), // iPhone 14 Pro / 14 Pro Max / 15 / 15 Plus
    ("t8122", "A16 Bionic"), // iPhone 15 / 15 Plus (alt rev)
    ("t8130", "A17 Pro"),    // iPhone 15 Pro / 15 Pro Max
    ("t8140", "A18"),        // iPhone 16 / 16 Plus
    ("t8145", "A18 Pro"),    // iPhone 16 Pro / 16 Pro Max
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_platforms() {
        assert_eq!(chip_name("t8030"), Some("A13 Bionic"));
        assert_eq!(chip_name("t8101"), Some("A14 Bionic"));
        assert_eq!(chip_name("t8110"), Some("A15 Bionic"));
        assert_eq!(chip_name("t8120"), Some("A16 Bionic"));
        assert_eq!(chip_name("t8130"), Some("A17 Pro"));
        assert_eq!(chip_name("t8140"), Some("A18"));
        assert_eq!(chip_name("t8145"), Some("A18 Pro"));
    }

    #[test]
    fn unknown_platform_is_none() {
        assert_eq!(chip_name("t9999"), None);
        assert_eq!(chip_name(""), None);
    }

    #[test]
    fn match_is_case_insensitive_and_trims() {
        assert_eq!(chip_name("T8120"), Some("A16 Bionic"));
        assert_eq!(chip_name("  t8120  "), Some("A16 Bionic"));
    }
}
