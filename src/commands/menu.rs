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

use crate::commands::{analyze, apps, capture, card, dashboard, info, logs, media, power, update};
use crate::device::Device;
use crate::ui::{now_unix, spinner, terminal_width};

const TAGLINE: &str = "Inspect and tidy your iPhone from the Mac";
const AUTHOR: &str = "by Lucas Dutra";
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy)]
enum Choice {
    Apps,
    Analyze,
    Media,
    Logs,
    Capture,
    Info,
    Card,
    Refresh,
    Reboot,
    Shutdown,
    Update,
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
        let menu: &[(&str, &str, Choice)] = &[
            ("Apps", "List & uninstall user apps", Choice::Apps),
            ("Analyze", "Find the heaviest media files", Choice::Analyze),
            ("Media", "Survey camera roll & downloads", Choice::Media),
            ("Logs", "Stream device syslog", Choice::Logs),
            ("Capture", "Stream network packets per app", Choice::Capture),
            ("Info", "Print device identity", Choice::Info),
            ("Card", "Render a shareable 1080² PNG", Choice::Card),
            ("Refresh", "Re-read device info", Choice::Refresh),
            ("Reboot", "Restart the device", Choice::Reboot),
            ("Shutdown", "Power off the device", Choice::Shutdown),
            ("Update", "Check for a new quokka release", Choice::Update),
            ("Quit", "", Choice::Quit),
        ];
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
            Choice::Capture => capture::run(device, capture::Options::default()).await?,
            Choice::Info => info::run(device, false, false).await?,
            Choice::Card => {
                card::run(
                    device,
                    now_unix(),
                    card::CardArgs {
                        output: None,
                        no_open: false,
                        redact: false,
                    },
                )
                .await?;
                // Card's own `prompt_for_star` is the natural pause — any
                // key already returns us to the loop. Skip the standard
                // "Press Enter to return to the menu..." so the user
                // doesn't have to gate twice.
                continue;
            }
            Choice::Reboot => power::run(device, power::Action::Reboot, false).await?,
            Choice::Shutdown => power::run(device, power::Action::Shutdown, false).await?,
            Choice::Update => update::run(false, false).await?,
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
    if !crate::ui::stdin_is_interactive() {
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
