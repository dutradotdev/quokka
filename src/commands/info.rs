//! `quokka info` — print the connected iPhone's static identity in three
//! labeled blocks (Device / System / Network). `--redact` masks PII fields.

use std::io::Write;

use anyhow::Result;

use crate::device::{Device, DeviceInfo};
use crate::ui::spinner;

const LABEL_WIDTH: usize = 18;
const REDACT_VISIBLE_TAIL: usize = 4;

pub async fn run(device: &dyn Device, redact: bool) -> Result<()> {
    let bar = spinner("Reading device info...");
    let info = device.info().await;
    bar.finish_and_clear();
    let info = info?;

    let mut out = anstream::stdout();
    write!(out, "{}", render(&info, redact))?;
    Ok(())
}

pub fn render(info: &DeviceInfo, redact: bool) -> String {
    let blocks = [
        render_device_block(info, redact),
        render_system_block(info),
        render_network_block(info, redact),
    ];
    let mut out = String::new();
    let mut first = true;
    for block in blocks.iter().flatten() {
        if !first {
            out.push('\n');
        }
        out.push_str(block);
        out.push('\n');
        first = false;
    }
    out
}

fn label_line(label: &str, value: &str) -> String {
    format!("  {label:<LABEL_WIDTH$}{value}\n")
}

fn block(header: &str, rows: Vec<(&str, String)>) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let mut s = format!("{header}\n");
    for (label, value) in rows {
        s.push_str(&label_line(label, &value));
    }
    Some(s.trim_end().to_string())
}

pub fn render_device_block(info: &DeviceInfo, redact: bool) -> Option<String> {
    let mut rows: Vec<(&str, String)> = Vec::new();
    rows.push(("Name", info.name.clone()));
    rows.push(("Model", format_model(info)));
    if let Some(v) = &info.model_number {
        rows.push(("Model number", v.clone()));
    }
    if let Some(v) = &info.region_info {
        rows.push(("Region", v.clone()));
    }
    if let Some(v) = &info.enclosure_color {
        rows.push(("Color", v.clone()));
    }
    rows.push(("Serial", maybe_redact(&info.serial, redact)));
    rows.push(("UDID", maybe_redact(&info.udid, redact)));
    block("Device", rows)
}

pub fn render_system_block(info: &DeviceInfo) -> Option<String> {
    let mut rows: Vec<(&str, String)> = Vec::new();
    let ios = match &info.ios_build {
        Some(build) => format!("{} (build {build})", info.ios_version),
        None => info.ios_version.clone(),
    };
    rows.push(("iOS", ios));
    if let Some(v) = &info.hardware_model {
        rows.push(("Hardware", v.clone()));
    }
    if let Some(v) = &info.cpu_architecture {
        rows.push(("CPU", v.clone()));
    }
    if let Some(v) = &info.activation_state {
        rows.push(("Activation", v.clone()));
    }
    if let Some(v) = info.is_supervised {
        rows.push(("Supervised", yes_no(v).to_string()));
    }
    if let Some(v) = info.developer_mode_enabled {
        rows.push(("Developer mode", on_off(v).to_string()));
    }
    block("System", rows)
}

pub fn render_network_block(info: &DeviceInfo, redact: bool) -> Option<String> {
    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &info.wifi_address {
        rows.push(("Wi-Fi MAC", maybe_redact(v, redact)));
    }
    if let Some(v) = &info.bluetooth_address {
        rows.push(("Bluetooth MAC", maybe_redact(v, redact)));
    }
    if let Some(v) = &info.imei {
        rows.push(("IMEI", maybe_redact(v, redact)));
    }
    if let Some(v) = &info.imei2 {
        rows.push(("IMEI 2", maybe_redact(v, redact)));
    }
    block("Network", rows)
}

fn format_model(info: &DeviceInfo) -> String {
    match &info.model_friendly {
        Some(friendly) => format!("{friendly} ({})", info.model_identifier),
        None => info.model_identifier.clone(),
    }
}

fn maybe_redact(value: &str, redact: bool) -> String {
    if redact {
        redact_tail(value, REDACT_VISIBLE_TAIL)
    } else {
        value.to_string()
    }
}

pub fn redact_tail(value: &str, visible_tail: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    if len <= visible_tail {
        return "*".repeat(len);
    }
    let mut s = String::with_capacity(len);
    for _ in 0..(len - visible_tail) {
        s.push('*');
    }
    for ch in &chars[len - visible_tail..] {
        s.push(*ch);
    }
    s
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "Yes"
    } else {
        "No"
    }
}

