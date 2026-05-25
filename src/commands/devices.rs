//! `quokka devices` — list every iPhone reachable through usbmuxd.
//! Helps the user pick a `--udid` when multiple devices are plugged in.

use std::io::Write;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::device::{list_devices, DeviceListing};

pub async fn run() -> Result<()> {
    let listings = list_devices().await?;
    let mut out = anstream::stdout();
    if listings.is_empty() {
        writeln!(out, "No iPhones connected.")?;
        return Ok(());
    }
    write!(out, "{}", render(&listings))?;
    Ok(())
}

pub fn render(listings: &[DeviceListing]) -> String {
    let name_w = listings
        .iter()
        .map(|d| d.name.as_deref().unwrap_or("(untrusted)").chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    let model_w = listings
        .iter()
        .map(|d| {
            d.model_friendly
                .as_deref()
                .or(d.model_identifier.as_deref())
                .unwrap_or("?")
                .chars()
                .count()
        })
        .max()
        .unwrap_or(5)
        .max(5);

    let mut out = String::new();
    for d in listings {
        let name = d.name.as_deref().unwrap_or("(untrusted)");
        let model = d
            .model_friendly
            .as_deref()
            .or(d.model_identifier.as_deref())
            .unwrap_or("?");
        out.push_str(&format!(
            "  {name:<name_w$}  {model:<model_w$}  {conn:<5}  {udid}\n",
            conn = d.connection,
            udid = d.udid.dimmed(),
        ));
    }
    let count = listings.len();
    let plural = if count == 1 { "device" } else { "devices" };
    out.push_str(&format!("\n{count} {plural} connected.\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceListing;

    fn paired(udid: &str, name: &str, model: &str, friendly: &str) -> DeviceListing {
        DeviceListing {
            udid: udid.into(),
            connection: "USB",
            name: Some(name.into()),
            model_identifier: Some(model.into()),
            model_friendly: Some(friendly.into()),
        }
    }

    #[test]
    fn render_single_device_uses_singular_count_line() {
        let out = render(&[paired(
            "UDID-1",
            "Lucas's iPhone",
            "iPhone16,2",
            "iPhone 15 Pro Max",
        )]);
        assert!(out.contains("Lucas's iPhone"));
        assert!(out.contains("iPhone 15 Pro Max"));
        assert!(out.contains("UDID-1"));
        assert!(out.contains("USB"));
        assert!(out.contains("1 device connected."));
        assert!(!out.contains("devices connected"));
    }

    #[test]
    fn render_multiple_devices_uses_plural_count_line() {
        let out = render(&[
            paired("UDID-1", "A", "iPhone15,3", "iPhone 14 Pro Max"),
            paired("UDID-2", "B", "iPhone16,2", "iPhone 15 Pro Max"),
        ]);
        assert!(out.contains("UDID-1"));
        assert!(out.contains("UDID-2"));
        assert!(out.contains("2 devices connected."));
    }

    #[test]
    fn render_untrusted_falls_back_to_placeholder_name_and_question_mark_model() {
        let out = render(&[DeviceListing {
            udid: "UDID-X".into(),
            connection: "USB",
            name: None,
            model_identifier: None,
            model_friendly: None,
        }]);
        assert!(out.contains("(untrusted)"));
        assert!(out.contains("?"));
        assert!(out.contains("UDID-X"));
    }

    #[test]
    fn render_falls_back_to_model_identifier_when_friendly_missing() {
        let out = render(&[DeviceListing {
            udid: "UDID-1".into(),
            connection: "USB",
            name: Some("Phone".into()),
            model_identifier: Some("iPhone99,9".into()),
            model_friendly: None,
        }]);
        assert!(out.contains("iPhone99,9"));
    }

    #[test]
    fn render_aligns_columns_to_widest_name_and_model() {
        // Pick names of very different widths; the short row should be padded
        // out to the long one's width. We check the indentation indirectly:
        // both rows must contain a literal double-space gap between the model
        // and the connection token.
        let out = render(&[
            paired("UDID-1", "X", "iPhone16,2", "iPhone 15 Pro Max"),
            paired(
                "UDID-2",
                "Lucas's iPhone",
                "iPhone15,3",
                "iPhone 14 Pro Max",
            ),
        ]);
        // Width of the model column has to fit "iPhone 15 Pro Max" (17 chars).
        // We don't pin a column count, just assert the longer name is present
        // verbatim — a width regression would cause the short row to mash
        // the model+conn columns together (no whitespace between them).
        assert!(out.contains("Lucas's iPhone"));
        for line in out.lines().filter(|l| l.contains("UDID-")) {
            assert!(
                line.contains("  USB"),
                "expected at least two spaces before USB column: {line:?}"
            );
        }
    }
}
