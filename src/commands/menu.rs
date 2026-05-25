//! Interactive launcher shown when `quokka`/`qk` is invoked with no
//! subcommand on a TTY. Renders the welcome dashboard (live read of the
//! connected iPhone) followed by a numbered list of commands.
//!
//! Non-TTY callers (pipes, CI) get clap's `--help` instead — see
//! `lib.rs`. A multi-select picker would be invisible there.

use std::io::{IsTerminal, Write};

use anyhow::Result;
use crossterm::{cursor, execute, terminal};
use dialoguer::{theme::ColorfulTheme, Select};
use owo_colors::OwoColorize;

use crate::commands::{analyze, apps, dashboard, info, logs, media, power};
use crate::device::Device;
use crate::ui::{now_unix, spinner, terminal_width};

const TAGLINE: &str = "Inspect and tidy your iPhone from the Mac";
const AUTHOR: &str = "by Lucas Dutra";

enum Choice {
    Apps,
    Analyze,
    Media,
    Logs,
    Info,
    Refresh,
    Reboot,
    Shutdown,
    Quit,
}

pub async fn run(device: &dyn Device) -> Result<()> {
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
            "  {tagline} · {author}",
            tagline = TAGLINE,
            author = AUTHOR.dimmed(),
        )?;
        writeln!(out)?;
        out.flush()?;

        let items = [
            format!("{:<10} {}", "Apps", "List & uninstall user apps"),
            format!("{:<10} {}", "Analyze", "Find the heaviest media files"),
            format!("{:<10} {}", "Media", "Survey camera roll & downloads"),
            format!("{:<10} {}", "Logs", "Stream device syslog"),
            format!("{:<10} {}", "Info", "Print device identity"),
            format!("{:<10} {}", "Refresh", "Re-read device info"),
            format!("{:<10} {}", "Reboot", "Restart the device"),
            format!("{:<10} {}", "Shutdown", "Power off the device"),
            "Quit".to_string(),
        ];
        let selection = Select::with_theme(&ColorfulTheme::default())
            .items(&items)
            .default(0)
            .interact_opt()?;

        let choice = match selection {
            None | Some(8) => Choice::Quit,
            Some(0) => Choice::Apps,
            Some(1) => Choice::Analyze,
            Some(2) => Choice::Media,
            Some(3) => Choice::Logs,
            Some(4) => Choice::Info,
            Some(5) => Choice::Refresh,
            Some(6) => Choice::Reboot,
            Some(7) => Choice::Shutdown,
            Some(_) => unreachable!(),
        };

        match choice {
            Choice::Quit => return Ok(()),
            Choice::Refresh => continue,
            Choice::Apps => {
                apps::run(
                    device,
                    apps::Options {
                        uninstall: None,
                        assume_yes: false,
                    },
                )
                .await?
            }
            Choice::Analyze => analyze::run(device, 20, true).await?,
            Choice::Media => media::run(device, false).await?,
            Choice::Logs => logs::run(device, logs::Options::default()).await?,
            Choice::Info => info::run(device, false).await?,
            Choice::Reboot => power::run(device, power::Action::Reboot, false).await?,
            Choice::Shutdown => power::run(device, power::Action::Shutdown, false).await?,
        }

        wait_for_continue()?;
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

fn wait_for_continue() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Ok(());
    }
    let mut out = anstream::stdout();
    writeln!(out)?;
    write!(out, "{} ", "Press Enter to return to the menu...".dimmed())?;
    out.flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    Ok(())
}
