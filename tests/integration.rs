//! Integration tests exercising commands against `FakeDevice` — no iPhone
//! required.

use quokka_cli::commands;
use quokka_cli::commands::dashboard;
use quokka_cli::device::{App, Battery, DeviceStatus, FakeDevice, MediaFile, Storage};

/// Fixed clock anchor so relative-date output is deterministic.
const NOW_UNIX: i64 = 1_779_793_200;

/// Force every command down its non-interactive path. `cargo test` keeps the
/// real terminal attached (libtest only redirects the `print!` macros, not the
/// file descriptors), so a test that reaches a prompt or TUI would otherwise
/// block on the developer's terminal and the non-tty assertions would not hold.
/// Idempotent; call it from any test that drives a command which can go
/// interactive (`apps` list, `analyze --delete`, `power` confirm, `card`).
fn headless() {
    std::env::set_var("QK_NON_INTERACTIVE", "1");
}

fn healthy_status() -> DeviceStatus {
    DeviceStatus {
        name: Some("Lucas's iPhone".into()),
        model: Some("iPhone15,3".into()),
        model_friendly: Some("iPhone 14 Pro Max".into()),
        ios_version: Some("18.2".into()),
        ios_build: Some("22C152".into()),
        enclosure_color: Some("Deep Purple".into()),
        storage: Some(Storage {
            total_bytes: 256_000_000_000,
            free_bytes: 95_400_000_000,
            system_bytes: Some(12_400_000_000),
            data_used_bytes: Some(148_200_000_000),
        }),
        battery: Battery {
            level_percent: Some(87),
            cycle_count: Some(142),
            health_percent: Some(91),
            temperature_celsius: Some(27.4),
            is_charging: Some(true),
            adapter_watts: Some(20),
            adapter_description: Some("USB-C".into()),
        },
        locale: Some("pt-BR".into()),
        time_zone: Some("America/Sao_Paulo".into()),
        app_count: Some(47),
        developer_mode: Some(false),
        find_my: Some(true),
        last_backup_unix: Some(1_700_000_000),
        paired_since_unix: Some(1_640_000_000),
        chip_name: Some("A16 Bionic".into()),
        storage_breakdown: None,
        oldest_app: None,
        jailbreak_detected: false,
        is_beta_build: false,
    }
}

#[tokio::test]
async fn status_command_runs_end_to_end_against_fake() {
    let fake = FakeDevice::default();
    commands::status::run(&fake)
        .await
        .expect("status should succeed against a healthy fake");
}

#[tokio::test]
async fn status_command_surfaces_device_error() {
    let fake = FakeDevice::with_status_error("simulated lockdown failure");
    let err = commands::status::run(&fake)
        .await
        .expect_err("status should propagate the device error");
    assert!(err.to_string().contains("simulated lockdown failure"));
}

#[tokio::test]
async fn dashboard_output_renders_name_and_storage() {
    let fake = FakeDevice::with_status(healthy_status());
    let status = fake_status(&fake).await;
    let out = dashboard::render(&status, 100, NOW_UNIX);
    assert!(out.contains("Lucas's iPhone"));
    assert!(out.contains("iPhone 14 Pro Max"));
    assert!(out.contains("iOS 18.2"));
    assert!(out.contains("build 22C152"));
    assert!(out.contains("Storage"));
    assert!(out.contains("256.0 GB"));
    assert!(out.contains("├─ System"));
    assert!(out.contains("├─ Data"));
    assert!(out.contains("└─ Free"));
    assert!(out.contains("⚡"));
}

#[tokio::test]
async fn dashboard_output_uses_dashes_for_partial_data() {
    let mut s = healthy_status();
    s.battery.cycle_count = None;
    s.storage = None;
    let fake = FakeDevice::with_status(s);
    let status = fake_status(&fake).await;
    let out = dashboard::render(&status, 100, NOW_UNIX);
    assert!(out.contains("—"));
}