fn on_off(v: bool) -> &'static str {
    if v {
        "On"
    } else {
        "Off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_info() -> DeviceInfo {
        DeviceInfo {
            name: "Lucas's iPhone".into(),
            model_identifier: "iPhone16,2".into(),
            model_friendly: Some("iPhone 15 Pro Max".into()),
            model_number: Some("MQ8X3LL/A".into()),
            region_info: Some("LL/A".into()),
            enclosure_color: Some("Natural Titanium".into()),
            serial: "F2LXXXXXXXXX".into(),
            udid: "00008130-001A2B3C4D5E6F7G".into(),
            ios_version: "18.2".into(),
            ios_build: Some("22C152".into()),
            hardware_model: Some("D74AP".into()),
            cpu_architecture: Some("arm64e".into()),
            activation_state: Some("Activated".into()),
            is_supervised: Some(false),
            developer_mode_enabled: Some(false),
            wifi_address: Some("AA:BB:CC:DD:EE:FF".into()),
            bluetooth_address: Some("AA:BB:CC:DD:EE:F0".into()),
            imei: Some("350123456789012".into()),
            imei2: Some("350123456789013".into()),
        }
    }

    fn minimal_info() -> DeviceInfo {
        DeviceInfo {
            name: "Phone".into(),
            model_identifier: "iPhone16,2".into(),
            serial: "F2L0000".into(),
            udid: "00008130-AAAA".into(),
            ios_version: "18.2".into(),
            ..Default::default()
        }
    }

    #[test]
    fn format_model_with_friendly() {
        let info = full_info();
        assert_eq!(format_model(&info), "iPhone 15 Pro Max (iPhone16,2)");
    }

    #[test]
    fn format_model_falls_back_to_identifier() {
        let mut info = full_info();
        info.model_friendly = None;
        assert_eq!(format_model(&info), "iPhone16,2");
    }

    #[test]
    fn redact_tail_keeps_last_n_chars() {
        assert_eq!(redact_tail("350123456789012", 4), "***********9012");
        assert_eq!(redact_tail("AA:BB:CC:DD:EE:FF", 4), "*************E:FF");
        assert_eq!(redact_tail("abc", 4), "***");
        assert_eq!(redact_tail("", 4), "");
    }

    #[test]
    fn render_device_block_includes_all_fields() {
        let info = full_info();
        let block = render_device_block(&info, false).unwrap();
        assert!(block.contains("Device"));
        assert!(block.contains("Lucas's iPhone"));
        assert!(block.contains("iPhone 15 Pro Max"));
        assert!(block.contains("MQ8X3LL/A"));
        assert!(block.contains("LL/A"));
        assert!(block.contains("Natural Titanium"));
        assert!(block.contains("F2LXXXXXXXXX"));
    }

    #[test]
    fn render_device_block_with_redact_masks_serial_and_udid() {
        let info = full_info();
        let block = render_device_block(&info, true).unwrap();
        assert!(!block.contains("F2LXXXXXXXXX"));
        assert!(block.contains("XXXX"));
        assert!(block.contains("Natural Titanium")); // color not masked
        assert!(block.contains("Lucas's iPhone"));
    }

    #[test]
    fn render_system_block_omits_unknown_supervised_and_dev_mode() {
        let mut info = full_info();
        info.is_supervised = None;
        info.developer_mode_enabled = None;
        let block = render_system_block(&info).unwrap();
        assert!(!block.contains("Supervised"));
        assert!(!block.contains("Developer mode"));
    }

    #[test]
    fn render_system_block_yes_no_and_on_off() {
        let mut info = full_info();
        info.is_supervised = Some(true);
        info.developer_mode_enabled = Some(true);
        let block = render_system_block(&info).unwrap();
        assert!(block.contains("Supervised        Yes"));
        assert!(block.contains("Developer mode    On"));
    }

    #[test]
    fn render_network_block_returns_none_when_empty() {
        let mut info = full_info();
        info.wifi_address = None;
        info.bluetooth_address = None;
        info.imei = None;
        info.imei2 = None;
        assert!(render_network_block(&info, false).is_none());
    }

    #[test]
    fn render_network_block_masks_when_redact() {
        let info = full_info();
        let block = render_network_block(&info, true).unwrap();
        assert!(!block.contains("AA:BB:CC:DD:EE:FF"));
        assert!(!block.contains("350123456789012"));
        assert!(block.contains("E:FF"));
        assert!(block.contains("9012"));
    }

    #[test]
    fn render_emits_blocks_in_device_system_network_order() {
        // Sequence is part of the output contract: swapping it would surprise
        // every user who diffs `qk info` output. Lock it with byte offsets.
        let info = full_info();
        let out = render(&info, false);
        let d = out.find("Device").expect("Device header present");
        let s = out.find("System").expect("System header present");
        let n = out.find("Network").expect("Network header present");
        assert!(d < s, "Device must come before System");
        assert!(s < n, "System must come before Network");
    }

    #[test]
    fn render_minimal_info_has_device_and_system_only() {
        let info = minimal_info();
        let out = render(&info, false);
        assert!(out.contains("Device"));
        assert!(out.contains("System"));
        assert!(!out.contains("Network"));
        assert!(!out.contains("IMEI"));
        assert!(!out.contains("Wi-Fi MAC"));
        assert!(!out.contains("Supervised"));
    }
}
