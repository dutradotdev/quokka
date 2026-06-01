//! Android backend behind the [`Device`](super::Device) trait, talking to a
//! local `adb` server over the adb protocol via the [`forensic_adb`] crate
//! (async, server mode — it connects to the daemon's TCP port and lets the
//! daemon do device auth, so the tree carries no `rsa`/crypto deps). Continuous
//! `logcat` is the one exception: it still spawns the `adb` binary, because
//! streaming a long-lived shell maps cleanly onto a child process and channel.
//!
//! This mirrors the isolation of the iOS `mod real`: nothing about adb — no
//! `forensic_adb` type, command string, or output shape — leaks past this
//! module. The commands only ever see quokka's own neutral types, and
//! forensic-adb's `DeviceError` is collapsed to [`DeviceError`] at the boundary.
//!
//! Non-rooted Android limits what is cheaply available, so some fields degrade
//! to best-effort the same way the iOS backend degrades unavailable lockdown
//! keys to `None`:
//! - Per-app sizes come from `dumpsys diskstats` (app code + user data), which
//!   the adb `shell` user reads without root on Android 8+. The numbers are a
//!   periodically-recomputed snapshot — accurate enough to rank "what's big to
//!   delete", not byte-exact at the instant of the call.
//! - App display names need the framework's label lookup, so the bundle id is
//!   used as the name.
//!
//! Packet capture is deliberately not implemented (it would need a VPN/root),
//! so `as_capture` keeps the trait default of `None` and `qk capture` reports a
//! clear "iOS only" error.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::process::Stdio;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use forensic_adb::{AndroidStorageInput, DeviceError as AdbError, Host};
use tokio::process::Command;

use super::{
    App, BatchCallback, BatchUpdate, Battery, Device, DeviceError, DeviceInfo, DeviceListing,
    DeviceStatus, LogEntry, LogLevel, MediaFile, Platform, Storage, WalkCallback, WalkProgress,
};

/// AFC-equivalent media roots on Android shared storage. The walk is confined
/// to these so a "select all + delete" can never touch app sandboxes or system
/// paths — the same guardrail the iOS backend gets from its AFC jail.
const ANDROID_MEDIA_ROOTS: &[&str] = &[
    "/sdcard/DCIM",
    "/sdcard/Download",
    "/sdcard/Movies",
    "/sdcard/Music",
    "/sdcard/Pictures",
];

/// `find -printf` format: size, epoch mtime, path — tab-separated, one per line.
const FIND_FORMAT: &str = "%s\t%T@\t%p\n";

/// Default TCP port the local `adb` server listens on.
const ADB_SERVER_PORT: u16 = 5037;

pub(super) struct AndroidDevice {
    serial: String,
}

impl AndroidDevice {
    /// Connect to an `adb`-reachable device. `target_serial` (the global
    /// `--udid`) pins a specific one; otherwise the single online device is
    /// used, and 2+ without a serial is an error.
    pub(super) async fn connect(target_serial: Option<&str>) -> Result<Self> {
        let devices = list_adb_devices().await?;
        let serial = select_serial(devices, target_serial)?;
        Ok(Self { serial })
    }

    /// A fresh forensic-adb device handle for this serial — see
    /// [`connect_adb_device`].
    async fn device(&self) -> Result<forensic_adb::Device> {
        connect_adb_device(self.serial.clone()).await
    }