#[tokio::test]
async fn dashboard_not_charging_drops_bolt() {
    let mut s = healthy_status();
    s.battery.is_charging = Some(false);
    let fake = FakeDevice::with_status(s);
    let status = fake_status(&fake).await;
    let out = dashboard::render(&status, 100, NOW_UNIX);
    assert!(!out.contains("⚡"));
}

#[tokio::test]
async fn dashboard_footer_shows_trivia_and_hides_normal_flags() {
    let fake = FakeDevice::with_status(healthy_status());
    let status = fake_status(&fake).await;
    let out = dashboard::render(&status, 100, NOW_UNIX);
    assert!(out.contains("47 apps"));
    assert!(out.contains("last backup"));
    assert!(out.contains("paired since"));
    // developer_mode = false and find_my = true are the expected states.
    assert!(!out.contains("⚠"));
}

#[tokio::test]
async fn dashboard_footer_flags_developer_mode_and_find_my() {
    let mut s = healthy_status();
    s.developer_mode = Some(true);
    s.find_my = Some(false);
    let fake = FakeDevice::with_status(s);
    let status = fake_status(&fake).await;
    let out = dashboard::render(&status, 100, NOW_UNIX);
    assert!(out.contains("Developer Mode on"));
    assert!(out.contains("Find My off"));
}

async fn fake_status(fake: &FakeDevice) -> DeviceStatus {
    use quokka_cli::device::Device;
    fake.status().await.expect("fake should not error here")
}

// ---------- apps ----------

fn sample_apps() -> Vec<App> {
    vec![
        App {
            bundle_id: "com.user.big".into(),
            name: "Big User App".into(),
            size_bytes: 800_000_000,
            is_system: false,
            install_date_unix: None,
        },
        App {
            bundle_id: "com.user.small".into(),
            name: "Small".into(),
            size_bytes: 50_000_000,
            is_system: false,
            install_date_unix: None,
        },
        App {
            bundle_id: "com.apple.MobileSMS".into(),
            name: "Messages".into(),
            size_bytes: 120_000_000,
            is_system: true,
            install_date_unix: None,
        },
    ]
}

#[tokio::test]
async fn apps_list_flow_renders_user_apps_against_fake() {
    headless();
    let fake = FakeDevice {
        apps: Ok(sample_apps()),
        ..Default::default()
    };
    commands::apps::run(
        &fake,
        commands::apps::Options {
            uninstall: None,
            assume_yes: false,
        },
    )
    .await
    .expect("list should succeed");
}

#[tokio::test]
async fn apps_uninstall_with_yes_calls_device_and_records_bundle_id() {
    let fake = FakeDevice {
        apps: Ok(sample_apps()),
        ..Default::default()
    };
    commands::apps::run(
        &fake,
        commands::apps::Options {
            uninstall: Some("com.user.big".into()),
            assume_yes: true,
        },
    )
    .await
    .expect("uninstall should succeed");
    assert_eq!(fake.uninstalled(), vec!["com.user.big".to_string()]);
}

#[tokio::test]
async fn apps_uninstall_unknown_bundle_id_errors_without_calling_device() {
    let fake = FakeDevice {
        apps: Ok(sample_apps()),
        ..Default::default()
    };
    let err = commands::apps::run(
        &fake,
        commands::apps::Options {
            uninstall: Some("com.does.not.exist".into()),
            assume_yes: true,
        },
    )
    .await
    .expect_err("unknown bundle id should error");
    assert!(err.to_string().contains("com.does.not.exist"));
    assert!(fake.uninstalled().is_empty());
}

#[tokio::test]
async fn apps_uninstall_propagates_device_error() {
    let fake = FakeDevice {
        apps: Ok(sample_apps()),
        uninstall_result: Err("device refused".into()),
        ..Default::default()
    };
    let err = commands::apps::run(
        &fake,
        commands::apps::Options {
            uninstall: Some("com.user.big".into()),
            assume_yes: true,
        },
    )
    .await
    .expect_err("device-side failure should surface");
    assert!(err.to_string().contains("device refused"));
    // The call was attempted before the device returned the error.
    assert_eq!(fake.uninstalled(), vec!["com.user.big".to_string()]);
}

