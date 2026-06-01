//! Pure PII redaction, shared by the CLI (`info`, `card`, `--json`) and the
//! GUI. Lives in the facade layer (not in a command) so every surface masks
//! the same fields the same way — the policy is defined once, here.

use crate::device::DeviceInfo;

/// How many trailing characters stay visible when masking an identifier.
pub const VISIBLE_TAIL: usize = 4;

/// Return a copy of `info` with every PII field masked: serial, UDID,
/// Wi-Fi/Bluetooth MAC, and both IMEIs. Non-sensitive fields (name, model,
/// colour, OS) are left untouched — they're safe to show in a shared
/// screenshot.
pub fn device_info(info: DeviceInfo) -> DeviceInfo {
    DeviceInfo {
        serial: tail(&info.serial),
        udid: tail(&info.udid),
        wifi_address: info.wifi_address.as_deref().map(tail),
        bluetooth_address: info.bluetooth_address.as_deref().map(tail),
        imei: info.imei.as_deref().map(tail),
        imei2: info.imei2.as_deref().map(tail),
        ..info
    }
}

/// Mask `value` keeping its last [`VISIBLE_TAIL`] characters.
pub fn tail(value: &str) -> String {
    redact_tail(value, VISIBLE_TAIL)
}

/// Mask `value` to a fixed-width form `***…XXXX` that keeps the last
/// `visible_tail` characters but hides the original length. Inputs shorter
/// than the tail are fully masked so the suffix's identity never leaks.
pub fn redact_tail(value: &str, visible_tail: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    if len <= visible_tail {
        return "*".repeat(len);
    }
    let tail: String = chars[len - visible_tail..].iter().collect();
    format!("***…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_tail_keeps_last_n_chars() {
        // Fixed-width prefix (`***…`) hides the original length while still
        // showing the last `n` chars. Inputs shorter than the tail mask
        // every char to avoid revealing the suffix's identity.
        assert_eq!(redact_tail("350123456789012", 4), "***…9012");
        assert_eq!(redact_tail("AA:BB:CC:DD:EE:FF", 4), "***…E:FF");
        assert_eq!(redact_tail("abc", 4), "***");
        assert_eq!(redact_tail("", 4), "");
    }

    #[test]
    fn device_info_masks_only_pii_fields() {
        let info = DeviceInfo {
            name: "Lucas's iPhone".into(),
            model_friendly: Some("iPhone 15 Pro Max".into()),
            enclosure_color: Some("Natural Titanium".into()),
            serial: "F2LXXXXXXXXX".into(),
            udid: "00008130-001A2B3C4D5E6F7G".into(),
            wifi_address: Some("AA:BB:CC:DD:EE:FF".into()),
            imei: Some("350123456789012".into()),
            ..Default::default()
        };
        let masked = device_info(info);
        // PII masked.
        assert!(!masked.serial.contains("F2LXXXXXXXXX"));
        assert!(masked.serial.starts_with("***…"));
        assert!(masked.udid.starts_with("***…"));
        assert_eq!(masked.wifi_address.as_deref(), Some("***…E:FF"));
        assert_eq!(masked.imei.as_deref(), Some("***…9012"));
        // Non-sensitive fields untouched.
        assert_eq!(masked.name, "Lucas's iPhone");
        assert_eq!(masked.model_friendly.as_deref(), Some("iPhone 15 Pro Max"));
        assert_eq!(masked.enclosure_color.as_deref(), Some("Natural Titanium"));
    }
}