    /// Run a shell command on the device through the adb server and return its
    /// stdout. forensic-adb's `shell:` transport returns stdout without an exit
    /// status, so a failing command surfaces as empty/partial output rather
    /// than an error — the same shape plain `adb shell` gives, and what the
    /// tolerant parsers downstream already absorb.
    async fn shell(&self, command: &[&str]) -> Result<String> {
        // The whole command line is handed to the device shell, which then
        // word-splits it. Single-quote every argument so values carrying
        // spaces, tabs, or newlines — the `find -printf` format, media paths —
        // survive as one token instead of being re-split into broken args.
        let cmdline = command
            .iter()
            .map(|arg| shell_single_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let device = self.device().await?;
        adb_result(device.execute_host_shell_command(&cmdline).await)
    }

    /// Read a single `getprop` key, `None` when empty or unreadable.
    async fn getprop(&self, key: &str) -> Option<String> {
        let value = self.shell(&["getprop", key]).await.ok()?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    }

    /// Per-package installed size (`app code + user data`) from `dumpsys
    /// diskstats`. Readable by the adb `shell` user without root on Android 8+.
    /// On any failure the map is empty, so sizes degrade to `0` rather than
    /// failing the whole listing — the same best-effort posture the iOS backend
    /// takes for unavailable lockdown keys.
    async fn app_sizes(&self) -> HashMap<String, u64> {
        self.shell(&["dumpsys", "diskstats"])
            .await
            .map(|out| parse_diskstats(&out))
            .unwrap_or_default()
    }

    /// List every file under one media `root`, preferring `find -printf`
    /// (64-bit size + mtime in one call) and degrading to a portable paths-only
    /// `find` (size/mtime `0`) where toybox lacks `-printf`. Returns the files
    /// plus whether the paths-only fallback was used, so the caller can warn
    /// once and skip the doomed `-printf` probe on later roots.
    ///
    /// `printf_known_bad` short-circuits the `-printf` attempt once a previous
    /// root has already proven it unsupported on this device.
    async fn find_media_under(&self, root: &str, printf_known_bad: bool) -> (Vec<MediaFile>, bool) {
        if !printf_known_bad {
            let printf_out = self
                .shell(&["find", root, "-type", "f", "-printf", FIND_FORMAT])
                .await
                .unwrap_or_default();
            let found = parse_find_output(&printf_out);
            if !found.is_empty() {
                return (found, false);
            }
        }
        // Either `-printf` is unsupported, or the root is empty/absent (its
        // `find` error went to the discarded stderr). A plain `find` lists
        // paths portably; if it finds files, `-printf` was the missing piece.
        let plain = self
            .shell(&["find", root, "-type", "f"])
            .await
            .unwrap_or_default();
        let paths: Vec<MediaFile> = plain
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|path| MediaFile {
                path: path.to_string(),
                size_bytes: 0,
                modified_unix: 0,
            })
            .collect();
        let used_fallback = !paths.is_empty();
        (paths, used_fallback)
    }
}

#[async_trait]
impl Device for AndroidDevice {
    async fn status(&self) -> Result<DeviceStatus> {
        // These reads are independent, so overlap them: the welcome screen's
        // wall-clock time is then bounded by the slowest call rather than their
        // sum — the same approach the iOS backend takes in `RealDevice::status`.
        let (battery_out, storage_out, third_party_out, model, os_version, os_build, chip_name) = tokio::join!(
            self.shell(&["dumpsys", "battery"]),
            self.shell(&["df", "/data"]),
            self.shell(&["pm", "list", "packages", "-3"]),
            self.getprop("ro.product.model"),
            self.getprop("ro.build.version.release"),
            self.getprop("ro.build.display.id"),
            self.getprop("ro.board.platform"),
        );
        let battery = parse_battery(&battery_out.unwrap_or_default());
        let storage = storage_out.ok().as_deref().and_then(parse_df);
        let app_count = third_party_out
            .ok()
            .map(|out| parse_pm_packages(&out).len());

        Ok(DeviceStatus {
            name: model.clone(),
            model: model.clone(),
            model_friendly: model,
            os_name: Some("Android".into()),
            os_version,
            os_build,
            storage,
            battery,
            app_count,
            chip_name,
            ..Default::default()
        })
    }

    async fn apps(&self) -> Result<Vec<App>> {
        let (sizes, out) = tokio::join!(
            self.app_sizes(),
            self.shell(&["pm", "list", "packages", "-3"]),
        );
        Ok(parse_pm_packages(&out?)
            .into_iter()
            .map(|id| {
                let size = sizes.get(&id).copied().unwrap_or(0);
                app_from_id(id, false, size)
            })
            .collect())
    }

    async fn all_apps(&self) -> Result<Vec<App>> {
        let (sizes, third_party_out, out) = tokio::join!(
            self.app_sizes(),
            self.shell(&["pm", "list", "packages", "-3"]),
            self.shell(&["pm", "list", "packages"]),
        );
        let third_party: HashSet<String> = parse_pm_packages(&third_party_out.unwrap_or_default())
            .into_iter()
            .collect();
        Ok(parse_pm_packages(&out?)
            .into_iter()
            .map(|id| {
                let is_system = !third_party.contains(&id);
                let size = sizes.get(&id).copied().unwrap_or(0);
                app_from_id(id, is_system, size)
            })
            .collect())
    }

    async fn with_dynamic_sizes(
        &self,
        apps: Vec<App>,
        on_batch: BatchCallback,
    ) -> Result<Vec<App>> {
        // `apps()` already carries the real `dumpsys diskstats` size, so there
        // is no separate enrichment phase on Android the way iOS has a slower
        // dynamic `browse`. Fire one synthetic batch with the input unchanged
        // so callers see the same streaming shape as the iOS backend.
        let total = apps.len();
        on_batch(BatchUpdate {
            apps: apps.clone(),
            done: total,
            total,
        });
        Ok(apps)
    }