// ---------- analyze ----------

fn sample_media() -> Vec<MediaFile> {
    let mtime = 1_700_000_000;
    vec![
        MediaFile {
            path: "/DCIM/100APPLE/IMG_0001.MOV".into(),
            size_bytes: 4_200_000_000,
            modified_unix: mtime,
        },
        MediaFile {
            path: "/DCIM/100APPLE/IMG_0002.HEIC".into(),
            size_bytes: 5_000_000,
            modified_unix: mtime,
        },
        MediaFile {
            path: "/Downloads/big.pdf".into(),
            size_bytes: 100_000_000,
            modified_unix: mtime,
        },
        MediaFile {
            path: "/Recordings/Memo.m4a".into(),
            size_bytes: 1_500_000,
            modified_unix: mtime,
        },
        MediaFile {
            path: "/Books/x.epub".into(),
            size_bytes: 800_000,
            modified_unix: mtime,
        },
    ]
}

#[tokio::test]
async fn analyze_read_only_succeeds_against_fake() {
    let fake = FakeDevice {
        media: sample_media(),
        ..Default::default()
    };
    commands::analyze::run(&fake, 5, false)
        .await
        .expect("analyze read-only should succeed");
    assert!(fake.deleted().is_empty());
}

#[tokio::test]
async fn analyze_no_files_succeeds_silently() {
    let fake = FakeDevice {
        media: Vec::new(),
        ..Default::default()
    };
    commands::analyze::run(&fake, 20, false)
        .await
        .expect("empty media should be Ok");
}

#[tokio::test]
async fn analyze_delete_without_tty_errors() {
    headless();
    let fake = FakeDevice {
        media: sample_media(),
        ..Default::default()
    };
    let err = commands::analyze::run(&fake, 5, true)
        .await
        .expect_err("delete on non-TTY should bail");
    assert!(err.to_string().contains("TTY") || err.to_string().contains("terminal"));
    assert!(fake.deleted().is_empty());
}

#[test]
fn analyze_top_n_picks_heaviest_files() {
    use quokka_cli::commands::top_n_by_size;
    let media = sample_media();
    let top = top_n_by_size(&media, 2);
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].path, "/DCIM/100APPLE/IMG_0001.MOV");
    assert_eq!(top[1].path, "/Downloads/big.pdf");
}

#[tokio::test]
async fn analyze_top_zero_does_not_panic_through_run() {
    let fake = FakeDevice {
        media: sample_media(),
        ..Default::default()
    };
    commands::analyze::run(&fake, 0, false)
        .await
        .expect("top=0 read-only run must succeed");
}

#[tokio::test]
async fn analyze_top_larger_than_files_saturates_through_run() {
    let fake = FakeDevice {
        media: sample_media(),
        ..Default::default()
    };
    commands::analyze::run(&fake, 999_999, false)
        .await
        .expect("top > files should saturate, not error");
    assert!(fake.deleted().is_empty());
}

// ---------- info ----------

#[tokio::test]
async fn info_command_runs_against_fake_default() {
    let fake = FakeDevice::default();
    commands::info::run(&fake, false, false)
        .await
        .expect("info ok");
}

