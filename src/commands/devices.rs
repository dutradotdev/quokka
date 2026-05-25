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
