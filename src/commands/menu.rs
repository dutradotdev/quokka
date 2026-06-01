//! Interactive launcher shown when `quokka`/`qk` is invoked with no
//! subcommand on a TTY. Renders the welcome dashboard (live read of the
//! connected iPhone) followed by a numbered list of commands.
//!
//! Non-TTY callers (pipes, CI) get clap's `--help` instead — see
//! `lib.rs`. A multi-select picker would be invisible there.

use std::io::Write;

use anyhow::Result;
use crossterm::{cursor, execute, terminal};
use dialoguer::{theme::ColorfulTheme, Select};
use owo_colors::OwoColorize;

use crate::commands::dashboard;
use crate::commands::device_action::{self, DeviceAction};
use crate::commands::sidebar;
use crate::device::{self, Device, Platform};
use crate::ui::{now_unix, spinner, terminal_width};

const TAGLINE: &str = "Inspect and tidy your device from the Mac";
const AUTHOR: &str = "by Lucas Dutra";
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy)]
enum Choice {
    /// A device-scoped command, dispatched through [`device_action`].
    Action(DeviceAction),
    /// Re-read the device and redraw the dashboard.
    Refresh,
    Quit,
}

/// Bare `quokka` / `qk` entry point. Owns device selection: any connected
/// device (one or more) opens the multi-device [`sidebar`] — the default view.
/// An explicit `--udid`/`--platform` target, or no device at all, falls back to
/// a single connection + the dialoguer [`run`] menu (which also surfaces the
/// "nothing connected" error).
pub async fn run_launcher(udid: Option<&str>, platform: Option<Platform>) -> Result<()> {
    // An explicit target picks exactly one device — no enumeration, and the
    // simple single-device menu rather than the multi-device sidebar.
    if udid.is_some() || platform.is_some() {
        let device = device::connect(udid, platform).await?;
        return run(&*device).await;
    }
    let listings = device::list_devices().await.unwrap_or_default();
    if listings.is_empty() {
        // Nothing connected anywhere — `connect` surfaces the actionable error
        // (and covers the rare race where a device appears between calls).
        let device = device::connect(None, None).await?;
        return run(&*device).await;
    }
    // The sidebar is the default launcher view for any connected device — a
    // single device still shows its device pane + dashboard + actions.
    sidebar::run(listings).await
}

/// Build the menu rows. `capture_supported` hides "Capture" on backends without
/// packet capture (Android), so the user never selects a guaranteed "iOS only"
/// error. Action rows reuse [`DeviceAction`] so labels and dispatch share one
/// source of truth with the sidebar launcher.
fn build_menu(capture_supported: bool) -> Vec<(&'static str, &'static str, Choice)> {
    let action_row = |a: DeviceAction| (a.label(), a.description(), Choice::Action(a));
    let mut menu: Vec<(&'static str, &'static str, Choice)> = vec![
        action_row(DeviceAction::Apps),
        action_row(DeviceAction::Analyze),
        action_row(DeviceAction::Media),
        action_row(DeviceAction::Logs),
    ];
    if capture_supported {
        menu.push(action_row(DeviceAction::Capture));
    }
    menu.push(action_row(DeviceAction::Info));
    menu.push(action_row(DeviceAction::Card));
    menu.push(("Refresh", "Re-read device info", Choice::Refresh));
    menu.push(action_row(DeviceAction::Reboot));
    menu.push(action_row(DeviceAction::Shutdown));
    menu.push(action_row(DeviceAction::Update));
    menu.push(("Quit", "", Choice::Quit));
    menu
}

/// The single-device interactive menu. Used directly when one device is
/// connected (or one was forced), and by `qk card`'s post-render hand-off.
pub async fn run(device: &dyn Device) -> Result<()> {
    let capture_supported = device.as_capture().is_some();
    loop {
        clear_screen()?;
        let bar = spinner("Reading device info...");
        let status = device.status().await;
        bar.finish_and_clear();
        let status = status?;

        let mut out = anstream::stdout();
        writeln!(
            out,
            "{}",
            dashboard::render(&status, terminal_width(), now_unix())
        )?;
        writeln!(out)?;
        writeln!(
            out,
            "  {tagline} · {author} · {version}",
            tagline = TAGLINE,
            author = AUTHOR.dimmed(),
            version = VERSION.dimmed(),
        )?;
        writeln!(out)?;
        out.flush()?;

        // Single source of truth — labels and choices stay aligned even when
        // a new entry is inserted in the middle. The old code hard-coded
        // `Some(8) => Quit` and would silently misroute on additions.
        let menu = build_menu(capture_supported);
        let items: Vec<String> = menu
            .iter()
            .map(|(label, desc, _)| {
                if desc.is_empty() {
                    (*label).to_string()
                } else {
                    format!("{:<10} {}", label, desc)
                }
            })
            .collect();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .items(&items)
            .default(0)
            .interact_opt()?;

        let choice = match selection {
            None => Choice::Quit,
            Some(i) => menu.get(i).map(|(_, _, c)| *c).unwrap_or(Choice::Quit),
        };

        match choice {
            Choice::Quit => return Ok(()),
            Choice::Refresh => continue,
            Choice::Action(action) => {
                device_action::run(device, action).await?;
                // Card's own star prompt is the natural pause; skip the extra
                // "Press Enter to return…" so the user doesn't gate twice.
                if matches!(action, DeviceAction::Card) {
                    continue;
                }
            }
        }

        crate::ui::wait_for_enter()?;
    }
}

fn clear_screen() -> Result<()> {
    let mut out = std::io::stdout();
    execute!(
        out,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(capture_supported: bool) -> Vec<&'static str> {
        build_menu(capture_supported)
            .into_iter()
            .map(|(label, _, _)| label)
            .collect()
    }

    #[test]
    fn capture_entry_hidden_when_unsupported() {
        // Android backends report no capture capability — the menu must not
        // offer an action that can only fail with "iOS only".
        assert!(!labels(false).contains(&"Capture"));
        assert!(labels(true).contains(&"Capture"));
    }

    #[test]
    fn menu_always_ends_with_quit() {
        for capture in [false, true] {
            assert_eq!(labels(capture).last().copied(), Some("Quit"));
        }
    }
}
