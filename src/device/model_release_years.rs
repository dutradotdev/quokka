//! Static lookup from Apple iPhone identifier (e.g. `iPhone15,3`) to the
//! calendar year the model shipped.
//!
//! Used by `qk card` for the **Day One** badge — "paired in the iPhone's
//! release year". Apple keeps the identifier scheme stable; when a new
//! iPhone family ships, append a row.
//!
//! Sources: cross-checked against the `model_names` table and Apple's
//! press releases.

/// Resolve `iPhone15,3` → `Some(2022)`. Returns `None` for unknown
/// identifiers (the badge then can't award itself).
pub fn release_year(identifier: &str) -> Option<u32> {
    let key = identifier.trim();
    TABLE
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(key))
        .map(|(_, year)| *year)
}

static TABLE: &[(&str, u32)] = &[
    // iPhone 8 / 8 Plus / X — 2017
    ("iPhone10,1", 2017),
    ("iPhone10,4", 2017),
    ("iPhone10,2", 2017),
    ("iPhone10,5", 2017),
    ("iPhone10,3", 2017),
    ("iPhone10,6", 2017),
    // iPhone XS / XS Max / XR — 2018
    ("iPhone11,2", 2018),
    ("iPhone11,4", 2018),
    ("iPhone11,6", 2018),
    ("iPhone11,8", 2018),
    // iPhone 11 / 11 Pro / 11 Pro Max — 2019
    ("iPhone12,1", 2019),
    ("iPhone12,3", 2019),
    ("iPhone12,5", 2019),
    // iPhone SE (2nd gen) — 2020
    ("iPhone12,8", 2020),
    // iPhone 12 family — 2020
    ("iPhone13,1", 2020),
    ("iPhone13,2", 2020),
    ("iPhone13,3", 2020),
    ("iPhone13,4", 2020),
    // iPhone 13 family — 2021
    ("iPhone14,2", 2021),
    ("iPhone14,3", 2021),
    ("iPhone14,4", 2021),
    ("iPhone14,5", 2021),
    // iPhone SE (3rd gen) — 2022
    ("iPhone14,6", 2022),
    // iPhone 14 family — 2022
    ("iPhone14,7", 2022),
    ("iPhone14,8", 2022),
    ("iPhone15,2", 2022),
    ("iPhone15,3", 2022),
    // iPhone 15 family — 2023
    ("iPhone15,4", 2023),
    ("iPhone15,5", 2023),
    ("iPhone16,1", 2023),
    ("iPhone16,2", 2023),
    // iPhone 16 family — 2024
    ("iPhone17,1", 2024),
    ("iPhone17,2", 2024),
    ("iPhone17,3", 2024),
    ("iPhone17,4", 2024),
    ("iPhone17,5", 2024),
    // iPhone 17 family (incl. iPhone Air) — 2025
    ("iPhone18,1", 2025),
    ("iPhone18,2", 2025),
    ("iPhone18,3", 2025),
    ("iPhone18,4", 2025),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_identifiers_to_release_year() {
        assert_eq!(release_year("iPhone15,3"), Some(2022)); // 14 Pro Max
        assert_eq!(release_year("iPhone16,2"), Some(2023)); // 15 Pro Max
        assert_eq!(release_year("iPhone17,2"), Some(2024)); // 16 Pro
        assert_eq!(release_year("iPhone18,2"), Some(2025)); // 17 Pro Max
        assert_eq!(release_year("iPhone10,3"), Some(2017)); // X
    }

    #[test]
    fn unknown_identifier_is_none() {
        assert_eq!(release_year("iPhone99,9"), None);
        assert_eq!(release_year(""), None);
    }

    #[test]
    fn match_is_case_insensitive_and_trims() {
        assert_eq!(release_year("IPHONE15,3"), Some(2022));
        assert_eq!(release_year("  iPhone15,3  "), Some(2022));
    }
}
