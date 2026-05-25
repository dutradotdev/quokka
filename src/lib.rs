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
    #[arg(long, global = true, env = "QK_UDID")]
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

#[cfg(test)]
mod cli_tests {
    //! Parser surface tests. The point is to fail loud when a PR renames a
    //! flag, changes a default, removes a subcommand alias, or shifts a
    //! flag's short form — every one of those is a user-visible break that
    //! `cargo build` happily lets through.
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("quokka").chain(args.iter().copied()))
            .expect("argv should parse")
    }

    #[test]
    fn clap_definition_is_internally_consistent() {
        // clap's debug_assert panics on conflicting/duplicate definitions.
        // Cheaper than enumerating every flag, and runs once per `cargo test`.
        Cli::command().debug_assert();
    }

    #[test]
    fn bare_invocation_has_no_subcommand() {
        let cli = parse(&[]);
        assert!(cli.command.is_none());
        assert!(cli.udid.is_none());
    }

    #[test]
    fn global_udid_flag_is_long_only_and_reads_env() {
        let cli = parse(&["--udid", "ABC", "status"]);
        assert_eq!(cli.udid.as_deref(), Some("ABC"));
        // `-d` must NOT alias `--udid` — it belongs to `media --find-duplicates`.
        // A regression here would silently steal `qk media -d`.
        assert!(Cli::try_parse_from(["quokka", "-d", "ABC", "status"]).is_err());
        let cmd = Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "udid")
            .expect("udid arg should exist");
        assert!(arg.is_global_set(), "--udid must remain global");
        assert!(
            arg.get_short().is_none(),
            "--udid must not have a short form (collides with `media -d`)"
        );
        let env = arg.get_env().expect("--udid must read from env");
        assert_eq!(env, "QK_UDID");
    }

    #[test]
    fn apps_uninstall_requires_a_value_and_defaults_yes_to_false() {
        let cli = parse(&["apps"]);
        assert!(matches!(
            cli.command,
            Some(Command::Apps {
                uninstall: None,
                yes: false
            })
        ));
        let cli = parse(&["apps", "--uninstall", "com.x"]);
        match cli.command {
            Some(Command::Apps { uninstall, yes }) => {
                assert_eq!(uninstall.as_deref(), Some("com.x"));
                assert!(!yes);
            }
            other => panic!("expected Apps, got {other:?}"),
        }
        // --uninstall without a value must fail.
        assert!(Cli::try_parse_from(["quokka", "apps", "--uninstall"]).is_err());
    }

    #[test]
    fn analyze_top_defaults_to_20_and_delete_defaults_false() {
        let cli = parse(&["analyze"]);
        assert!(matches!(
            cli.command,
            Some(Command::Analyze {
                top: 20,
                delete: false
            })
        ));
        let cli = parse(&["analyze", "--top", "50", "--delete"]);
        assert!(matches!(
            cli.command,
            Some(Command::Analyze {
                top: 50,
                delete: true
            })
        ));
        // Non-numeric --top must fail.
        assert!(Cli::try_parse_from(["quokka", "analyze", "--top", "lots"]).is_err());
    }

    #[test]
    fn info_redact_has_short_and_long_form() {
        assert!(matches!(
            parse(&["info"]).command,
            Some(Command::Info { redact: false })
        ));
        assert!(matches!(
            parse(&["info", "--redact"]).command,
            Some(Command::Info { redact: true })
        ));
        assert!(matches!(
            parse(&["info", "-r"]).command,
            Some(Command::Info { redact: true })
        ));
    }

    #[test]
    fn power_commands_yes_short_and_long() {
        assert!(matches!(
            parse(&["reboot"]).command,
            Some(Command::Reboot { yes: false })
        ));
        assert!(matches!(
            parse(&["reboot", "--yes"]).command,
            Some(Command::Reboot { yes: true })
        ));
        assert!(matches!(
            parse(&["reboot", "-y"]).command,
            Some(Command::Reboot { yes: true })
        ));
        assert!(matches!(
            parse(&["shutdown", "-y"]).command,
            Some(Command::Shutdown { yes: true })
        ));
    }

    #[test]
    fn media_find_duplicates_short_and_long() {
        assert!(matches!(
            parse(&["media"]).command,
            Some(Command::Media {
                find_duplicates: false
            })
        ));
        assert!(matches!(
            parse(&["media", "-d"]).command,
            Some(Command::Media {
                find_duplicates: true
            })
        ));
        assert!(matches!(
            parse(&["media", "--find-duplicates"]).command,
            Some(Command::Media {
                find_duplicates: true
            })
        ));
    }

    #[test]
    fn logs_defaults_and_min_level_value_enum() {
        let cli = parse(&["logs"]);
        match cli.command {
            Some(Command::Logs {
                no_tui,
                min_level,
                process,
                save,
            }) => {
                assert!(!no_tui);
                assert!(matches!(min_level, LogLevelArg::Notice));
                assert!(process.is_none());
                assert!(save.is_none());
            }
            other => panic!("expected Logs, got {other:?}"),
        }
        // Each variant must accept its kebab-case form.
        for lvl in ["debug", "info", "notice", "warning", "error", "fault"] {
            assert!(
                Cli::try_parse_from(["quokka", "logs", "--min-level", lvl]).is_ok(),
                "logs --min-level {lvl} should parse"
            );
        }
        // And reject unknown levels.
        assert!(Cli::try_parse_from(["quokka", "logs", "--min-level", "trace"]).is_err());
    }

    #[test]
    fn logs_save_path_is_pathbuf_and_process_filter_is_passed_through() {
        let cli = parse(&[
            "logs",
            "--no-tui",
            "--process",
            "SpringBoard",
            "--save",
            "/tmp/x.log",
        ]);
        match cli.command {
            Some(Command::Logs {
                no_tui,
                process,
                save,
                ..
            }) => {
                assert!(no_tui);
                assert_eq!(process.as_deref(), Some("SpringBoard"));
                assert_eq!(save.as_deref(), Some(std::path::Path::new("/tmp/x.log")));
            }
            other => panic!("expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn devices_subcommand_parses_with_no_args() {
        assert!(matches!(
            parse(&["devices"]).command,
            Some(Command::Devices)
        ));
    }

    #[test]
    fn unknown_subcommand_is_an_error() {
        assert!(Cli::try_parse_from(["quokka", "doesnotexist"]).is_err());
    }

    #[test]
    fn log_level_arg_round_trips_to_device_log_level() {
        // Bit map so a future drift between LogLevelArg and LogLevel surfaces here.
        let pairs = [
            (LogLevelArg::Debug, LogLevel::Debug),
            (LogLevelArg::Info, LogLevel::Info),
            (LogLevelArg::Notice, LogLevel::Notice),
            (LogLevelArg::Warning, LogLevel::Warning),
            (LogLevelArg::Error, LogLevel::Error),
            (LogLevelArg::Fault, LogLevel::Fault),
        ];
        for (arg, expected) in pairs {
            assert_eq!(LogLevel::from(arg), expected);
        }
    }
}