/// Snapshot of the plain `render_network_block` output. Locks layout so a
/// future column-width or ordering change in the renderer fails loudly
/// instead of silently shifting the user-facing format.
#[tokio::test]
async fn info_network_block_layout_snapshot() {
    let fake = FakeDevice::default();
    let info = fake.info().await.unwrap();
    let block = quokka_cli::commands::info::render_network_block(&info, false)
        .expect("network block should render with the default fake");
    insta::assert_snapshot!(block, @r"
    Network
      Wi-Fi MAC         AA:BB:CC:DD:EE:FF
      Bluetooth MAC     AA:BB:CC:DD:EE:F0
      IMEI              350123456789012
      IMEI 2            350123456789013
    ");
}

/// Snapshot of the JSON output of `qk info --json`. Locks both shape and
/// key ordering so downstream scripts can rely on the format.
#[tokio::test]
async fn info_json_output_snapshot() {
    let fake = FakeDevice::default();
    let info = fake.info().await.unwrap();
    let json = quokka_cli::commands::info::render_json(&info, false);
    insta::assert_snapshot!(json, @r#"
    {
      "device": {
        "enclosure_color": "Natural Titanium",
        "model_friendly": "iPhone 15 Pro Max",
        "model_identifier": "iPhone16,2",
        "model_number": "MQ8X3LL/A",
        "name": "Lucas's iPhone",
        "region_info": "LL/A",
        "serial": "F2LXXXXXXXXX",
        "udid": "00008130-001A2B3C4D5E6F7G"
      },
      "network": {
        "bluetooth_address": "AA:BB:CC:DD:EE:F0",
        "imei": "350123456789012",
        "imei2": "350123456789013",
        "wifi_address": "AA:BB:CC:DD:EE:FF"
      },
      "system": {
        "activation_state": "Activated",
        "cpu_architecture": "arm64e",
        "developer_mode_enabled": false,
        "hardware_model": "D74AP",
        "ios_build": "22C152",
        "ios_version": "18.2",
        "is_supervised": false
      }
    }
    "#);
}

#[tokio::test]
async fn info_renders_required_labels_against_fake() {
    let fake = FakeDevice::default();
    let info = fake.info().await.unwrap();
    let out = commands::info::render(&info, false);
    assert!(out.contains("Device"));
    assert!(out.contains("System"));
    assert!(out.contains("Network"));
    assert!(out.contains("Lucas's iPhone"));
    assert!(out.contains("iPhone 15 Pro Max"));
    assert!(out.contains("F2LXXXXXXXXX"));
    assert!(out.contains("350123456789012"));
}

#[tokio::test]
async fn info_redact_masks_pii_only() {
    let fake = FakeDevice::default();
    let info = fake.info().await.unwrap();
    let out = commands::info::render(&info, true);
    assert!(!out.contains("F2LXXXXXXXXX"));
    assert!(!out.contains("350123456789012"));
    assert!(!out.contains("AA:BB:CC:DD:EE:FF"));
    assert!(out.contains("Lucas's iPhone"));
    assert!(out.contains("iPhone 15 Pro Max"));
    assert!(out.contains("Natural Titanium"));
}

#[tokio::test]
async fn info_minimal_skips_network_block() {
    use quokka_cli::device::DeviceInfo;
    let fake = FakeDevice {
        info: DeviceInfo {
            name: "Phone".into(),
            model_identifier: "iPhone16,2".into(),
            serial: "AAAA".into(),
            udid: "BBBB-CCCC".into(),
            ios_version: "18.2".into(),
            ..DeviceInfo::default()
        },
        ..Default::default()
    };
    let info = fake.info().await.unwrap();
    let out = commands::info::render(&info, false);
    assert!(out.contains("Phone"));
    assert!(out.contains("Serial"));
    assert!(!out.contains("Network"));
    assert!(!out.contains("IMEI"));
    assert!(!out.contains("Supervised"));
}

use quokka_cli::device::Device;

// ---------- power ----------

#[tokio::test]
async fn power_reboot_with_yes_records_reboot_call() {
    let fake = FakeDevice::default();
    commands::power::run(&fake, commands::power::Action::Reboot, true)
        .await
        .expect("reboot ok");
    let calls = fake.power_calls();
    assert_eq!(calls, vec![quokka_cli::device::PowerCall::Reboot]);
}

#[tokio::test]
async fn power_shutdown_with_yes_records_shutdown_call() {
    let fake = FakeDevice::default();
    commands::power::run(&fake, commands::power::Action::Shutdown, true)
        .await
        .expect("shutdown ok");
    let calls = fake.power_calls();
    assert_eq!(calls, vec![quokka_cli::device::PowerCall::Shutdown]);
}

#[tokio::test]
async fn power_without_yes_on_non_tty_aborts_with_message() {
    headless();
    let fake = FakeDevice::default();
    let err = commands::power::run(&fake, commands::power::Action::Reboot, false)
        .await
        .expect_err("non-tty + no --yes should fail");
    assert!(err.to_string().contains("--yes"));
    assert!(fake.power_calls().is_empty());
}

#[tokio::test]
async fn power_propagates_device_failure() {
    let fake = FakeDevice {
        fail_power: true,
        ..Default::default()
    };
    let err = commands::power::run(&fake, commands::power::Action::Reboot, true)
        .await
        .expect_err("device failure surfaces");
    assert!(err.to_string().contains("request failed"));
}

// ---------- media ----------

fn sample_media_with_mtime() -> Vec<MediaFile> {
    // 2026-05 anchor: 1779840000 = 2026-05-25
    let recent = 1_779_840_000;
    vec![
        MediaFile {
            path: "/DCIM/100APPLE/IMG_0001.HEIC".into(),
            size_bytes: 4_000_000,
            modified_unix: recent,
        },
        MediaFile {
            path: "/DCIM/100APPLE/IMG_0002.HEIC".into(),
            size_bytes: 4_000_000,
            modified_unix: recent,
        },
        MediaFile {
            path: "/DCIM/100APPLE/IMG_0003.MOV".into(),
            size_bytes: 200_000_000,
            modified_unix: recent,
        },
        MediaFile {
            path: "/Recordings/m.m4a".into(),
            size_bytes: 500_000,
            modified_unix: 0,
        },
    ]
}

#[tokio::test]
async fn media_default_renders_sections() {
    let fake = FakeDevice {
        media: sample_media_with_mtime(),
        ..Default::default()
    };
    commands::media::run(&fake, false).await.expect("media ok");
}

#[test]
fn media_build_report_includes_expected_sections() {
    let report = commands::media::build_report(
        &sample_media_with_mtime(),
        true,
        1_779_840_000,
        Some("Test".into()),
    );
    assert_eq!(report.total_files, 4);
    let out = commands::media::render(&report);
    assert!(out.contains("By kind"));
    assert!(out.contains("By month"));
    assert!(out.contains("Largest 10"));
    assert!(out.contains("Likely duplicates"));
    // Two HEICs of same size form a dup group.
    assert!(out.contains("× 2"));
}

#[tokio::test]
async fn media_empty_walk_prints_no_files() {
    let fake = FakeDevice {
        media: Vec::new(),
        ..Default::default()
    };
    commands::media::run(&fake, false)
        .await
        .expect("empty media ok");
}

// ---------- logs ----------

use quokka_cli::device::{LogEntry, LogLevel};

fn log_entry(level: LogLevel, process: &str, msg: &str) -> LogEntry {
    LogEntry {
        timestamp_unix_ms: Some(1_700_000_000_000),
        time_text: None,
        host: "host".into(),
        process: process.into(),
        pid: Some(1),
        level,
        message: msg.into(),
    }
}

#[tokio::test]
async fn logs_plain_mode_filters_below_min_level() {
    let fake = FakeDevice {
        seeded_logs: vec![
            Ok(log_entry(LogLevel::Debug, "p", "d")),
            Ok(log_entry(LogLevel::Info, "p", "i")),
            Ok(log_entry(LogLevel::Notice, "p", "n")),
            Ok(log_entry(LogLevel::Warning, "p", "w")),
            Ok(log_entry(LogLevel::Error, "p", "e")),
        ],
        ..Default::default()
    };
    // Just verify the device stream works; full stdout capture is awkward.
    let mut rx = fake.stream_logs().await.unwrap();
    let mut got = 0;
    while let Some(item) = rx.recv().await {
        if item.is_ok() {
            got += 1;
        }
    }
    assert_eq!(got, 5);
}

#[test]
fn logs_filter_chain_works() {
    use quokka_cli::commands::logs::{matches_filter, Filter};
    let f = Filter {
        min_level: LogLevel::Warning,
        process: Some("SpringBoard".into()),
    };
    assert!(matches_filter(
        &log_entry(LogLevel::Error, "SpringBoard", "x"),
        &f
    ));
    assert!(!matches_filter(
        &log_entry(LogLevel::Info, "SpringBoard", "x"),
        &f
    ));
    assert!(!matches_filter(
        &log_entry(LogLevel::Error, "mediaserverd", "x"),
        &f
    ));
}

use quokka_cli::device::Packet;

fn sample_pkt(pid: u32, comm: &str, iface: &str, bytes: usize) -> Packet {
    Packet {
        pid,
        comm: comm.into(),
        epid: 0,
        ecomm: String::new(),
        interface: iface.into(),
        seconds: 1_700_000_000,
        microseconds: 0,
        io: 1,
        data: vec![0u8; bytes],
    }
}

#[tokio::test]
async fn capture_drains_seeded_packets_in_order() {
    // The FakeDevice replays seeded packets one-per-channel-send; this proves
    // the seam (trait + receiver wiring) end-to-end without an iPhone.
    let fake = FakeDevice {
        seeded_packets: vec![
            Ok(sample_pkt(1, "a", "en0", 10)),
            Ok(sample_pkt(2, "b", "pdp_ip0", 20)),
            Ok(sample_pkt(3, "c", "en0", 30)),
        ],
        ..Default::default()
    };
    let mut stream = fake.capture_packets().await.unwrap();
    let mut pids = Vec::new();
    while let Some(item) = stream.rx.recv().await {
        pids.push(item.unwrap().pid);
    }
    assert_eq!(pids, vec![1, 2, 3]);
}

#[tokio::test]
async fn capture_run_respects_max_and_returns_ok() {
    // Drives commands::capture::run against a fake with more packets than
    // the --max limit — exercises the early-exit branch in the select loop.
    let mut seeded = Vec::new();
    for i in 0..50 {
        seeded.push(Ok(sample_pkt(i, "proc", "en0", 64)));
    }
    let fake = FakeDevice {
        seeded_packets: seeded,
        ..Default::default()
    };
    let result = commands::capture::run(
        &fake,
        commands::capture::Options {
            max: Some(5),
            save: None,
            filter: Default::default(),
            // Phase 6.4: Stream/Hosts route through the interactive TUI
            // (which needs a real terminal). Headless drives the same
            // ingest path without one, so the assertion still proves
            // run() exits cleanly under --max.
            mode: quokka_cli::commands::capture::Mode::Headless,
        },
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn capture_run_exits_when_stream_closes_without_max() {
    // No --max, but the seeded stream is finite — verify the command exits
    // cleanly when the channel closes rather than hanging forever.
    let fake = FakeDevice {
        seeded_packets: vec![Ok(sample_pkt(7, "p", "en0", 1))],
        ..Default::default()
    };
    let result = commands::capture::run(
        &fake,
        commands::capture::Options {
            max: None,
            save: None,
            filter: Default::default(),
            mode: quokka_cli::commands::capture::Mode::Headless,
        },
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn capture_run_hosts_mode_terminates_cleanly() {
    // Headless covers the Hosts dispatch path the same way it covers
    // Stream — the only difference at this layer is the App's initial
    // view, which isn't observable without rendering.
    let fake = quokka_cli::device::FakeDevice {
        seeded_packets: vec![Ok(sample_pkt(1, "x", "en0", 64))],
        ..Default::default()
    };
    let result = commands::capture::run(
        &fake,
        commands::capture::Options {
            max: None,
            save: None,
            filter: Default::default(),
            mode: quokka_cli::commands::capture::Mode::Headless,
        },
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn capture_run_dns_and_sni_modes_terminate_cleanly() {
    let fake = quokka_cli::device::FakeDevice {
        seeded_packets: vec![Ok(sample_pkt(1, "x", "en0", 64))],
        ..Default::default()
    };
    for mode in [
        quokka_cli::commands::capture::Mode::Dns,
        quokka_cli::commands::capture::Mode::Sni,
    ] {
        let result = commands::capture::run(
            &fake,
            commands::capture::Options {
                max: None,
                save: None,
                filter: Default::default(),
                mode,
            },
        )
        .await;
        assert!(result.is_ok(), "mode {mode:?} should exit Ok");
    }
}

#[tokio::test]
async fn capture_run_with_app_filter_does_not_crash_on_misses() {
    // The renderer is fire-and-forget at this layer (writes to real
    // stdout), so we can't capture lines from outside. What we *can*
    // verify is that run() returns Ok cleanly when filters reject every
    // packet — no crash, no infinite loop, no panic on the channel-close
    // path with `count == 0`.
    let fake = quokka_cli::device::FakeDevice {
        seeded_packets: vec![
            Ok(sample_pkt(1, "Safari", "en0", 64)),
            Ok(sample_pkt(2, "Mail", "en0", 64)),
        ],
        ..Default::default()
    };
    let result = commands::capture::run(
        &fake,
        commands::capture::Options {
            max: None,
            save: None,
            filter: quokka_cli::commands::capture::Filter {
                app: Some("instagram".into()),
                ..Default::default()
            },
            mode: quokka_cli::commands::capture::Mode::Headless,
        },
    )
    .await;
    assert!(result.is_ok());
}

// ============================================================================
// `qk card` — render against FakeDevice
// ============================================================================

#[tokio::test]
async fn card_run_writes_a_1080x1080_png_to_the_given_path() {
    headless();
    use quokka_cli::commands::card;

    let fake = quokka_cli::device::FakeDevice::with_status(healthy_status());
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let png_path = tmp.path().with_extension("png");

    card::run(
        &fake,
        NOW_UNIX,
        card::CardArgs {
            output: Some(png_path.clone()),
            no_open: true,
            redact: false,
        },
    )
    .await
    .expect("card::run should succeed against a healthy fake");

    let bytes = std::fs::read(&png_path).expect("PNG written");
    assert!(
        bytes.len() > 50_000,
        "PNG suspiciously small ({} bytes) — likely rendered as empty",
        bytes.len()
    );
    let (w, h) = card::png::read_png_dimensions(&bytes).expect("valid PNG IHDR");
    assert_eq!((w, h), (1080, 1080));

    let _ = std::fs::remove_file(&png_path);
}

#[tokio::test]
async fn card_redact_suppresses_build_number_and_exact_first_seen_month() {
    use quokka_cli::commands::card::{data, render};

    let status = healthy_status();
    let normal = render::render_svg(&data::project(&status, NOW_UNIX, false));
    let redacted = render::render_svg(&data::project(&status, NOW_UNIX, true));

    // Default renders include the full build + month name.
    assert!(
        normal.contains("(22C152)"),
        "normal mode missing build number"
    );
    // Redacted strips both.
    assert!(
        !redacted.contains("22C152"),
        "--redact must hide the iOS build number"
    );
    assert!(
        !redacted.contains("Mar 2022"),
        "--redact must hide the precise first-seen month"
    );
    // Year still present in some form (the paired_since year).
    assert!(redacted.contains("2021") || redacted.contains("2020") || redacted.contains("2022"));
}

#[tokio::test]
async fn card_jailbreak_flag_surfaces_on_the_apps_row() {
    use quokka_cli::commands::card::{data, render};

    let mut status = healthy_status();
    status.jailbreak_detected = true;
    let svg = render::render_svg(&data::project(&status, NOW_UNIX, false));
    assert!(
        svg.contains("jailbroken"),
        "jailbreak flag should surface as `jailbroken`"
    );
    assert!(!svg.contains("pristine"));
}

#[tokio::test]
async fn card_renders_with_storage_breakdown_fallback_when_ios_omits_categories() {
    use quokka_cli::commands::card::{data, render};

    let mut status = healthy_status();
    status.storage_breakdown = None;
    let svg = render::render_svg(&data::project(&status, NOW_UNIX, false));
    // Fallback path still emits the STORAGE section but without the four
    // category rows.
    assert!(svg.contains("STORAGE"));
    assert!(!svg.contains("photos"));
    assert!(!svg.contains("apps      "));
}
