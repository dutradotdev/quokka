//! Classification helpers for the lockdown `BuildVersion` string.
//!
//! Apple's build numbers follow a stable shape: a 2-digit major + a single
//! letter for the OS branch + 1-or-more digits, optionally with a trailing
//! letter for *betas only*. Examples:
//!
//! - `22C152`   — stable release (no trailing letter)
//! - `22D5034e` — developer / public beta (trailing lowercase letter)
//!
//! No `regex` crate dependency — the check is a 5-line manual scan that
//! costs nothing and is fully testable.

/// `true` when `build` matches the developer/public-beta shape: the build
/// number ends in a lowercase letter *after* at least one digit. Stable
/// builds (e.g. `22C152`) end in a digit; beta builds (e.g. `22D5034e`) end
/// in a letter.
pub fn is_beta(build: &str) -> bool {
    let s = build.trim();
    // Need at least one digit before the trailing letter.
    let bytes = s.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let last = bytes[bytes.len() - 1];
    let prev = bytes[bytes.len() - 2];
    last.is_ascii_lowercase() && prev.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_builds_are_not_beta() {
        for stable in [
            "22C152", // iOS 18.2
            "21H125", // iOS 17.7
            "20G75",  // iOS 16.7.2
            "19H384", // iOS 15.8.3
        ] {
            assert!(!is_beta(stable), "{stable} should be stable");
        }
    }

    #[test]
    fn beta_builds_are_detected() {
        for beta in [
            "22D5034e", // iOS 18.3 dev beta 5
            "21A5277h", // iOS 17 dev beta
            "22D5050a", // iOS 18.3 dev beta (later spin)
        ] {
            assert!(is_beta(beta), "{beta} should be beta");
        }
    }

    #[test]
    fn empty_or_short_input_is_not_beta() {
        assert!(!is_beta(""));
        assert!(!is_beta("a"));
        assert!(!is_beta("22"));
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        assert!(is_beta("  22D5034e  "));
        assert!(!is_beta("  22C152  "));
    }
}