    async fn app(&self, bundle_id: &str) -> Result<Option<App>> {
        let list_cmd = ["pm", "list", "packages", bundle_id];
        let (list_out, sizes) = tokio::join!(self.shell(&list_cmd), self.app_sizes());
        let found = parse_pm_packages(&list_out?)
            .into_iter()
            .any(|p| p == bundle_id);
        if !found {
            return Ok(None);
        }
        let size = sizes.get(bundle_id).copied().unwrap_or(0);
        Ok(Some(app_from_id(bundle_id.to_string(), false, size)))
    }

    async fn uninstall_app(&self, bundle_id: &str) -> Result<()> {
        // `adb uninstall` is `pm uninstall`; the shell transport gives no exit
        // status, so confirm success from the output the way forensic-adb's own
        // `pm clear` wrapper does ("Success" / "Failure [reason]").
        let out = self.shell(&["pm", "uninstall", bundle_id]).await?;
        if out.contains("Success") {
            Ok(())
        } else {
            Err(DeviceError::AdbCommandFailed(format!(
                "uninstall {bundle_id} failed: {}",
                out.trim()
            ))
            .into())
        }
    }

    fn media_roots(&self) -> &'static [&'static str] {
        ANDROID_MEDIA_ROOTS
    }

    async fn afc_walk(&self, roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>> {
        // The adb SYNC `list`/`stat` services were considered but rejected:
        // they report file size as `u32`, which wraps for media over 4 GiB —
        // exactly the large videos this walk exists to surface. `find -printf`
        // gives a 64-bit size in one round trip per root, so it stays (over the
        // shell transport). `find_media_under` owns the toybox fallback; here we
        // just orchestrate the roots and report progress.
        let mut files = Vec::new();
        let mut printf_unsupported = false;
        for &root in roots {
            let (found, used_fallback) = self.find_media_under(root, printf_unsupported).await;
            printf_unsupported |= used_fallback;
            files.extend(found);
            let bytes_seen = files.iter().map(|f| f.size_bytes).sum();
            on_progress(WalkProgress {
                files_seen: files.len(),
                bytes_seen,
            });
        }
        if printf_unsupported {
            eprintln!(
                "warning: this Android build's `find` lacks -printf; \
                 media file sizes are unavailable (showing paths only)"
            );
        }
        Ok(files)
    }

    async fn afc_delete(&self, path: &str) -> Result<()> {
        // `shell` single-quotes each argument, so pass the raw path — quoting
        // here too would double-quote it.
        self.shell(&["rm", "-f", path]).await?;
        Ok(())
    }

    async fn info(&self) -> Result<DeviceInfo> {
        // Independent property reads — overlap them like `status` does.
        let (model, device_id, os_version, os_build, hardware_model, cpu_architecture) = tokio::join!(
            self.getprop("ro.product.model"),
            self.getprop("ro.product.device"),
            self.getprop("ro.build.version.release"),
            self.getprop("ro.build.display.id"),
            self.getprop("ro.board.platform"),
            self.getprop("ro.product.cpu.abi"),
        );
        Ok(DeviceInfo {
            name: model
                .clone()
                .unwrap_or_else(|| "Android device".to_string()),
            model_identifier: device_id.unwrap_or_else(|| "android".to_string()),
            model_friendly: model,
            serial: self.serial.clone(),
            udid: self.serial.clone(),
            os_version: os_version.unwrap_or_default(),
            os_build,
            hardware_model,
            cpu_architecture,
            ..Default::default()
        })
    }

    async fn reboot(&self) -> Result<()> {
        // The adb `reboot:` host service (empty target = reboot to system) — the
        // daemon ACKs and returns before the device goes down, so this resolves
        // cleanly rather than racing the dropped connection a `shell reboot`
        // would.
        let device = self.device().await?;
        adb_result(device.execute_host_command("reboot:", false, false).await).map(|_| ())
    }

    async fn shutdown(&self) -> Result<()> {
        // There is no adb power-off service, so this stays a shell `reboot -p`
        // (toybox/OEM dependent — validated on a real device by the e2e-android
        // suite, never in CI).
        self.shell(&["reboot", "-p"]).await?;
        Ok(())
    }

    async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut child = Command::new("adb")
            .args(["-s", &self.serial, "logcat", "-v", "threadtime"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(spawn_error)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture logcat stdout"))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogEntry>>(256);
        tokio::spawn(async move {
            // The task owns the child so `kill_on_drop` tears logcat down once
            // the consumer drops the receiver and the send below starts failing.
            let _child = child;
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(entry) = parse_logcat_line(&line) else {
                    continue;
                };
                if tx.send(Ok(entry)).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

/// Map a process spawn failure to a typed [`DeviceError`].
fn spawn_error(e: std::io::Error) -> DeviceError {
    if e.kind() == std::io::ErrorKind::NotFound {
        DeviceError::AdbNotFound
    } else {
        DeviceError::AdbCommandFailed(e.to_string())
    }
}

/// Map a forensic-adb error to a typed [`DeviceError`], collapsing it to the
/// cases the UI branches on and keeping every `forensic_adb` type sealed inside
/// this module. A `NotFound` I/O error keeps the "adb missing" mapping for
/// safety, and an unknown serial becomes the no-device case.
fn map_adb_err(error: AdbError) -> DeviceError {
    match error {
        AdbError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => DeviceError::AdbNotFound,
        AdbError::UnknownDevice(_) => DeviceError::NoAndroidDevice,
        other => DeviceError::AdbCommandFailed(other.to_string()),
    }
}

/// Collapse a forensic-adb result into quokka's `anyhow::Result`, routing the
/// error through [`map_adb_err`] so no `forensic_adb` type escapes the module.
fn adb_result<T>(result: Result<T, AdbError>) -> Result<T> {
    result.map_err(|e| map_adb_err(e).into())
}

/// Build a forensic-adb device handle for `serial`. The handle holds no live
/// socket — each operation opens its own TCP connection to the adb server — so
/// building one per call is cheap and keeps [`AndroidDevice`] stateless,
/// mirroring how the old code spawned a handle per call.
/// `AndroidStorageInput::Auto` only affects push/pull (unused here).
async fn connect_adb_device(serial: String) -> Result<forensic_adb::Device> {
    adb_result(forensic_adb::Device::new(adb_host(), serial, AndroidStorageInput::Auto).await)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdbState {
    Device,
    Unauthorized,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdbDevice {
    serial: String,
    state: AdbState,
}

/// A [`Host`] pointing at the local adb server. We pin `127.0.0.1` rather than
/// use `Host::default()`'s `"localhost"`: the adb daemon binds IPv4 only, so a
/// `localhost` that resolves to `::1` first would burn a refused connection
/// before falling back. [`ADB_SERVER_PORT`] stays the single source of truth.
fn adb_host() -> Host {
    Host {
        host: Some(Ipv4Addr::LOCALHOST.to_string()),
        port: Some(ADB_SERVER_PORT),
    }
}

/// List every adb-known device with its connection state.
///
/// forensic-adb's typed `Host::devices()` drops everything but online (`device`)
/// entries, which would hide the unauthorized/offline cases quokka reports
/// specifically. So we read the raw `host:devices-l` output and parse the state
/// column ourselves with [`parse_device_line`].
async fn list_adb_devices() -> Result<Vec<AdbDevice>> {
    let raw = adb_result(
        adb_host()
            .execute_host_command("devices-l", true, true)
            .await,
    )?;
    Ok(raw.lines().filter_map(parse_device_line).collect())
}

/// Parse one `adb devices -l` line — `"<serial> <state> [k:v ...]"` — into an
/// [`AdbDevice`], collapsing adb's many connection states into the three quokka
/// acts on: ready, needs-authorization, or otherwise unusable. The `List of
/// devices attached` header and blank lines yield `None`.
fn parse_device_line(line: &str) -> Option<AdbDevice> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("List of devices") {
        return None;
    }
    let mut cols = line.split_whitespace();
    let serial = cols.next()?.to_string();
    let state = match cols.next()? {
        "device" => AdbState::Device,
        "unauthorized" => AdbState::Unauthorized,
        _ => AdbState::Offline,
    };
    Some(AdbDevice { serial, state })
}

/// Pick which serial to target, mirroring the iOS connect semantics.
fn select_serial(devices: Vec<AdbDevice>, target: Option<&str>) -> Result<String> {
    if let Some(serial) = target {
        return match devices.iter().find(|d| d.serial == serial) {
            Some(d) if d.state == AdbState::Device => Ok(serial.to_string()),
            Some(d) if d.state == AdbState::Unauthorized => {
                Err(DeviceError::AndroidUnauthorized(serial.to_string()).into())
            }
            Some(_) => Err(DeviceError::AdbCommandFailed(format!(
                "Android device {serial} is offline"
            ))
            .into()),
            None => Err(DeviceError::NoAndroidDevice.into()),
        };
    }

    let online: Vec<&AdbDevice> = devices
        .iter()
        .filter(|d| d.state == AdbState::Device)
        .collect();
    if online.is_empty() {
        if let Some(unauthorized) = devices.iter().find(|d| d.state == AdbState::Unauthorized) {
            return Err(DeviceError::AndroidUnauthorized(unauthorized.serial.clone()).into());
        }
        return Err(DeviceError::NoAndroidDevice.into());
    }
    if online.len() > 1 {
        return Err(DeviceError::AdbCommandFailed(
            "multiple Android devices connected — pass --udid <serial> to choose one".to_string(),
        )
        .into());
    }
    Ok(online[0].serial.clone())
}

/// List the serials of adb devices that are online (`state == Device`).
///
/// Cheap (no per-device `getprop`) and best-effort: a missing or unreachable
/// adb server yields an empty list, never an error, so cross-platform
/// autodetect can count devices without aborting the command.
pub(super) async fn online_serials() -> Vec<String> {
    list_adb_devices()
        .await
        .map(|devices| {
            devices
                .into_iter()
                .filter(|d| d.state == AdbState::Device)
                .map(|d| d.serial)
                .collect()
        })
        .unwrap_or_default()
}

/// List every adb-known device as a neutral [`DeviceListing`] for the merged
/// `qk devices` output. Online devices are enriched with their model via
/// `getprop`; unauthorized/offline devices list with `None` identity (the
/// renderer shows a placeholder). Best-effort: an unreachable adb server yields
/// an empty list, never an error.
pub(super) async fn list_devices_impl() -> Result<Vec<DeviceListing>> {
    let devices = list_adb_devices().await.unwrap_or_default();

    let listings = futures::future::join_all(devices.into_iter().map(|d| async move {
        let model = match d.state {
            AdbState::Device => read_model(&d.serial).await,
            _ => None,
        };
        DeviceListing {
            platform: Platform::Android,
            // A network/`adb connect` serial carries `host:port`; a USB serial
            // does not. Cheap way to label the transport without a round-trip.
            connection: if d.serial.contains(':') {
                "Wi-Fi"
            } else {
                "USB"
            },
            udid: d.serial,
            // Android exposes no separately set device name without extra
            // round-trips, so the model is the best human label we have here.
            name: model.clone(),
            model_identifier: None,
            model_friendly: model,
        }
    }))
    .await;
    Ok(listings)
}

/// Read `ro.product.model` for one device. `None` on any failure (device went
/// offline mid-scan, empty value) so a single bad device never sinks the whole
/// listing.
async fn read_model(serial: &str) -> Option<String> {
    let device = connect_adb_device(serial.to_string()).await.ok()?;
    let value = device
        .execute_host_shell_command("getprop ro.product.model")
        .await
        .ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Build an [`App`] from a bundle id and its `dumpsys diskstats` size. The name
/// is the id — the missing label is a non-root limitation documented at the
/// module level.
///
// TODO(lucasdutra): resolve the human label ("Spotify") instead of the id.
// Deliberately deferred — the only non-root path is pulling each base.apk
// (GBs of transfer) and resolving `@string/app_name` through a pre-1.0
// AXML/arsc parser, which is poor value for cosmetic polish. — 2026-05-30
fn app_from_id(bundle_id: String, is_system: bool, size_bytes: u64) -> App {
    App {
        name: bundle_id.clone(),
        bundle_id,
        size_bytes,
        is_system,
        install_date_unix: None,
    }
}

/// Parse `dumpsys battery` into a [`Battery`]. Cycle count and health are not
/// exposed without root, so they stay `None`.
fn parse_battery(output: &str) -> Battery {
    let mut battery = Battery::default();
    for line in output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("level:") {
            battery.level_percent = v.trim().parse::<u8>().ok();
        } else if let Some(v) = line.strip_prefix("temperature:") {
            // dumpsys reports tenths of a degree Celsius.
            battery.temperature_celsius = v.trim().parse::<f32>().ok().map(|t| t / 10.0);
        } else if let Some(v) = line.strip_prefix("status:") {
            // BatteryManager: 2 = charging, 5 = full.
            battery.is_charging = v.trim().parse::<u8>().ok().map(|s| s == 2 || s == 5);
        }
    }
    battery
}

/// Parse `df /data` into a [`Storage`]. Columns are `1K-blocks`, so values are
/// scaled to bytes.
fn parse_df(output: &str) -> Option<Storage> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Filesystem") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let blocks = cols[1].parse::<u64>().ok()?;
        let available = cols[3].parse::<u64>().ok()?;
        return Some(Storage {
            total_bytes: blocks * 1024,
            free_bytes: available * 1024,
            ..Default::default()
        });
    }
    None
}

/// Parse `dumpsys diskstats` parallel arrays into a `package -> bytes` map.
///
/// The dump prints index-aligned arrays:
/// ```text
/// Package Names: [com.spotify.music, com.whatsapp]
/// App Sizes: [180000000, 95000000]
/// App Data Sizes: [1200000000, 800000000]
/// Cache Sizes: [50000000, 30000000]
/// ```
/// Size per package = `App Size + App Data Size` (this mirrors iOS Settings →
/// iPhone Storage = app + documents & data). Cache is ephemeral and excluded.
///
/// Defensive by design: arrays of differing length are matched up to the
/// shortest, a missing size cell counts as `0`, and absent labels yield an
/// empty map. Malformed input never panics — sizes just degrade toward `0`.
fn parse_diskstats(output: &str) -> HashMap<String, u64> {
    let names = diskstats_array(output, "Package Names:");
    let app_sizes = diskstats_numbers(output, "App Sizes:");
    let data_sizes = diskstats_numbers(output, "App Data Sizes:");

    names
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
        .map(|(index, name)| {
            let app = app_sizes.get(index).copied().unwrap_or(0);
            let data = data_sizes.get(index).copied().unwrap_or(0);
            (name.to_string(), app.saturating_add(data))
        })
        .collect()
}

/// Extract a `Label: [a, b, c]` array as trimmed elements. Empty vec when the
/// label is absent or the array is empty.
fn diskstats_array<'a>(output: &'a str, label: &str) -> Vec<&'a str> {
    let Some(line) = output.lines().map(str::trim).find(|l| l.starts_with(label)) else {
        return Vec::new();
    };
    let body = line[label.len()..].trim();
    let body = body.strip_prefix('[').unwrap_or(body);
    let body = body.strip_suffix(']').unwrap_or(body);
    if body.trim().is_empty() {
        return Vec::new();
    }
    // MIUI (and other OEMs) quote each package name — `["com.foo","com.bar"]` —
    // while AOSP prints them bare. Strip surrounding quotes so both shapes
    // parse; the numeric arrays carry no quotes, so this is a no-op for them.
    body.split(',')
        .map(|cell| cell.trim().trim_matches('"'))
        .collect()
}

/// Same as [`diskstats_array`] but parses each element as `u64`, treating an
/// unparseable cell as `0` so index alignment with the package list survives.
fn diskstats_numbers(output: &str, label: &str) -> Vec<u64> {
    diskstats_array(output, label)
        .iter()
        .map(|cell| cell.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Parse `pm list packages` output into bundle ids (the `package:` prefix is
/// stripped).
fn parse_pm_packages(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("package:")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// Parse `find -printf "%s\t%T@\t%p\n"` output into [`MediaFile`]s.
fn parse_find_output(output: &str) -> Vec<MediaFile> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let size_bytes = parts.next()?.trim().parse::<u64>().ok()?;
            let modified_unix = parts
                .next()?
                .split('.')
                .next()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let path = parts.next()?.to_string();
            Some(MediaFile {
                path,
                size_bytes,
                modified_unix,
            })
        })
        .collect()
}

/// Parse one `logcat -v threadtime` line into a [`LogEntry`]. Continuation and
/// banner lines (`--------- beginning of ...`) return `None`.
fn parse_logcat_line(line: &str) -> Option<LogEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("---------") {
        return None;
    }
    let mut tokens = line.split_whitespace();
    let _date = tokens.next()?;
    let time = tokens.next()?; // HH:MM:SS.mmm
    let pid = tokens.next()?.parse::<u32>().ok();
    let _tid = tokens.next()?;
    let level = match tokens.next()? {
        "V" | "D" => LogLevel::Debug,
        "I" => LogLevel::Info,
        "W" => LogLevel::Warning,
        "E" => LogLevel::Error,
        "F" => LogLevel::Fault,
        _ => LogLevel::Unknown,
    };
    let rest = tokens.collect::<Vec<_>>().join(" ");
    let (process, message) = match rest.split_once(':') {
        Some((tag, msg)) => (tag.trim().to_string(), msg.trim().to_string()),
        None => (String::new(), rest),
    };
    Some(LogEntry {
        timestamp_unix_ms: None,
        time_text: time.split('.').next().map(str::to_string),
        host: String::new(),
        process,
        pid,
        level,
        message,
    })
}

/// Single-quote a string for safe interpolation into an `adb shell` command,
/// so paths with spaces survive the device-side shell split.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_device_line_maps_states_and_skips_header() {
        // The `-l` long format carries trailing `key:value` pairs after the
        // state; only the serial and state columns matter here.
        let ready =
            parse_device_line("ABC123  device product:sdk model:Pixel_7 device:panther").unwrap();
        assert_eq!(ready.serial, "ABC123");
        assert_eq!(ready.state, AdbState::Device);

        let unauthorized = parse_device_line("DEF456  unauthorized").unwrap();
        assert_eq!(unauthorized.serial, "DEF456");
        assert_eq!(unauthorized.state, AdbState::Unauthorized);

        // Every other adb state collapses to Offline (unusable).
        assert_eq!(
            parse_device_line("GHI789  recovery").unwrap().state,
            AdbState::Offline
        );

        // A network serial keeps its `host:port` shape intact.
        assert_eq!(
            parse_device_line("192.168.1.5:5555  device")
                .unwrap()
                .serial,
            "192.168.1.5:5555"
        );

        // The header line and blank lines are not devices.
        assert!(parse_device_line("List of devices attached").is_none());
        assert!(parse_device_line("").is_none());
    }

    #[test]
    fn select_serial_picks_single_online_device() {
        let devices = vec![AdbDevice {
            serial: "ABC123".into(),
            state: AdbState::Device,
        }];
        assert_eq!(select_serial(devices, None).unwrap(), "ABC123");
    }

    #[test]
    fn select_serial_errors_on_unauthorized() {
        let devices = vec![AdbDevice {
            serial: "ABC123".into(),
            state: AdbState::Unauthorized,
        }];
        let err = select_serial(devices, None).unwrap_err();
        assert!(err.to_string().contains("unauthorized"));
    }

    #[test]
    fn select_serial_errors_on_multiple_without_target() {
        let devices = vec![
            AdbDevice {
                serial: "A".into(),
                state: AdbState::Device,
            },
            AdbDevice {
                serial: "B".into(),
                state: AdbState::Device,
            },
        ];
        let err = select_serial(devices, None).unwrap_err();
        assert!(err.to_string().contains("multiple Android devices"));
    }

    #[test]
    fn select_serial_honors_target() {
        let devices = vec![
            AdbDevice {
                serial: "A".into(),
                state: AdbState::Device,
            },
            AdbDevice {
                serial: "B".into(),
                state: AdbState::Device,
            },
        ];
        assert_eq!(select_serial(devices, Some("B")).unwrap(), "B");
    }

    #[test]
    fn parse_battery_reads_level_temp_and_charging() {
        let out = "Current Battery Service state:\n  \
                   level: 87\n  scale: 100\n  temperature: 274\n  status: 2\n";
        let b = parse_battery(out);
        assert_eq!(b.level_percent, Some(87));
        assert_eq!(b.temperature_celsius, Some(27.4));
        assert_eq!(b.is_charging, Some(true));
        assert!(b.cycle_count.is_none());
    }

    #[test]
    fn parse_df_scales_blocks_to_bytes() {
        let out = "Filesystem     1K-blocks    Used Available Use% Mounted on\n\
                   /dev/block/dm-5 100000  40000  60000  40% /data\n";
        let s = parse_df(out).unwrap();
        assert_eq!(s.total_bytes, 100_000 * 1024);
        assert_eq!(s.free_bytes, 60_000 * 1024);
        assert_eq!(s.used_bytes(), 40_000 * 1024);
    }

    #[test]
    fn parse_pm_packages_strips_prefix() {
        let out = "package:com.android.chrome\npackage:com.spotify.music\n";
        assert_eq!(
            parse_pm_packages(out),
            vec!["com.android.chrome", "com.spotify.music"]
        );
    }

    #[test]
    fn parse_find_output_reads_size_mtime_path() {
        let out = "4210000\t1700000000.0000000000\t/sdcard/DCIM/IMG_1.MOV\n\
                   52000\t1699999999\t/sdcard/Download/manual.pdf\n";
        let files = parse_find_output(out);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].size_bytes, 4_210_000);
        assert_eq!(files[0].modified_unix, 1_700_000_000);
        assert_eq!(files[0].path, "/sdcard/DCIM/IMG_1.MOV");
        assert_eq!(files[1].path, "/sdcard/Download/manual.pdf");
    }

    #[test]
    fn parse_logcat_line_threadtime() {
        let line = "05-30 12:34:56.789  1234  1250 W ActivityManager: low memory";
        let entry = parse_logcat_line(line).unwrap();
        assert_eq!(entry.time_text.as_deref(), Some("12:34:56"));
        assert_eq!(entry.pid, Some(1234));
        assert_eq!(entry.level, LogLevel::Warning);
        assert_eq!(entry.process, "ActivityManager");
        assert_eq!(entry.message, "low memory");
    }

    #[test]
    fn parse_logcat_line_skips_banner() {
        assert!(parse_logcat_line("--------- beginning of main").is_none());
        assert!(parse_logcat_line("").is_none());
    }

    #[test]
    fn shell_single_quote_escapes_quotes_and_spaces() {
        assert_eq!(
            shell_single_quote("/sdcard/My File.jpg"),
            "'/sdcard/My File.jpg'"
        );
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn app_from_id_uses_id_as_name_and_given_size() {
        let app = app_from_id("com.example.app".into(), true, 1_380_000_000);
        assert_eq!(app.bundle_id, "com.example.app");
        assert_eq!(app.name, "com.example.app");
        assert_eq!(app.size_bytes, 1_380_000_000);
        assert!(app.is_system);
        assert!(app.install_date_unix.is_none());
    }

    #[test]
    fn parse_diskstats_matches_arrays_by_index() {
        let out = "Package Names: [com.spotify.music, com.whatsapp]\n\
                   App Sizes: [180000000, 95000000]\n\
                   App Data Sizes: [1200000000, 800000000]\n\
                   Cache Sizes: [50000000, 30000000]\n";
        let sizes = parse_diskstats(out);
        // app + data; cache is deliberately excluded.
        assert_eq!(sizes["com.spotify.music"], 1_380_000_000);
        assert_eq!(sizes["com.whatsapp"], 895_000_000);
    }

    #[test]
    fn parse_diskstats_tolerates_missing_or_ragged_arrays() {
        // `App Data Sizes` is shorter than the package list and `Cache Sizes`
        // is absent entirely. Missing cells count as zero; nothing panics.
        let out = "Package Names: [com.a, com.b, com.c]\n\
                   App Sizes: [100, 200, 300]\n\
                   App Data Sizes: [10]\n";
        let sizes = parse_diskstats(out);
        assert_eq!(sizes["com.a"], 110);
        assert_eq!(sizes["com.b"], 200);
        assert_eq!(sizes["com.c"], 300);
        assert_eq!(sizes.len(), 3);

        // Empty input is an empty map, not a panic.
        assert!(parse_diskstats("").is_empty());
    }

    #[test]
    fn parse_diskstats_handles_quoted_miui_arrays() {
        // Real Redmi/MIUI shape: package names are double-quoted with no space
        // after commas, and the dump also carries singular `App Size:` /
        // `App Data Size:` totals that must not be mistaken for the per-app
        // `App Sizes:` arrays. One OEM-agnostic parser handles this and the
        // bare AOSP shape above.
        let out = "App Size: 9578543104\n\
                   App Data Size: 5711364013\n\
                   Package Names: [\"com.whatsapp.w4b\",\"com.miui.gallery\"]\n\
                   App Sizes: [193439232,182879744]\n\
                   App Data Sizes: [139890688,200941568]\n\
                   Cache Sizes: [33411072,802816]\n";
        let sizes = parse_diskstats(out);
        assert_eq!(sizes["com.whatsapp.w4b"], 193_439_232 + 139_890_688);
        assert_eq!(sizes["com.miui.gallery"], 182_879_744 + 200_941_568);
    }

    // --- Property tests: tolerant parsers never panic on OEM variation. ---
    // The project's rule is "tolerant parsing, not per-OEM branching"; these
    // fuzz that invariant instead of enumerating manufacturers by hand.
    proptest::proptest! {
        #[test]
        fn parsers_never_panic_on_arbitrary_input(s in "(?s).{0,400}") {
            let _ = parse_battery(&s);
            let _ = parse_df(&s);
            let _ = parse_diskstats(&s);
            let _ = parse_pm_packages(&s);
            let _ = parse_find_output(&s);
            let _ = parse_device_line(&s);
            let _ = parse_logcat_line(&s);
        }

        /// `parse_diskstats` matches three parallel arrays by index. OEM dumps
        /// routinely ship arrays of mismatched length; that must never panic.
        #[test]
        fn parse_diskstats_survives_ragged_arrays(
            names in proptest::collection::vec("[a-z][a-z.]{0,20}", 0..12),
            sizes in proptest::collection::vec(0u64..1_000_000_000, 0..12),
            data_sizes in proptest::collection::vec(0u64..1_000_000_000, 0..12),
        ) {
            let join = |v: &[u64]| {
                v.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
            };
            let quoted = names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(",");
            let out = format!(
                "Package Names: [{quoted}]\nApp Sizes: [{}]\nApp Data Sizes: [{}]\n",
                join(&sizes),
                join(&data_sizes),
            );
            let result = parse_diskstats(&out);
            // Every reported entry must correspond to a real package name —
            // a ragged array must never invent or mis-key a package.
            for key in result.keys() {
                proptest::prop_assert!(names.iter().any(|n| n == key));
            }
        }
    }
}
