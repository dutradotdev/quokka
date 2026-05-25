//! Static lookup from Apple model identifier (e.g. `iPhone15,3`) to the
//! marketing name. iPhone-only — quokka does not support iPads or Watches.
//!
//! Sources: <https://support.apple.com/en-us/108044> and Apple's identifier
//! lists. When Apple ships a new iPhone, append a row.

/// Resolve `iPhone15,3` → `"iPhone 14 Pro Max"`. Returns `None` for unknown
/// identifiers; the renderer falls back to the raw identifier.
///
/// Match is case-insensitive and trims whitespace.
pub fn friendly_name(identifier: &str) -> Option<&'static str> {
    let key = identifier.trim();
    TABLE
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(key))
        .map(|(_, name)| *name)
}

static TABLE: &[(&str, &str)] = &[
    // iPhone 8 / 8 Plus / X
    ("iPhone10,1", "iPhone 8"),
    ("iPhone10,4", "iPhone 8"),
    ("iPhone10,2", "iPhone 8 Plus"),
    ("iPhone10,5", "iPhone 8 Plus"),
    ("iPhone10,3", "iPhone X"),
    ("iPhone10,6", "iPhone X"),
    // iPhone XS / XR
    ("iPhone11,2", "iPhone XS"),
    ("iPhone11,4", "iPhone XS Max"),
    ("iPhone11,6", "iPhone XS Max"),
    ("iPhone11,8", "iPhone XR"),
    // iPhone 11
    ("iPhone12,1", "iPhone 11"),
    ("iPhone12,3", "iPhone 11 Pro"),
    ("iPhone12,5", "iPhone 11 Pro Max"),
    ("iPhone12,8", "iPhone SE (2nd gen)"),
    // iPhone 12
    ("iPhone13,1", "iPhone 12 mini"),
    ("iPhone13,2", "iPhone 12"),
    ("iPhone13,3", "iPhone 12 Pro"),
    ("iPhone13,4", "iPhone 12 Pro Max"),
    // iPhone 13
    ("iPhone14,2", "iPhone 13 Pro"),
    ("iPhone14,3", "iPhone 13 Pro Max"),
    ("iPhone14,4", "iPhone 13 mini"),
    ("iPhone14,5", "iPhone 13"),
    ("iPhone14,6", "iPhone SE (3rd gen)"),
    // iPhone 14
    ("iPhone14,7", "iPhone 14"),
    ("iPhone14,8", "iPhone 14 Plus"),
    ("iPhone15,2", "iPhone 14 Pro"),
    ("iPhone15,3", "iPhone 14 Pro Max"),
    // iPhone 15
    ("iPhone15,4", "iPhone 15"),
    ("iPhone15,5", "iPhone 15 Plus"),
    ("iPhone16,1", "iPhone 15 Pro"),
    ("iPhone16,2", "iPhone 15 Pro Max"),
    // iPhone 16
    ("iPhone17,1", "iPhone 16 Pro"),
    ("iPhone17,2", "iPhone 16 Pro Max"),
    ("iPhone17,3", "iPhone 16"),
    ("iPhone17,4", "iPhone 16 Plus"),
    ("iPhone17,5", "iPhone 16e"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_identifier_resolves() {
        assert_eq!(friendly_name("iPhone15,3"), Some("iPhone 14 Pro Max"));
        assert_eq!(friendly_name("iPhone12,1"), Some("iPhone 11"));
    }

    #[test]
    fn unknown_identifier_returns_none() {
        assert_eq!(friendly_name("iPhone99,9"), None);
        assert_eq!(friendly_name(""), None);
    }

    #[test]
    fn match_is_case_insensitive_and_trims() {
        assert_eq!(friendly_name("iphone15,3"), Some("iPhone 14 Pro Max"));
        assert_eq!(friendly_name("  IPHONE15,3 "), Some("iPhone 14 Pro Max"));
    }
}
