//! quokka — CLI to inspect and tidy an iPhone connected to a Mac over USB.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

pub mod commands;
pub mod device;
pub mod ui;

use crate::device::LogLevel;

/// Transport protocol surface for `qk capture --proto`. Mirrors the
/// renderable variants of [`commands::capture::Protocol`] minus `Other`,
/// which isn't a useful filter target.
#[derive(ValueEnum, Debug, Clone, Copy)]
enum ProtoArg {
    Tcp,
    Udp,
    Icmp,
}

impl From<ProtoArg> for commands::capture::Protocol {
    fn from(v: ProtoArg) -> Self {
        match v {
            ProtoArg::Tcp => commands::capture::Protocol::Tcp,
            ProtoArg::Udp => commands::capture::Protocol::Udp,
            ProtoArg::Icmp => commands::capture::Protocol::Icmp,
        }
    }
}

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

    /// Emit machine-readable JSON instead of the human dashboard. Currently
    /// honored by `info` and `devices`; other subcommands ignore it.
    #[arg(long, global = true)]
    json: bool,

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
    /// Stream network packets from `com.apple.pcapd` with per-packet
    /// process info (PID + comm). Press Ctrl-C to stop.
    Capture {
        /// Stop after capturing this many packets. Useful for smoke tests.
        #[arg(long, value_name = "N")]
        max: Option<usize>,
        /// Also write every packet to a file. Extension picks the format:
        /// `.pcap` for classic pcap, anything else (default) for pcapng
        /// with per-packet process info in the comment field. Wireshark
        /// can filter on `frame.comment contains "Instagram"`.
        #[arg(long, value_name = "PATH")]
        save: Option<PathBuf>,
        /// Case-insensitive substring against the process name. Matches
        /// "Instagram", "InstagramShare", etc. for `--app instagram`.
        #[arg(long, value_name = "NAME")]
        app: Option<String>,
        /// Exact PID match.
        #[arg(long, value_name = "PID")]
        pid: Option<u32>,
        /// Source or destination port match.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// Filter by transport protocol.
        #[arg(long, value_enum, value_name = "PROTO")]
        proto: Option<ProtoArg>,
        /// Case-insensitive substring against the interface name. `utun`
        /// matches `utun0`, `utun4`, etc.
        #[arg(long, value_name = "NAME")]
        interface: Option<String>,
        /// Aggregate by process + remote host. Periodically clears the
        /// screen and reprints (top-style). Ctrl-C prints the final
        /// snapshot.
        #[arg(long, conflicts_with_all = ["dns", "sni", "save"])]
        hosts: bool,
        /// Extract DNS queries from UDP port 53 only. One line per query.
        #[arg(long, conflicts_with_all = ["hosts", "sni", "save"])]
        dns: bool,
        /// Extract TLS SNI (Server Name Indication) from TCP port 443
        /// ClientHellos. One line per hello.
        #[arg(long, conflicts_with_all = ["hosts", "dns", "save"])]
        sni: bool,
    },
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

    // `qk devices` is the one subcommand that does not select a device,
    // so handle it before reaching for the connection.
    if matches!(cli.command, Some(Command::Devices)) {
        return commands::devices::run(cli.json).await;
    }

    let device = device::connect(cli.udid.as_deref()).await?;

    match cli.command {
        None => commands::menu::run(&*device).await,
        Some(Command::Devices) => {
            // Already handled above; keeping the arm satisfies the
            // exhaustiveness check without an unreachable!().
            Ok(())
        }
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
        Some(Command::Info { redact }) => commands::info::run(&*device, redact, cli.json).await,
        Some(Command::Reboot { yes }) => {
            commands::power::run(&*device, commands::power::Action::Reboot, yes).await
        }
        Some(Command::Shutdown { yes }) => {
            commands::power::run(&*device, commands::power::Action::Shutdown, yes).await
        }
        Some(Command::Media { find_duplicates }) => {
            commands::media::run(&*device, find_duplicates).await
        }
        Some(Command::Capture {
            max,
            save,
            app,
            pid,
            port,
            proto,
            interface,
            hosts,
            dns,
            sni,
        }) => {
            // Clap's `conflicts_with_all` guarantees at most one of the
            // mode flags is true, so the `if/else if` chain is safe.
            let mode = if hosts {
                commands::capture::Mode::Hosts
            } else if dns {
                commands::capture::Mode::Dns
            } else if sni {
                commands::capture::Mode::Sni
            } else {
                commands::capture::Mode::Stream
            };
            commands::capture::run(
                &*device,
                commands::capture::Options {
                    max,
                    save,
                    filter: commands::capture::Filter {
                        app,
                        pid,
                        port,
                        proto: proto.map(Into::into),
                        interface,
                    },
                    mode,
                },
            )
            .await
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
    fn capture_parses_with_and_without_max() {
        assert!(matches!(
            parse(&["capture"]).command,
            Some(Command::Capture {
                max: None,
                save: None,
                ..
            })
        ));
        assert!(matches!(
            parse(&["capture", "--max", "10"]).command,
            Some(Command::Capture {
                max: Some(10),
                save: None,
                ..
            })
        ));
        // --max without a value or with a non-numeric value must fail.
        assert!(Cli::try_parse_from(["quokka", "capture", "--max"]).is_err());
        assert!(Cli::try_parse_from(["quokka", "capture", "--max", "lots"]).is_err());
    }

    #[test]
    fn capture_save_is_pathbuf_and_optional() {
        match parse(&["capture", "--save", "out.pcapng"]).command {
            Some(Command::Capture {
                max: None, save, ..
            }) => {
                assert_eq!(save.as_deref(), Some(std::path::Path::new("out.pcapng")));
            }
            other => panic!("expected Capture with save, got {other:?}"),
        }
        // --save without a value must fail.
        assert!(Cli::try_parse_from(["quokka", "capture", "--save"]).is_err());
    }

    #[test]
    fn capture_mode_flags_are_mutually_exclusive() {
        // Bare capture parses (Stream is the default — no flag).
        assert!(matches!(
            parse(&["capture"]).command,
            Some(Command::Capture {
                hosts: false,
                dns: false,
                sni: false,
                ..
            })
        ));
        // Each mode flag parses on its own.
        assert!(matches!(
            parse(&["capture", "--hosts"]).command,
            Some(Command::Capture { hosts: true, .. })
        ));
        assert!(matches!(
            parse(&["capture", "--dns"]).command,
            Some(Command::Capture { dns: true, .. })
        ));
        assert!(matches!(
            parse(&["capture", "--sni"]).command,
            Some(Command::Capture { sni: true, .. })
        ));
        // But combining any two must fail — spec calls these "mutually
        // exclusive".
        assert!(Cli::try_parse_from(["quokka", "capture", "--hosts", "--dns"]).is_err());
        assert!(Cli::try_parse_from(["quokka", "capture", "--dns", "--sni"]).is_err());
        assert!(Cli::try_parse_from(["quokka", "capture", "--hosts", "--sni"]).is_err());
        // --save is incompatible with aggregation/extraction modes (per
        // spec: "Modos NÃO incluem --save ainda").
        assert!(Cli::try_parse_from(["quokka", "capture", "--hosts", "--save", "x.pcap"]).is_err());
    }

    #[test]
    fn capture_filter_flags_parse_into_options() {
        match parse(&[
            "capture",
            "--app",
            "instagram",
            "--pid",
            "4521",
            "--port",
            "443",
            "--proto",
            "tcp",
            "--interface",
            "en0",
        ])
        .command
        {
            Some(Command::Capture {
                app,
                pid,
                port,
                proto,
                interface,
                ..
            }) => {
                assert_eq!(app.as_deref(), Some("instagram"));
                assert_eq!(pid, Some(4521));
                assert_eq!(port, Some(443));
                assert!(matches!(proto, Some(ProtoArg::Tcp)));
                assert_eq!(interface.as_deref(), Some("en0"));
            }
            other => panic!("expected Capture with filters, got {other:?}"),
        }
        // --proto enforces the value enum.
        assert!(Cli::try_parse_from(["quokka", "capture", "--proto", "bogus"]).is_err());
        // --port and --pid must be numeric.
        assert!(Cli::try_parse_from(["quokka", "capture", "--port", "lots"]).is_err());
        assert!(Cli::try_parse_from(["quokka", "capture", "--pid", "lots"]).is_err());
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
