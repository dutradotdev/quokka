//! Heuristic: does the installed-app list include a known jailbreak store
//! or launcher?
//!
//! `installation_proxy.browse` returns bundle ids regardless of jailbreak
//! state — Sileo, Zebra, etc. show up the same way any other user app does.
//! Matching against a curated bundle-id list is exact and cheap; it avoids
//! the syslog-sampling approach the original spec sketch proposed (which is
//! slow and non-deterministic).
//!
//! False positives are unlikely with exact bundle-id match: legitimate apps
//! whose names happen to overlap (e.g. an app *called* "Cydia") don't share
//! the bundle id.

use crate::device::App;

/// Bundle ids of widely-used jailbreak stores, launchers, and entitlement
/// patches. Keep the list small and well-attested — every entry should be
/// something users actually install on a jailbroken device, not a long-tail
/// utility that depends on jailbreak but isn't diagnostic of one.
pub(crate) const JAILBREAK_BUNDLE_IDS: &[&str] = &[
    // Package managers / stores
    "org.coolstar.SileoStore",
    "xyz.willy.Zebra",
    "com.saurik.Cydia",
    // Launchers / loaders / RCE bundles
    "org.checkra1n.layout",
    "com.opa334.Dopamine",
    "com.opa334.Trollstore",
    "com.tigisoftware.Filza",
];

/// `true` when at least one installed bundle id exactly matches an entry in
/// the curated jailbreak list. Comparison is case-insensitive — Apple's
/// CFBundleIdentifier rules are case-sensitive, but iOS itself often
/// normalises, and the cost of being loose here is zero.
pub fn detect_in_apps(apps: &[App]) -> bool {
    apps.iter().any(|app| {
        JAILBREAK_BUNDLE_IDS
            .iter()
            .any(|known| app.bundle_id.eq_ignore_ascii_case(known))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(bundle_id: &str) -> App {
        App {
            bundle_id: bundle_id.into(),
            name: "name".into(),
            size_bytes: 0,
            is_system: false,
            install_date_unix: None,
        }
    }

    #[test]
    fn detects_each_known_store() {
        for id in JAILBREAK_BUNDLE_IDS {
            assert!(detect_in_apps(&[app(id)]), "missed known id `{id}`");
        }
    }

    #[test]
    fn case_insensitive_match() {
        assert!(detect_in_apps(&[app("ORG.COOLSTAR.SILEOSTORE")]));
    }

    #[test]
    fn does_not_match_substrings() {
        // App named like "cydia" but with a real bundle id should NOT trip.
        assert!(!detect_in_apps(&[app("com.example.cydiafan")]));
        assert!(!detect_in_apps(&[app("com.zebra.printers")]));
    }

    #[test]
    fn empty_app_list_is_clean() {
        assert!(!detect_in_apps(&[]));
    }
}
