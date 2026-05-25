//! `quokka status` — print the dashboard non-interactively.
//!
//! Same renderer as the welcome screen; this command just runs it once and
//! exits instead of looping back to the menu.

use anyhow::Result;

use crate::commands::dashboard;
use crate::device::Device;
use crate::ui::{now_unix, spinner, terminal_width};

pub async fn run(device: &dyn Device) -> Result<()> {
    let bar = spinner("Reading device info...");
    let status = device.status().await;
    bar.finish_and_clear();

    let status = status?;
    let output = dashboard::render(&status, terminal_width(), now_unix());
    let mut out = anstream::stdout();
    use std::io::Write;
    writeln!(out, "{output}")?;
    Ok(())
}
