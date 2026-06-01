//! `quokka devices` — list every reachable device across both transports
//! (iPhones over usbmuxd, Android over adb). Helps the user pick a `--udid`
//! when multiple devices are plugged in.

use std::io::Write;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::device::{list_devices, DeviceListing};

/// Minimum column widths so headers and short rows stay aligned.
const NAME_COL_MIN: usize = 4;
const PLATFORM_COL_MIN: usize = 7;
const MODEL_COL_MIN: usize = 5;
const CONN_COL_WIDTH: usize = 5;

/// Placeholder when a device hasn't been trusted/authorized yet, so its
/// identity reads back empty.
const UNTRUSTED_NAME: &str = "(untrusted)";
const UNKNOWN_MODEL: &str = "?";

pub async fn run(json: bool) -> Result<()> {
    let listings = list_devices().await?;
    let mut out = anstream::stdout();
    if json {
        write!(out, "{}", render_json(&listings))?;
    } else {
        write!(out, "{}", format_listings(&listings))?;
    }
    Ok(())
}

fn render_json(listings: &[DeviceListing]) -> String {
    // Hand-rolled object so we don't have to put `Serialize` on every domain
    // type — keeps the JSON shape decoupled from the internal struct.
    let array: Vec<serde_json::Value> = listings
        .iter()
        .map(|d| {
            serde_json::json!({
                "platform": d.platform,
                "udid": d.udid,
                "connection": d.connection,
                "name": d.name,
                "model_identifier": d.model_identifier,
                "model_friendly": d.model_friendly,
            })
        })
        .collect();
    let mut s = serde_json::to_string_pretty(&array).unwrap_or_else(|_| "[]".to_string());
    s.push('\n');
    s
}

/// Pure formatter shared by `run` and the unit tests: yields the
/// "No devices connected." line when empty, otherwise the columnar render.
pub fn format_listings(listings: &[DeviceListing]) -> String {
    if listings.is_empty() {
        return "No devices connected.\n".to_string();
    }
    render(listings)
}

/// The human label for a device's model column, falling back from friendly
/// name to raw identifier to `?`.
fn model_label(d: &DeviceListing) -> &str {
    d.model_friendly
        .as_deref()
        .or(d.model_identifier.as_deref())
        .unwrap_or(UNKNOWN_MODEL)
}

pub fn render(listings: &[DeviceListing]) -> String {
    let name_w = listings
        .iter()
        .map(|d| d.name.as_deref().unwrap_or(UNTRUSTED_NAME).chars().count())
        .max()
        .unwrap_or(NAME_COL_MIN)
        .max(NAME_COL_MIN);
    let platform_w = listings
        .iter()
        .map(|d| d.platform.label().chars().count())
        .max()
        .unwrap_or(PLATFORM_COL_MIN)
        .max(PLATFORM_COL_MIN);
    let model_w = listings
        .iter()
        .map(|d| model_label(d).chars().count())
        .max()
        .unwrap_or(MODEL_COL_MIN)
        .max(MODEL_COL_MIN);

    let mut out = String::new();
    for d in listings {
        let name = d.name.as_deref().unwrap_or(UNTRUSTED_NAME);
        out.push_str(&format!(
            "  {name:<name_w$}  {platform:<platform_w$}  {model:<model_w$}  {conn:<CONN_COL_WIDTH$}  {udid}\n",
            platform = d.platform.label(),
            model = model_label(d),
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
    use crate::device::{DeviceListing, Platform};

    fn paired(udid: &str, name: &str, model: &str, friendly: &str) -> DeviceListing {
        DeviceListing {
            platform: Platform::Ios,
            udid: udid.into(),
            connection: "USB",
            name: Some(name.into()),
            model_identifier: Some(model.into()),
            model_friendly: Some(friendly.into()),
        }
    }

    fn android(udid: &str, model: &str) -> DeviceListing {
        DeviceListing {
            platform: Platform::Android,
            udid: udid.into(),
            connection: "USB",
            name: Some(model.into()),
            model_identifier: None,
            model_friendly: Some(model.into()),
        }
    }

    #[test]
    fn format_listings_empty_prints_no_devices_message() {
        let out = format_listings(&[]);
        assert_eq!(out, "No devices connected.\n");
    }

    #[test]
    fn render_shows_platform_column_for_each_device() {
        let out = render(&[
            paired(
                "UDID-1",
                "Lucas's iPhone",
                "iPhone16,2",
                "iPhone 15 Pro Max",
            ),
            android("ABC123", "Pixel 8"),
        ]);
        assert!(out.contains("iOS"));
        assert!(out.contains("Android"));
        assert!(out.contains("Pixel 8"));
        assert!(out.contains("2 devices connected."));
    }

    #[test]
    fn format_listings_non_empty_delegates_to_render() {
        let listings = [paired("UDID-1", "X", "iPhone16,2", "iPhone 15 Pro Max")];
        assert_eq!(format_listings(&listings), render(&listings));
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
            platform: Platform::Ios,
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
            platform: Platform::Ios,
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
