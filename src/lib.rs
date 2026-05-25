//! quokka — CLI to inspect and tidy an iPhone connected to a Mac over USB.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

pub mod commands;
pub mod device;
pub mod ui;

use crate::device::LogLevel;

#[derive(ValueEnum, Debug, Clone, Copy)]
enum LogLevelArg {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Fault,
}

impl From<LogLevelArg> for LogLevel {
    fn from(v: LogLevelArg) -> Self {
        match v {
            LogLevelArg::Debug => LogLevel::Debug,
            LogLevelArg::Info => LogLevel::Info,
            LogLevelArg::Notice => LogLevel::Notice,
            LogLevelArg::Warning => LogLevel::Warning,
            LogLevelArg::Error => LogLevel::Error,
            LogLevelArg::Fault => LogLevel::Fault,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "quokka",
    bin_name = "quokka",
    about = "Inspect and tidy an iPhone connected to your Mac over USB.",
    long_about = "Inspect and tidy an iPhone connected to your Mac over USB.\n\
\n\
Run without a subcommand on a TTY to open the interactive launcher. \
The `qk` binary is a short alias for `quokka` and behaves identically.",
    version,
    propagate_version = true
)]
struct Cli {
    /// UDID of the iPhone to target. Required when 2+ devices are connected
    /// in a non-interactive shell. Reads from `QK_UDID` env if unset.
    #[arg(long, short = 'd', global = true, env = "QK_UDID")]
    udid: Option<String>,

    // Optional so `quokka`/`qk` with no args opens the interactive
    // launcher on a TTY. Non-TTY callers (pipes/CI) fall back to `--help`.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show device dashboard: name, model, iOS, storage, battery.
    Status,
    /// List installed user apps. Use --uninstall to remove one.
    Apps {
        /// Bundle id of an app to uninstall. Requires explicit confirmation.
        #[arg(long, value_name = "BUNDLE_ID")]
        uninstall: Option<String>,
        /// Skip the interactive confirmation for `--uninstall`.
        #[arg(long)]
        yes: bool,
    },
    /// Walk DCIM, Downloads, Recordings, Books and surface the heaviest files.
    ///
    /// Read-only by default — prints the top N files. With `--delete` on a TTY
    /// opens an interactive picker with min-size filter (≥10 MB default),
    /// substring search, and an auto-mark menu that detects Live Photo
    /// motion videos, originals with edited siblings, old screenshots, and
    /// exact duplicates.
    Analyze {
        /// Rows shown in the read-only table. Ignored in the interactive
        /// picker, which always shows the full walk filtered by min-size.
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Open the interactive deletion picker (requires a TTY). Without
        /// this flag, analyze is strictly read-only.
        #[arg(long)]
        delete: bool,
    },
    /// Print the connected iPhone's static identity in three labeled blocks.
    ///
    /// By default the output includes PII — serial number, UDID, IMEI(s), and
    /// Wi-Fi/Bluetooth MAC addresses. Pass `--redact` to mask these before
    /// sharing screenshots or piping into a paste service.
    Info {
        /// Mask serial, UDID, IMEI, and MAC addresses before printing.
        #[arg(short, long)]
        redact: bool,
    },
    /// Soft reboot the device via diagnostics_relay.
    Reboot {
        /// Skip the interactive confirmation. Required for non-TTY use.
        #[arg(short, long)]
        yes: bool,
    },
    /// Power off the device via diagnostics_relay.
    Shutdown {
        /// Skip the interactive confirmation. Required for non-TTY use.
        #[arg(short, long)]
        yes: bool,
    },
    /// Survey the AFC media area (counts/sizes per kind, per month, largest).
    Media {
        /// Also print likely-duplicate groups (exact size + kind match).
        #[arg(short = 'd', long)]
        find_duplicates: bool,
    },
    /// Stream the device's syslog.
    Logs {
        /// Disable the TUI and stream plain text to stdout.
        #[arg(long)]
        no_tui: bool,
        /// Hide entries below this level.
        #[arg(long, value_enum, default_value_t = LogLevelArg::Notice)]
        min_level: LogLevelArg,
        /// Case-insensitive substring match against the process name.
        #[arg(long)]
        process: Option<String>,
        /// Append every emitted line to this file as well.
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// List every iPhone reachable through usbmuxd (works with no --udid).
    Devices,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    // No subcommand + non-TTY (pipe/CI): the interactive launcher would be
    // invisible. Fall back to `--help` without touching the device.
    if cli.command.is_none() && !std::io::stdout().is_terminal() {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }

    // `qk devices` is the one subcommand that does not select a device.
    if matches!(cli.command, Some(Command::Devices)) {
        return commands::devices::run().await;
    }

    let device = device::connect(cli.udid.as_deref()).await?;

    match cli.command {
        None => commands::menu::run(&*device).await,
        Some(Command::Devices) => unreachable!("handled above"),
        Some(Command::Status) => commands::status::run(&*device).await,
        Some(Command::Apps { uninstall, yes }) => {
            commands::apps::run(
                &*device,
                commands::apps::Options {
                    uninstall,
                    assume_yes: yes,
                },
            )
            .await
        }
        Some(Command::Analyze { top, delete }) => {
            commands::analyze::run(&*device, top, delete).await
        }
        Some(Command::Info { redact }) => commands::info::run(&*device, redact).await,
        Some(Command::Reboot { yes }) => {
            commands::power::run(&*device, commands::power::Action::Reboot, yes).await
        }
        Some(Command::Shutdown { yes }) => {
            commands::power::run(&*device, commands::power::Action::Shutdown, yes).await
        }
        Some(Command::Media { find_duplicates }) => {
            commands::media::run(&*device, find_duplicates).await
        }
        Some(Command::Logs {
            no_tui,
            min_level,
            process,
            save,
        }) => {
            commands::logs::run(
                &*device,
                commands::logs::Options {
                    no_tui,
                    min_level: min_level.into(),
                    process_filter: process,
                    save_path: save,
                },
            )
            .await
        }
    }
}
