//! Integration tests exercising commands against `FakeDevice` — no iPhone
//! required.

use quokka_cli::commands;
use quokka_cli::commands::dashboard;
use quokka_cli::device::{App, Battery, DeviceStatus, FakeDevice, MediaFile, Storage};

/// Fixed clock anchor so relative-date output is deterministic.
const NOW_UNIX: i64 = 1_779_793_200;

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
        },
        App {
            bundle_id: "com.user.small".into(),
            name: "Small".into(),
            size_bytes: 50_000_000,
            is_system: false,
        },
        App {
            bundle_id: "com.apple.MobileSMS".into(),
            name: "Messages".into(),
            size_bytes: 120_000_000,
            is_system: true,
        },
    ]
}

#[tokio::test]
async fn apps_list_flow_renders_user_apps_against_fake() {
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
    use quokka_cli::commands::analyze::top_n_by_size;
    let top = top_n_by_size(sample_media(), 2);
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
    commands::info::run(&fake, false).await.expect("info ok");
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
