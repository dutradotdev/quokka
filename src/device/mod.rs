//! Boundary with the `idevice` crate.
//!
//! Everything quokka does to an iPhone goes through the [`Device`] trait. The
//! real implementation talks to `idevice`; tests provide their own fake. No
//! `idevice` type may be exposed from this module — the seam exists so the
//! rest of the codebase does not depend on `idevice`'s still-evolving API
//! (the crate ships breaking changes at every point release until 0.2.0).

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

pub mod model_names;

/// Streaming update from [`Device::with_dynamic_sizes`] — one fires per
/// batch as the device returns enriched sizes. `apps` carries only the
/// bundle ids that finished in this batch; `done`/`total` are cumulative.
pub struct BatchUpdate {
    pub apps: Vec<App>,
    pub done: usize,
    pub total: usize,
}

/// Per-batch callback for [`Device::with_dynamic_sizes`].
pub type BatchCallback = Box<dyn Fn(BatchUpdate) + Send + Sync>;

/// One file discovered during an AFC walk.
#[derive(Debug, Clone)]
pub struct MediaFile {
    /// Absolute path under the AFC root (e.g. `/DCIM/100APPLE/IMG_4521.MOV`).
    pub path: String,
    pub size_bytes: u64,
    /// Modification time as Unix epoch seconds. Used for age-based heuristics.
    pub modified_unix: i64,
}

/// Cumulative progress reported during [`Device::afc_walk`].
#[derive(Debug, Clone, Copy)]
pub struct WalkProgress {
    pub files_seen: usize,
    pub bytes_seen: u64,
}

/// Periodic progress callback for [`Device::afc_walk`].
pub type WalkCallback = Box<dyn Fn(WalkProgress) + Send + Sync>;

/// Static identity snapshot used by [`Device::info`]. Optional fields degrade
/// silently to `None` on a per-key read failure — only the required fields
/// (`name`, `model_identifier`, `serial`, `udid`, `ios_version`) abort.
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub model_identifier: String,
    pub model_friendly: Option<String>,
    pub model_number: Option<String>,
    pub region_info: Option<String>,
    pub enclosure_color: Option<String>,
    pub serial: String,
    pub udid: String,

    pub ios_version: String,
    pub ios_build: Option<String>,
    pub hardware_model: Option<String>,
    pub cpu_architecture: Option<String>,
    pub activation_state: Option<String>,
    pub is_supervised: Option<bool>,
    pub developer_mode_enabled: Option<bool>,

    pub wifi_address: Option<String>,
    pub bluetooth_address: Option<String>,
    pub imei: Option<String>,
    pub imei2: Option<String>,
}

/// One log line from `com.apple.syslog_relay`, parsed to structured form.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp_unix_ms: Option<i64>,
    /// `HH:MM:SS` extracted verbatim from the syslog frame's BSD-style
    /// timestamp prefix. Cheap to render in narrow columns without parsing
    /// the date back into epoch ms.
    pub time_text: Option<String>,
    pub host: String,
    pub process: String,
    pub pid: Option<u32>,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Fault,
    Unknown,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Notice => "Notice",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
            LogLevel::Fault => "Fault",
            LogLevel::Unknown => "Unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "notice" => LogLevel::Notice,
            "warning" | "warn" => LogLevel::Warning,
            "error" | "err" => LogLevel::Error,
            "fault" | "critical" | "emergency" | "alert" => LogLevel::Fault,
            _ => LogLevel::Unknown,
        }
    }
}

/// Power-state action recorded by [`FakeDevice`] for test inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerCall {
    Reboot,
    Shutdown,
}

#[async_trait]
pub trait Device: Send + Sync {
    /// Full status snapshot: identity, OS, storage, battery. Individual
    /// fields are `Option` because the device exposes them inconsistently
    /// across iOS versions — missing fields render as `—` rather than fail.
    async fn status(&self) -> Result<DeviceStatus>;

    /// All apps known to `installation_proxy`, system and user, with
    /// **bundle sizes only** (`StaticDiskUsage`). Fast — one round-trip.
    /// To include cache/downloads, follow up with
    /// [`with_dynamic_sizes`](Self::with_dynamic_sizes).
    async fn apps(&self) -> Result<Vec<App>>;

    /// Enrich an already-fetched list with `DynamicDiskUsage` so each
    /// `App::size_bytes` matches what iOS Settings → iPhone Storage shows.
    /// Internally runs bounded-concurrency batched `browse`s; on iOS 26+ a
    /// single bulk `browse` with Dynamic enabled hangs for minutes, but
    /// `BundleIDs`-scoped batches stay snappy. Progress reports apps done.
    async fn with_dynamic_sizes(&self, apps: Vec<App>, on_batch: BatchCallback)
        -> Result<Vec<App>>;

    /// Single app lookup with full size (static + dynamic). Cheap because
    /// the device only walks one container. Returns `Ok(None)` if the
    /// bundle id is not installed.
    async fn app(&self, bundle_id: &str) -> Result<Option<App>>;

    /// Uninstall the app with the given bundle id. The device returns
    /// success even for unknown bundle ids on some iOS versions — callers
    /// that need that distinction should `app()` first.
    async fn uninstall_app(&self, bundle_id: &str) -> Result<()>;

    /// Recursively list every file under each of `roots` via AFC. `roots`
    /// are AFC-relative paths (e.g. `"/DCIM"`). `on_progress` fires
    /// periodically with cumulative counts so the UI can show live updates.
    /// Per-subdir permission errors are logged and skipped; the whole walk
    /// only fails if AFC itself cannot be reached.
    async fn afc_walk(&self, roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>>;

    /// Delete a single file via AFC.
    async fn afc_delete(&self, path: &str) -> Result<()>;

    /// Identity snapshot — lockdown-classic reads of ~15 keys. Optional
    /// fields degrade silently to `None` per-key.
    async fn info(&self) -> Result<DeviceInfo>;

    /// Request a soft reboot via `diagnostics_relay`. Returns once the
    /// service acknowledges the request — the actual restart happens
    /// asynchronously on the device.
    async fn reboot(&self) -> Result<()>;

    /// Request a power-off via `diagnostics_relay`. Returns once the
    /// service acknowledges the request.
    async fn shutdown(&self) -> Result<()>;

    /// Open a streaming syslog session. The returned `mpsc::Receiver` yields
    /// one `Result<LogEntry>` per parsed line. The session ends when the
    /// receiver is dropped or the device disconnects.
    async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>>;
}

#[derive(Debug, Clone)]
pub struct App {
    pub bundle_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub is_system: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceStatus {
    pub name: Option<String>,
    pub model: Option<String>,
    /// Marketing name resolved from `model` via [`model_names::friendly_name`].
    pub model_friendly: Option<String>,
    pub ios_version: Option<String>,
    /// Lockdown `BuildVersion`, e.g. `22C152`.
    pub ios_build: Option<String>,
    /// Raw enclosure colour string from lockdown. Mapping to a terminal
    /// colour is the renderer's job — values may be human-readable names
    /// (e.g. `"Sierra Blue"`) or opaque on modern iOS.
    pub enclosure_color: Option<String>,
    pub storage: Option<Storage>,
    // Not `Option<Battery>` because battery is a bundle of independent
    // signals: any of cycles/level/health/temp can be present while others
    // are missing. `Battery::default()` represents "diagnostics unreachable",
    // which is semantically the same as "all fields None".
    pub battery: Battery,
    /// BCP-47-shaped locale (`pt-BR`), normalized from the lockdown
    /// `Locale` key (which often comes back as `pt_BR`).
    pub locale: Option<String>,
    /// IANA time zone (e.g. `America/Sao_Paulo`).
    pub time_zone: Option<String>,
    /// Number of installed user apps. Comes from a fast
    /// `installation_proxy.browse` and stays `None` if that call fails so
    /// the dashboard can still render the rest.
    pub app_count: Option<usize>,
    /// Whether Developer Mode is enabled. Lockdown key is known to return
    /// `MobileGestaltDeprecated` on some iOS versions — `None` is the
    /// common case rather than an error.
    pub developer_mode: Option<bool>,
    /// Whether Find My iPhone is enabled. `None` when the lockdown source
    /// is not reachable on this iOS major.
    pub find_my: Option<bool>,
    /// Unix seconds of the device's last completed backup (lockdown
    /// `com.apple.mobile.iTunes` domain).
    pub last_backup_unix: Option<i64>,
    /// Unix seconds the local pairing record was created — i.e. the first
    /// time this Mac was trusted. Read from the on-disk plist, not the
    /// device itself.
    pub paired_since_unix: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Storage {
    pub total_bytes: u64,
    pub free_bytes: u64,
    /// System (OS) capacity from lockdown `TotalSystemCapacity`. When `Some`
    /// together with `data_used_bytes`, the dashboard shows a three-line
    /// breakdown; otherwise it falls back to the single bar row.
    pub system_bytes: Option<u64>,
    /// Bytes used in the data partition (derived from `TotalDataCapacity -
    /// AmountDataAvailable`).
    pub data_used_bytes: Option<u64>,
}

impl Storage {
    pub fn used_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    /// Used capacity as a 0..=100 percentage, rounded to nearest integer.
    pub fn used_percent(self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        let pct = (self.used_bytes() as f64 / self.total_bytes as f64) * 100.0;
        pct.round().clamp(0.0, 100.0) as u8
    }
}

#[derive(Debug, Clone, Default)]
pub struct Battery {
    pub level_percent: Option<u8>,
    pub cycle_count: Option<u32>,
    pub health_percent: Option<u8>,
    pub temperature_celsius: Option<f32>,
    /// Whether the device is currently charging. From the lockdown
    /// `com.apple.mobile.battery` domain (`ExternalConnected` /
    /// `BatteryIsCharging`).
    pub is_charging: Option<bool>,
    /// Adapter wattage, only known while charging. Read from
    /// `AdapterDetails.Watts` in the lockdown battery domain.
    pub adapter_watts: Option<u32>,
    /// Adapter description string from `AdapterDetails.Description`
    /// (e.g. "USB-C", "USB Power Adapter").
    pub adapter_description: Option<String>,
}

/// Connect to an iPhone reachable over usbmuxd. With `udid = Some(_)`
/// targets that exact device. With `udid = None`, picks the only device
/// connected, or — when 2+ are present — opens an interactive picker on a
/// TTY (errors with a hint on non-TTY).
pub async fn connect(udid: Option<&str>) -> Result<Box<dyn Device>> {
    Ok(Box::new(real::RealDevice::connect(udid).await?))
}

/// One row returned by [`list_devices`].
#[derive(Debug, Clone)]
pub struct DeviceListing {
    pub udid: String,
    pub connection: &'static str,
    pub name: Option<String>,
    pub model_identifier: Option<String>,
    pub model_friendly: Option<String>,
}

/// List every iPhone reachable through usbmuxd. Best-effort: identity
/// fields are `None` if the device is not paired/trusted yet.
pub async fn list_devices() -> Result<Vec<DeviceListing>> {
    real::list_devices_impl().await
}

/// In-memory [`Device`] for tests. Public so integration tests in `tests/`
/// can construct it — production code never does.
#[derive(Debug)]
pub struct FakeDevice {
    // String errors (not `anyhow::Error`) because `anyhow::Error` isn't Clone
    // and `&self` methods need to hand the value back without consuming it.
    pub status: Result<DeviceStatus, String>,
    pub apps: Result<Vec<App>, String>,
    pub uninstall_result: Result<(), String>,
    /// Bundle ids that have been passed to [`Device::uninstall_app`]. Tests
    /// inspect this to verify the destructive path was reached.
    pub uninstalled: std::sync::Mutex<Vec<String>>,
    /// Files returned from [`Device::afc_walk`].
    pub media: Vec<MediaFile>,
    /// Paths that have been passed to [`Device::afc_delete`]. Tests inspect
    /// this to verify the destructive path was reached.
    pub deleted: std::sync::Mutex<Vec<String>>,
    pub delete_result: Result<(), String>,
    /// Seeded device-info snapshot returned from [`Device::info`].
    pub info: DeviceInfo,
    /// Recorded power requests (reboot / shutdown).
    pub power_calls: std::sync::Mutex<Vec<PowerCall>>,
    /// When `true`, both [`Device::reboot`] and [`Device::shutdown`] return an error.
    pub fail_power: bool,
    /// Seeded syslog entries — `stream_logs` replays them one-per-tick.
    pub seeded_logs: Vec<Result<LogEntry, String>>,
}

impl FakeDevice {
    pub fn with_status(status: DeviceStatus) -> Self {
        Self {
            status: Ok(status),
            ..Self::default()
        }
    }

    pub fn with_status_error(message: impl Into<String>) -> Self {
        Self {
            status: Err(message.into()),
            ..Self::default()
        }
    }

    /// Snapshot of bundle ids uninstalled so far. Convenience for tests.
    pub fn uninstalled(&self) -> Vec<String> {
        self.uninstalled.lock().unwrap().clone()
    }

    /// Snapshot of paths deleted via [`Device::afc_delete`].
    pub fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }
}

impl Default for FakeDevice {
    /// Plausible, healthy device. Mutate fields with struct-update syntax
    /// (`FakeDevice { apps: ..., ..Default::default() }`) or use one of the
    /// convenience constructors.
    fn default() -> Self {
        Self {
            status: Ok(DeviceStatus {
                name: Some("Test iPhone".into()),
                model: Some("iPhone15,3".into()),
                model_friendly: Some("iPhone 14 Pro Max".into()),
                ios_version: Some("18.2".into()),
                ios_build: Some("22C152".into()),
                enclosure_color: Some("Deep Purple".into()),
                storage: Some(Storage {
                    total_bytes: 256_000_000_000,
                    free_bytes: 148_500_000_000,
                    system_bytes: Some(12_400_000_000),
                    data_used_bytes: Some(95_100_000_000),
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
            }),
            apps: Ok(vec![
                App {
                    bundle_id: "com.example.heavy".into(),
                    name: "Heavy User App".into(),
                    size_bytes: 800_000_000,
                    is_system: false,
                },
                App {
                    bundle_id: "com.example.medium".into(),
                    name: "Medium User App".into(),
                    size_bytes: 250_000_000,
                    is_system: false,
                },
                App {
                    bundle_id: "com.apple.MobileSMS".into(),
                    name: "Messages".into(),
                    size_bytes: 120_000_000,
                    is_system: true,
                },
            ]),
            uninstall_result: Ok(()),
            uninstalled: std::sync::Mutex::new(Vec::new()),
            media: vec![
                MediaFile {
                    path: "/DCIM/103APPLE/IMG_4521.MOV".into(),
                    size_bytes: 4_210_000_000,
                    modified_unix: 1_700_000_000,
                },
                MediaFile {
                    path: "/DCIM/103APPLE/IMG_4519.MOV".into(),
                    size_bytes: 2_880_000_000,
                    modified_unix: 1_700_000_000,
                },
                MediaFile {
                    path: "/DCIM/100APPLE/IMG_0123.HEIC".into(),
                    size_bytes: 4_200_000,
                    modified_unix: 1_700_000_000,
                },
                MediaFile {
                    path: "/Downloads/manual.pdf".into(),
                    size_bytes: 52_000_000,
                    modified_unix: 1_700_000_000,
                },
                MediaFile {
                    path: "/Recordings/Memo 01.m4a".into(),
                    size_bytes: 1_500_000,
                    modified_unix: 1_700_000_000,
                },
            ],
            deleted: std::sync::Mutex::new(Vec::new()),
            delete_result: Ok(()),
            info: DeviceInfo {
                name: "Lucas's iPhone".into(),
                model_identifier: "iPhone16,2".into(),
                model_friendly: Some("iPhone 15 Pro Max".into()),
                model_number: Some("MQ8X3LL/A".into()),
                region_info: Some("LL/A".into()),
                enclosure_color: Some("Natural Titanium".into()),
                serial: "F2LXXXXXXXXX".into(),
                udid: "00008130-001A2B3C4D5E6F7G".into(),
                ios_version: "18.2".into(),
                ios_build: Some("22C152".into()),
                hardware_model: Some("D74AP".into()),
                cpu_architecture: Some("arm64e".into()),
                activation_state: Some("Activated".into()),
                is_supervised: Some(false),
                developer_mode_enabled: Some(false),
                wifi_address: Some("AA:BB:CC:DD:EE:FF".into()),
                bluetooth_address: Some("AA:BB:CC:DD:EE:F0".into()),
                imei: Some("350123456789012".into()),
                imei2: Some("350123456789013".into()),
            },
            power_calls: std::sync::Mutex::new(Vec::new()),
            fail_power: false,
            seeded_logs: Vec::new(),
        }
    }
}

#[async_trait]
impl Device for FakeDevice {
    async fn status(&self) -> Result<DeviceStatus> {
        self.status.clone().map_err(|e| anyhow!(e))
    }

    async fn apps(&self) -> Result<Vec<App>> {
        self.apps.clone().map_err(|e| anyhow!(e))
    }

    async fn with_dynamic_sizes(
        &self,
        apps: Vec<App>,
        on_batch: BatchCallback,
    ) -> Result<Vec<App>> {
        // No real device, no dynamic-vs-static distinction — fake apps
        // already carry whatever size the test set. Fire one synthetic
        // batch covering everything, for symmetry with the real impl.
        let total = apps.len();
        on_batch(BatchUpdate {
            apps: apps.clone(),
            done: total,
            total,
        });
        Ok(apps)
    }

    async fn app(&self, bundle_id: &str) -> Result<Option<App>> {
        let apps = self.apps.clone().map_err(|e| anyhow!(e))?;
        Ok(apps.into_iter().find(|a| a.bundle_id == bundle_id))
    }

    async fn uninstall_app(&self, bundle_id: &str) -> Result<()> {
        self.uninstalled.lock().unwrap().push(bundle_id.to_string());
        self.uninstall_result
            .clone()
            .map_err(|e| anyhow!("{bundle_id}: {e}"))
    }

    async fn afc_walk(&self, _roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>> {
        let files = self.media.clone();
        let bytes_seen = files.iter().map(|f| f.size_bytes).sum();
        on_progress(WalkProgress {
            files_seen: files.len(),
            bytes_seen,
        });
        Ok(files)
    }

    async fn afc_delete(&self, path: &str) -> Result<()> {
        self.deleted.lock().unwrap().push(path.to_string());
        self.delete_result
            .clone()
            .map_err(|e| anyhow!("{path}: {e}"))
    }

    async fn info(&self) -> Result<DeviceInfo> {
        Ok(self.info.clone())
    }

    async fn reboot(&self) -> Result<()> {
        self.power_calls.lock().unwrap().push(PowerCall::Reboot);
        if self.fail_power {
            return Err(anyhow!("simulated diagnostics_relay failure"));
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.power_calls.lock().unwrap().push(PowerCall::Shutdown);
        if self.fail_power {
            return Err(anyhow!("simulated diagnostics_relay failure"));
        }
        Ok(())
    }

    async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogEntry>>(64);
        let entries = self.seeded_logs.clone();
        tokio::spawn(async move {
            for entry in entries {
                let result = entry.map_err(|e| anyhow!(e));
                if tx.send(result).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

impl FakeDevice {
    /// Convenience snapshot of recorded power calls.
    pub fn power_calls(&self) -> Vec<PowerCall> {
        self.power_calls.lock().unwrap().clone()
    }
}

mod real {
    use super::*;
    use idevice::{
        afc::AfcClient,
        installation_proxy::InstallationProxyClient,
        lockdown::LockdownClient,
        pairing_file::PairingFile,
        provider::IdeviceProvider,
        services::diagnostics_relay::DiagnosticsRelayClient,
        services::syslog_relay::SyslogRelayClient,
        usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
        IdeviceService,
    };
    use plist::Value;

    pub(super) struct RealDevice {
        provider: Box<dyn IdeviceProvider>,
        pairing: PairingFile,
        udid: String,
    }

    impl RealDevice {
        pub(super) async fn connect(target_udid: Option<&str>) -> Result<Self> {
            use std::io::IsTerminal;
            let mut usbmuxd = UsbmuxdConnection::default().await.map_err(|e| {
                anyhow!(
                    "Could not reach usbmuxd on this Mac ({e:?}). Make sure macOS sees the device \
                     in Finder, then try again."
                )
            })?;

            let devs = usbmuxd
                .get_devices()
                .await
                .context("usbmuxd refused to list devices")?;

            if devs.is_empty() {
                return Err(anyhow!(
                    "No iPhone connected. Plug it in via cable and tap 'Trust' on the device \
                     when prompted."
                ));
            }

            // Prefer USB over Wi-Fi when both shapes of the same device are
            // listed — Wi-Fi pairing is out of scope for the MVP.
            let usb_devs: Vec<_> = devs
                .iter()
                .filter(|d| d.connection_type == Connection::Usb)
                .collect();
            let candidates: Vec<_> = if !usb_devs.is_empty() {
                usb_devs
            } else {
                devs.iter().collect()
            };

            let chosen = if let Some(want) = target_udid {
                candidates
                    .iter()
                    .find(|d| d.udid == want)
                    .copied()
                    .ok_or_else(|| {
                        let available = candidates
                            .iter()
                            .map(|d| d.udid.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        anyhow!(
                            "No device with UDID `{want}` is connected. Available: [{available}]. \
                             Run `qk devices` to see what's plugged in."
                        )
                    })?
            } else if candidates.len() == 1 {
                candidates[0]
            } else if std::io::stderr().is_terminal() {
                pick_device_interactively(&candidates).await?
            } else {
                let udids = candidates
                    .iter()
                    .map(|d| d.udid.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(anyhow!(
                    "{} iPhones connected. Pass --udid <UDID> (or set QK_UDID) to pick one. \
                     Available: [{udids}]. Run `qk devices` to see names + models.",
                    candidates.len()
                ));
            };

            let udid = chosen.udid.clone();
            let provider: Box<dyn IdeviceProvider> = Box::new(chosen.to_provider(
                UsbmuxdAddr::from_env_var().context("Could not resolve usbmuxd socket address")?,
                "quokka",
            ));

            let pairing = provider.get_pairing_file().await.map_err(|e| {
                anyhow!(
                    "No pairing file for this device ({e:?}). Unlock the iPhone and tap 'Trust \
                     this computer'."
                )
            })?;

            Ok(Self {
                provider,
                pairing,
                udid,
            })
        }
    }

    async fn pick_device_interactively<'a>(
        candidates: &'a [&'a idevice::usbmuxd::UsbmuxdDevice],
    ) -> Result<&'a idevice::usbmuxd::UsbmuxdDevice> {
        let listings = enrich_candidates(candidates).await;
        let items: Vec<String> = candidates
            .iter()
            .zip(listings.iter())
            .map(|(d, ident)| {
                let conn = if d.connection_type == Connection::Usb {
                    "USB"
                } else {
                    "Wi-Fi"
                };
                let name = ident
                    .0
                    .as_deref()
                    .unwrap_or("(untrusted — tap Trust on the device)");
                let model = ident.1.as_deref().unwrap_or("?");
                format!("{name}  ·  {model}  ·  {conn}  ·  {}", d.udid)
            })
            .collect();
        let sel = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Multiple iPhones connected — pick one")
            .items(&items)
            .default(0)
            .interact_opt()
            .map_err(|e| anyhow!("picker failed: {e}"))?;
        let idx = sel.ok_or_else(|| anyhow!("Aborted."))?;
        Ok(candidates[idx])
    }

    /// Brief lockdown probe of every candidate to fetch DeviceName +
    /// ProductType. Returns `(name, friendly_model)` per device, falling
    /// back to `None` when the device isn't paired/trusted yet.
    async fn enrich_candidates(
        candidates: &[&idevice::usbmuxd::UsbmuxdDevice],
    ) -> Vec<(Option<String>, Option<String>)> {
        let addr = match UsbmuxdAddr::from_env_var() {
            Ok(a) => a,
            Err(_) => return vec![(None, None); candidates.len()],
        };
        let mut out = Vec::with_capacity(candidates.len());
        for d in candidates {
            let provider = d.to_provider(addr.clone(), "quokka-list");
            out.push(read_identity_quick(&provider).await);
        }
        out
    }

    async fn read_identity_quick(
        provider: &dyn IdeviceProvider,
    ) -> (Option<String>, Option<String>) {
        let Ok(mut lock) = LockdownClient::connect(provider).await else {
            return (None, None);
        };
        let pairing = provider.get_pairing_file().await.ok();
        if let Some(p) = pairing.as_ref() {
            let _ = lock.start_session(p).await;
        }
        let name = lock
            .get_value(Some("DeviceName"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(String::from));
        let model = lock
            .get_value(Some("ProductType"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(String::from));
        let friendly = model
            .as_deref()
            .and_then(model_names::friendly_name)
            .map(String::from);
        (name, friendly.or(model))
    }

    pub(super) async fn list_devices_impl() -> Result<Vec<DeviceListing>> {
        let mut usbmuxd = UsbmuxdConnection::default()
            .await
            .map_err(|e| anyhow!("Could not reach usbmuxd ({e:?})."))?;
        let devs = usbmuxd
            .get_devices()
            .await
            .context("usbmuxd refused to list devices")?;
        let refs: Vec<&_> = devs.iter().collect();
        let identities = enrich_candidates(&refs).await;
        let mut out = Vec::with_capacity(devs.len());
        for (d, (name, friendly)) in devs.iter().zip(identities) {
            // `friendly` may already be the raw model id when no marketing
            // name resolved; surface it as both fields then.
            let model_identifier = friendly.clone();
            let model_friendly = friendly
                .as_deref()
                .and_then(model_names::friendly_name)
                .map(String::from);
            out.push(DeviceListing {
                udid: d.udid.clone(),
                connection: if d.connection_type == Connection::Usb {
                    "USB"
                } else {
                    "Wi-Fi"
                },
                name,
                model_identifier,
                model_friendly,
            });
        }
        Ok(out)
    }

    impl RealDevice {
        #[cfg(feature = "e2e")]
        pub(super) fn provider_ref(&self) -> &dyn IdeviceProvider {
            &*self.provider
        }
    }

    #[async_trait]
    impl Device for RealDevice {
        async fn status(&self) -> Result<DeviceStatus> {
            // Three independent service connections — overlap them so the
            // welcome screen's wall-clock time is bounded by the slowest one.
            // `count_user_apps` is best-effort: if installation_proxy is busy
            // the dashboard still renders without an app count.
            let (lockdown, diag, app_count) = tokio::join!(
                self.read_lockdown_info(),
                read_battery_diag(&*self.provider),
                count_user_apps(&*self.provider),
            );
            let info = lockdown?;
            let model_friendly = info
                .model
                .as_deref()
                .and_then(model_names::friendly_name)
                .map(str::to_string);
            let battery = Battery {
                level_percent: info.battery_level,
                is_charging: info.is_charging,
                adapter_watts: info.adapter_watts,
                adapter_description: info.adapter_description,
                ..diag
            };
            Ok(DeviceStatus {
                name: info.name,
                model: info.model,
                model_friendly,
                ios_version: info.ios_version,
                ios_build: info.ios_build,
                enclosure_color: info.enclosure_color,
                storage: info.storage,
                battery,
                locale: info.locale,
                time_zone: info.time_zone,
                app_count,
                developer_mode: info.developer_mode,
                find_my: info.find_my,
                last_backup_unix: info.last_backup_unix,
                paired_since_unix: read_paired_since_unix(&self.udid),
            })
        }

        async fn apps(&self) -> Result<Vec<App>> {
            // Server-side filter to "User" — system apps are dropped by the
            // device, cutting payload and plist parse on installs with lots
            // of preinstalled Apple apps. Matches what the upstream `idevice`
            // tool does (`get_apps(Some("User"), None)`).
            lookup_apps(&*self.provider, None, false, "User").await
        }

        async fn with_dynamic_sizes(
            &self,
            apps: Vec<App>,
            on_batch: BatchCallback,
        ) -> Result<Vec<App>> {
            enrich_with_dynamic_sizes(&*self.provider, apps, on_batch).await
        }

        async fn app(&self, bundle_id: &str) -> Result<Option<App>> {
            // Single-bundle lookup with `DynamicDiskUsage`: server-side
            // scoped to one app, so the full size is cheap to compute.
            let mut found = lookup_apps(
                &*self.provider,
                Some(vec![bundle_id.to_string()]),
                true,
                "Any",
            )
            .await?;
            Ok(found.pop())
        }

        async fn uninstall_app(&self, bundle_id: &str) -> Result<()> {
            let mut ip = InstallationProxyClient::connect(&*self.provider)
                .await
                .map_err(|e| anyhow!("Could not open installation_proxy ({e:?})"))?;
            ip.uninstall(bundle_id.to_string(), None)
                .await
                .map_err(|e| anyhow!("uninstall {bundle_id} failed: {e:?}"))
        }

        async fn afc_walk(
            &self,
            roots: &[&str],
            on_progress: WalkCallback,
        ) -> Result<Vec<MediaFile>> {
            afc_walk_impl(&*self.provider, roots, on_progress).await
        }

        async fn afc_delete(&self, path: &str) -> Result<()> {
            let mut afc = AfcClient::connect(&*self.provider).await.map_err(|e| {
                anyhow!("Could not open AFC ({e}). Unlock the iPhone and try again.")
            })?;
            afc.remove(path.to_string())
                .await
                .map_err(|e| anyhow!("delete {path} failed: {e}"))
        }

        async fn info(&self) -> Result<DeviceInfo> {
            read_device_info(&*self.provider, &self.pairing).await
        }

        async fn reboot(&self) -> Result<()> {
            let mut diag = DiagnosticsRelayClient::connect(&*self.provider)
                .await
                .map_err(|e| anyhow!("Could not open diagnostics_relay ({e:?})"))?;
            diag.restart()
                .await
                .map_err(|e| anyhow!("reboot request failed: {e:?}"))
        }

        async fn shutdown(&self) -> Result<()> {
            let mut diag = DiagnosticsRelayClient::connect(&*self.provider)
                .await
                .map_err(|e| anyhow!("Could not open diagnostics_relay ({e:?})"))?;
            diag.shutdown()
                .await
                .map_err(|e| anyhow!("shutdown request failed: {e:?}"))
        }

        async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>> {
            let mut syslog = SyslogRelayClient::connect(&*self.provider)
                .await
                .map_err(|e| {
                    anyhow!("Could not open syslog_relay ({e:?}). Is the device trusted?")
                })?;
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogEntry>>(1024);
            tokio::spawn(async move {
                // Continuation-aware: lines that start with whitespace
                // append to the previous entry's message.
                let mut pending: Option<LogEntry> = None;
                loop {
                    let raw = match syslog.next().await {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.send(Err(anyhow!("syslog stream ended: {e:?}"))).await;
                            break;
                        }
                    };
                    if crate::commands::logs::parser::is_continuation(&raw) {
                        if let Some(prev) = pending.as_mut() {
                            prev.message.push('\n');
                            prev.message.push_str(raw.trim_end());
                            continue;
                        }
                    }
                    if let Some(entry) = pending.take() {
                        if tx.send(Ok(entry)).await.is_err() {
                            break;
                        }
                    }
                    pending = Some(crate::commands::logs::parser::parse_syslog_line(&raw));
                }
                if let Some(entry) = pending.take() {
                    let _ = tx.send(Ok(entry)).await;
                }
            });
            Ok(rx)
        }
    }

    async fn read_device_info(
        provider: &dyn IdeviceProvider,
        pairing: &PairingFile,
    ) -> Result<DeviceInfo> {
        let mut lock = LockdownClient::connect(provider)
            .await
            .map_err(|e| anyhow!("Could not open lockdown ({e:?}). Is the device trusted?"))?;
        lock.start_session(pairing)
            .await
            .map_err(|e| anyhow!("Lockdown session failed: {e:?}"))?;

        let name = read_string(&mut lock, "DeviceName", None)
            .await
            .ok_or_else(|| anyhow!("Could not read device identity (failed on: DeviceName)"))?;
        let model_identifier = read_string(&mut lock, "ProductType", None)
            .await
            .ok_or_else(|| anyhow!("Could not read device identity (failed on: ProductType)"))?;
        let serial = read_string(&mut lock, "SerialNumber", None)
            .await
            .ok_or_else(|| anyhow!("Could not read device identity (failed on: SerialNumber)"))?;
        let udid = read_string(&mut lock, "UniqueDeviceID", None)
            .await
            .ok_or_else(|| anyhow!("Could not read device identity (failed on: UniqueDeviceID)"))?;
        let ios_version = read_string(&mut lock, "ProductVersion", None)
            .await
            .ok_or_else(|| anyhow!("Could not read device identity (failed on: ProductVersion)"))?;

        let model_friendly = model_names::friendly_name(&model_identifier).map(str::to_string);

        Ok(DeviceInfo {
            name,
            model_identifier,
            model_friendly,
            model_number: read_string(&mut lock, "ModelNumber", None).await,
            region_info: read_string(&mut lock, "RegionInfo", None).await,
            enclosure_color: match read_string(&mut lock, "DeviceEnclosureColor", None).await {
                Some(v) => Some(v),
                None => read_string(&mut lock, "DeviceColor", None).await,
            },
            serial,
            udid,
            ios_version,
            ios_build: read_string(&mut lock, "BuildVersion", None).await,
            hardware_model: read_string(&mut lock, "HardwareModel", None).await,
            cpu_architecture: read_string(&mut lock, "CPUArchitecture", None).await,
            activation_state: read_string(&mut lock, "ActivationState", None).await,
            is_supervised: read_bool(
                &mut lock,
                "IsSupervised",
                Some("com.apple.mobile.chaperone"),
            )
            .await,
            developer_mode_enabled: read_bool(
                &mut lock,
                "DeveloperModeStatus",
                Some("com.apple.security.mac.amfi"),
            )
            .await,
            wifi_address: read_string(&mut lock, "WiFiAddress", None).await,
            bluetooth_address: read_string(&mut lock, "BluetoothAddress", None).await,
            imei: read_string(&mut lock, "InternationalMobileEquipmentIdentity", None).await,
            imei2: read_string(&mut lock, "InternationalMobileEquipmentIdentity2", None).await,
        })
    }

    /// `on_progress` fires every ~100 files or ~200 ms, whichever comes
    /// first, to keep the spinner update channel cheap on large walks.
    /// Per-entry errors are logged and skipped — the walk only aborts if
    /// AFC itself can't be reached.
    async fn afc_walk_impl(
        provider: &dyn IdeviceProvider,
        roots: &[&str],
        on_progress: WalkCallback,
    ) -> Result<Vec<MediaFile>> {
        use std::time::{Duration, Instant};
        let mut afc = AfcClient::connect(provider)
            .await
            .map_err(|e| anyhow!("Could not open AFC ({e}). Unlock the iPhone and try again."))?;

        let mut files = Vec::new();
        let mut queue: std::collections::VecDeque<String> =
            roots.iter().map(|r| (*r).to_string()).collect();

        let mut bytes_seen: u64 = 0;
        let mut since_tick = 0usize;
        let mut last_tick = Instant::now();

        while let Some(dir) = queue.pop_front() {
            let entries = match afc.list_dir(dir.clone()).await {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("warning: skipping {dir}: {e}");
                    continue;
                }
            };
            for entry in entries {
                if entry == "." || entry == ".." {
                    continue;
                }
                let full = if dir.ends_with('/') {
                    format!("{dir}{entry}")
                } else {
                    format!("{dir}/{entry}")
                };
                let info = match afc.get_file_info(full.clone()).await {
                    Ok(i) => i,
                    Err(e) => {
                        eprintln!("warning: skipping {full}: {e}");
                        continue;
                    }
                };
                match info.st_ifmt.as_str() {
                    "S_IFDIR" => queue.push_back(full),
                    "S_IFREG" => {
                        let size = info.size as u64;
                        bytes_seen = bytes_seen.saturating_add(size);
                        files.push(MediaFile {
                            path: full,
                            size_bytes: size,
                            modified_unix: info.modified.and_utc().timestamp(),
                        });
                        since_tick += 1;
                        if since_tick >= 100 || last_tick.elapsed() >= Duration::from_millis(200) {
                            on_progress(WalkProgress {
                                files_seen: files.len(),
                                bytes_seen,
                            });
                            since_tick = 0;
                            last_tick = Instant::now();
                        }
                    }
                    // Symlinks and other special types: ignore.
                    _ => {}
                }
            }
        }

        on_progress(WalkProgress {
            files_seen: files.len(),
            bytes_seen,
        });
        Ok(files)
    }

    /// Single `installation_proxy.browse` call. Pass `bundle_ids = Some(...)`
    /// to scope server-side (essential when `include_dynamic` is true — see
    /// note below). When `include_dynamic` is true, the device walks each
    /// requested app's container to compute `DynamicDiskUsage`, which on
    /// iOS 26+ can stall a *bulk* browse for minutes. Scoping it to small
    /// batches keeps every call bounded.
    async fn lookup_apps(
        provider: &dyn IdeviceProvider,
        bundle_ids: Option<Vec<String>>,
        include_dynamic: bool,
        application_type: &str,
    ) -> Result<Vec<App>> {
        let mut ip = InstallationProxyClient::connect(provider)
            .await
            .map_err(|e| anyhow!("Could not open installation_proxy ({e:?})"))?;

        let mut opts = plist::Dictionary::new();
        opts.insert("ApplicationType".into(), Value::from(application_type));
        if let Some(ids) = bundle_ids {
            opts.insert(
                "BundleIDs".into(),
                Value::Array(ids.into_iter().map(Value::from).collect()),
            );
        }

        let mut attrs = vec![
            "CFBundleIdentifier",
            "CFBundleDisplayName",
            "CFBundleName",
            "ApplicationType",
            "StaticDiskUsage",
        ];
        if include_dynamic {
            attrs.push("DynamicDiskUsage");
        }
        opts.insert(
            "ReturnAttributes".into(),
            Value::Array(attrs.into_iter().map(Value::from).collect()),
        );

        let raw = ip
            .browse(Some(Value::Dictionary(opts)))
            .await
            .map_err(|e| anyhow!("browse failed: {e:?}"))?;
        Ok(raw.into_iter().filter_map(parse_app).collect())
    }

    // Tuned by the `sweep_batch_and_concurrency` e2e bench: (16, 8) was
    // 1.11× faster than the previous (8, 4) default on a 299-app device.
    // Re-run the bench on a new iOS major before changing these.
    pub(super) const BATCH_SIZE: usize = 16;
    pub(super) const MAX_CONCURRENT: usize = 8;

    /// Enrich `apps` with `DynamicDiskUsage` via parallel batched `browse`s.
    /// Each batch is scoped to a small `BundleIDs` set; concurrency caps the
    /// in-flight count so the device isn't hammered. `progress` fires after
    /// each batch with cumulative `(apps_done, apps_total)`.
    async fn enrich_with_dynamic_sizes(
        provider: &dyn IdeviceProvider,
        apps: Vec<App>,
        on_batch: BatchCallback,
    ) -> Result<Vec<App>> {
        enrich_with_dynamic_sizes_tuned(provider, apps, on_batch, BATCH_SIZE, MAX_CONCURRENT).await
    }

    /// Tunable variant of [`enrich_with_dynamic_sizes`]. Production paths use
    /// the constants above; the e2e benchmark sweeps a grid via
    /// [`super::bench`] to find the best combo on real hardware.
    pub(super) async fn enrich_with_dynamic_sizes_tuned(
        provider: &dyn IdeviceProvider,
        apps: Vec<App>,
        on_batch: BatchCallback,
        batch_size: usize,
        max_concurrent: usize,
    ) -> Result<Vec<App>> {
        use futures::stream::{FuturesUnordered, StreamExt};

        let batch_size = batch_size.max(1);
        let max_concurrent = max_concurrent.max(1);

        if apps.is_empty() {
            return Ok(apps);
        }

        let total = apps.len();
        // Heaviest-first: users care most about CapCut/Insta360/DJI-class
        // apps; surfacing their real sizes early is the whole point.
        let mut sorted = apps;
        sorted.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

        let batches: Vec<Vec<String>> = sorted
            .chunks(batch_size)
            .map(|c| c.iter().map(|a| a.bundle_id.clone()).collect())
            .collect();
        let index: std::collections::HashMap<String, usize> = sorted
            .iter()
            .enumerate()
            .map(|(i, a)| (a.bundle_id.clone(), i))
            .collect();

        let mut in_flight = FuturesUnordered::new();
        let mut next_batch = 0;
        while next_batch < batches.len() && in_flight.len() < max_concurrent {
            in_flight.push(lookup_apps(
                provider,
                Some(batches[next_batch].clone()),
                true,
                "Any",
            ));
            next_batch += 1;
        }

        let mut done = 0;
        while let Some(result) = in_flight.next().await {
            let updated = result?;
            done += updated.len();
            for app in &updated {
                if let Some(&i) = index.get(&app.bundle_id) {
                    sorted[i] = app.clone();
                }
            }
            on_batch(BatchUpdate {
                apps: updated,
                done: done.min(total),
                total,
            });
            if next_batch < batches.len() {
                in_flight.push(lookup_apps(
                    provider,
                    Some(batches[next_batch].clone()),
                    true,
                    "Any",
                ));
                next_batch += 1;
            }
        }

        Ok(sorted)
    }

    /// Convert one entry of `InstallationProxyClient::browse`'s output into
    /// an [`App`]. Returns `None` if the entry is not a dictionary or has no
    /// bundle identifier — the device occasionally lists incomplete records.
    fn parse_app(value: plist::Value) -> Option<App> {
        let dict = value.into_dictionary()?;
        let bundle_id = dict
            .get("CFBundleIdentifier")
            .and_then(plist::Value::as_string)?
            .to_string();
        let name = dict
            .get("CFBundleDisplayName")
            .and_then(plist::Value::as_string)
            .or_else(|| dict.get("CFBundleName").and_then(plist::Value::as_string))
            .unwrap_or(&bundle_id)
            .to_string();
        let is_system = dict
            .get("ApplicationType")
            .and_then(plist::Value::as_string)
            .is_some_and(|t| !t.eq_ignore_ascii_case("User"));
        let static_size = dict
            .get("StaticDiskUsage")
            .and_then(plist_as_u64)
            .unwrap_or(0);
        let dynamic_size = dict
            .get("DynamicDiskUsage")
            .and_then(plist_as_u64)
            .unwrap_or(0);
        Some(App {
            bundle_id,
            name,
            size_bytes: static_size.saturating_add(dynamic_size),
            is_system,
        })
    }

    pub(super) struct LockdownInfo {
        pub name: Option<String>,
        pub model: Option<String>,
        pub ios_version: Option<String>,
        pub ios_build: Option<String>,
        pub enclosure_color: Option<String>,
        pub storage: Option<Storage>,
        pub battery_level: Option<u8>,
        pub is_charging: Option<bool>,
        pub adapter_watts: Option<u32>,
        pub adapter_description: Option<String>,
        pub locale: Option<String>,
        pub time_zone: Option<String>,
        pub developer_mode: Option<bool>,
        pub find_my: Option<bool>,
        pub last_backup_unix: Option<i64>,
    }

    impl RealDevice {
        async fn read_lockdown_info(&self) -> Result<LockdownInfo> {
            let mut lock = LockdownClient::connect(&*self.provider)
                .await
                .map_err(|e| anyhow!("Could not open lockdown ({e:?}). Is the device trusted?"))?;
            lock.start_session(&self.pairing)
                .await
                .map_err(|e| anyhow!("Lockdown session failed: {e:?}"))?;

            let name = read_string(&mut lock, "DeviceName", None).await;
            let model = read_string(&mut lock, "ProductType", None).await;
            let ios_version = read_string(&mut lock, "ProductVersion", None).await;
            let ios_build = read_string(&mut lock, "BuildVersion", None).await;
            // `DeviceEnclosureColor` is the modern key; on older devices Apple
            // exposed `DeviceColor` instead — fall back so the colour mapping
            // has a chance on iPhone XS / XR vintages.
            let enclosure_color = match read_string(&mut lock, "DeviceEnclosureColor", None).await {
                Some(v) => Some(v),
                None => read_string(&mut lock, "DeviceColor", None).await,
            };
            let storage = read_storage(&mut lock).await;
            let battery_domain = Some("com.apple.mobile.battery");
            // MobileGestalt was deprecated for diagnostics_relay in iOS 17.4;
            // the lockdown battery domain is the supported path now.
            let battery_level = read_u64(&mut lock, "BatteryCurrentCapacity", battery_domain)
                .await
                .and_then(|n| u8::try_from(n).ok());
            // Both keys are queried — different iOS minor versions surface
            // different ones, and either signal answers "is the cable in?".
            let is_charging = read_bool(&mut lock, "BatteryIsCharging", battery_domain)
                .await
                .or(read_bool(&mut lock, "ExternalConnected", battery_domain).await);
            let (adapter_watts, adapter_description) = read_adapter_details(&mut lock).await;
            // Locale comes back as `pt_BR`-style; normalize to BCP-47 (`pt-BR`).
            let locale = read_string(&mut lock, "Locale", None)
                .await
                .map(|s| s.replace('_', "-"));
            let time_zone = read_string(&mut lock, "TimeZone", None).await;
            let developer_mode = read_bool(
                &mut lock,
                "DeveloperModeStatus",
                Some("com.apple.security.mac.amfi"),
            )
            .await;
            let find_my = read_bool(&mut lock, "FMIPEnabled", Some("com.apple.fmip.fmipd")).await;
            let last_backup_unix = read_date_unix(
                &mut lock,
                "LastiTunesSyncFromDevice",
                Some("com.apple.mobile.iTunes"),
            )
            .await;
            Ok(LockdownInfo {
                name,
                model,
                ios_version,
                ios_build,
                enclosure_color,
                storage,
                battery_level,
                is_charging,
                adapter_watts,
                adapter_description,
                locale,
                time_zone,
                developer_mode,
                find_my,
                last_backup_unix,
            })
        }
    }

    /// Read `AdapterDetails` from the lockdown battery domain. The key is
    /// only populated while charging — when nothing is plugged in it returns
    /// an empty dict or no value at all, which surfaces as `(None, None)`.
    async fn read_adapter_details(lock: &mut LockdownClient) -> (Option<u32>, Option<String>) {
        let value = match lock
            .get_value(Some("AdapterDetails"), Some("com.apple.mobile.battery"))
            .await
        {
            Ok(v) => v,
            Err(_) => return (None, None),
        };
        let Some(dict) = value.as_dictionary() else {
            return (None, None);
        };
        let watts = dict
            .get("Watts")
            .and_then(plist_as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let description = dict
            .get("Description")
            .and_then(plist::Value::as_string)
            .map(|s| s.to_string());
        (watts, description)
    }

    async fn count_user_apps(provider: &dyn IdeviceProvider) -> Option<usize> {
        lookup_apps(provider, None, false, "User")
            .await
            .ok()
            .map(|apps| apps.len())
    }

    /// Pair record creation time. Two standard locations on macOS — the
    /// system-wide path (`/var/db/lockdown`) is usually only readable as
    /// root, the per-user path (`~/Library/Lockdown`) is the modern default.
    /// Returns `None` if neither file exists or the filesystem doesn't
    /// expose a creation time.
    fn read_paired_since_unix(udid: &str) -> Option<i64> {
        let candidates = [
            std::env::var("HOME").ok().map(|home| {
                std::path::PathBuf::from(home).join(format!("Library/Lockdown/{udid}.plist"))
            }),
            Some(std::path::PathBuf::from(format!(
                "/var/db/lockdown/{udid}.plist"
            ))),
        ];
        candidates
            .into_iter()
            .flatten()
            .filter_map(|path| std::fs::metadata(&path).ok()?.created().ok())
            .filter_map(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .next()
    }

    async fn read_date_unix(
        lock: &mut LockdownClient,
        key: &str,
        domain: Option<&str>,
    ) -> Option<i64> {
        let value = lock.get_value(Some(key), domain).await.ok()?;
        // `idevice`'s plist exposes dates via `as_date` → `plist::Date`,
        // which converts to `SystemTime`. Anything that fails to parse as a
        // date is silently dropped.
        let date = value.as_date()?;
        let system_time: std::time::SystemTime = date.into();
        system_time
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64)
    }

    async fn read_string(
        lock: &mut LockdownClient,
        key: &str,
        domain: Option<&str>,
    ) -> Option<String> {
        let value = lock.get_value(Some(key), domain).await.ok()?;
        value.as_string().map(|s| s.to_string())
    }

    async fn read_storage(lock: &mut LockdownClient) -> Option<Storage> {
        let domain = Some("com.apple.disk_usage");
        let total = read_u64(lock, "TotalDiskCapacity", domain).await?;
        let free = read_u64(lock, "AmountDataAvailable", domain).await?;
        let system_bytes = read_u64(lock, "TotalSystemCapacity", domain).await;
        // Data partition usage: total data capacity minus what's free there.
        // `free` already holds `AmountDataAvailable`, which is the data
        // partition's free space — reuse it instead of re-querying.
        let data_used_bytes = read_u64(lock, "TotalDataCapacity", domain)
            .await
            .map(|cap| cap.saturating_sub(free));
        Some(Storage {
            total_bytes: total,
            free_bytes: free,
            system_bytes,
            data_used_bytes,
        })
    }

    async fn read_u64(lock: &mut LockdownClient, key: &str, domain: Option<&str>) -> Option<u64> {
        let value = lock.get_value(Some(key), domain).await.ok()?;
        plist_as_u64(&value)
    }

    async fn read_bool(lock: &mut LockdownClient, key: &str, domain: Option<&str>) -> Option<bool> {
        let value = lock.get_value(Some(key), domain).await.ok()?;
        // Some lockdown plists box booleans as integers (0/1) instead of the
        // plist boolean type — accept either shape.
        value
            .as_boolean()
            .or_else(|| plist_as_u64(&value).map(|n| n != 0))
    }

    fn plist_as_u64(value: &Value) -> Option<u64> {
        // Some plist producers tag non-negative counts as signed integers
        // (notably gas-gauge fields on certain iOS versions). The signed
        // fallback recovers those without ever accepting a true negative.
        value.as_unsigned_integer().or_else(|| {
            value
                .as_signed_integer()
                .and_then(|n| u64::try_from(n).ok())
        })
    }

    fn compute_health_percent(gas: &plist::Dictionary) -> Option<u8> {
        let fcc = gas.get("FullChargeCapacity").and_then(plist_as_u64)?;
        // iOS 17+ returns FullChargeCapacity already as a percentage of
        // design capacity (the same number iOS Settings shows as "Maximum
        // Capacity"). Older iOS returned it in mAh, in which case fall back
        // to computing the ratio against DesignCapacity.
        if fcc <= 100 {
            return u8::try_from(fcc).ok();
        }
        let design = gas.get("DesignCapacity").and_then(plist_as_u64)?;
        let ratio = fcc.saturating_mul(100).checked_div(design)?;
        u8::try_from(ratio).ok()
    }

    /// Reads battery metrics that come from `diagnostics_relay`: cycle count
    /// and health. Level is read separately from the lockdown battery domain
    /// (MobileGestalt was deprecated in iOS 17.4). Temperature is no longer
    /// exposed via `gasguage` on iOS 17+ either — would require an
    /// `ioregistry` dump of `AppleSmartBattery`, which is many KB for one
    /// number; not worth the cost in the MVP.
    async fn read_battery_diag(provider: &dyn IdeviceProvider) -> Battery {
        let Ok(mut diag) = DiagnosticsRelayClient::connect(provider).await else {
            return Battery::default();
        };

        let mut battery = Battery::default();

        // The gasguage response is wrapped one level deep:
        //   { "GasGauge": { "CycleCount": ..., "FullChargeCapacity": ..., ... } }
        if let Ok(Some(dict)) = diag.gasguage().await {
            let gas = dict.get("GasGauge").and_then(plist::Value::as_dictionary);
            if let Some(gas) = gas {
                battery.cycle_count = gas
                    .get("CycleCount")
                    .and_then(plist_as_u64)
                    .and_then(|n| u32::try_from(n).ok());
                battery.health_percent = compute_health_percent(gas);
            }
        }

        battery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_default_returns_seed_status() {
        let fake = FakeDevice::default();
        let status = fake.status().await.unwrap();
        assert_eq!(status.name.as_deref(), Some("Test iPhone"));
        assert_eq!(status.ios_version.as_deref(), Some("18.2"));
        assert!(status.storage.is_some());
        assert_eq!(status.battery.level_percent, Some(87));
    }

    #[tokio::test]
    async fn fake_with_status_overrides_default() {
        let fake = FakeDevice::with_status(DeviceStatus {
            name: Some("Lucas's iPhone".into()),
            ..Default::default()
        });
        let status = fake.status().await.unwrap();
        assert_eq!(status.name.as_deref(), Some("Lucas's iPhone"));
        assert!(status.model.is_none());
        assert!(status.battery.level_percent.is_none());
    }

    #[tokio::test]
    async fn fake_propagates_seeded_error() {
        let fake = FakeDevice::with_status_error("device not trusted");
        let err = fake.status().await.unwrap_err();
        assert!(err.to_string().contains("device not trusted"));
    }

    #[test]
    fn storage_used_percent_rounds_to_nearest() {
        // 100 GB total, 40 GB free → 60% used.
        let s = Storage {
            total_bytes: 100,
            free_bytes: 40,
            ..Storage::default()
        };
        assert_eq!(s.used_percent(), 60);
        assert_eq!(s.used_bytes(), 60);
    }

    #[test]
    fn storage_used_percent_handles_zero_total() {
        let s = Storage {
            total_bytes: 0,
            free_bytes: 0,
            ..Storage::default()
        };
        assert_eq!(s.used_percent(), 0);
    }

    #[test]
    fn storage_used_percent_handles_free_gt_total() {
        // Defensive: a malformed report (free > total) shouldn't panic.
        let s = Storage {
            total_bytes: 10,
            free_bytes: 20,
            ..Storage::default()
        };
        assert_eq!(s.used_bytes(), 0);
        assert_eq!(s.used_percent(), 0);
    }
}

/// Benchmark hook for sweeping the two knobs in `enrich_with_dynamic_sizes`
/// (batch size, in-flight cap) against a real device. Lives behind the `e2e`
/// feature because it only makes sense with a connected iPhone — a fake
/// device's enrichment is synchronous and tells you nothing about the real
/// I/O cost. See `tests/e2e_enrich_bench.rs`.
#[cfg(feature = "e2e")]
pub mod bench {
    use super::*;
    use std::time::{Duration, Instant};

    pub const DEFAULT_BATCH_SIZE: usize = real::BATCH_SIZE;
    pub const DEFAULT_MAX_CONCURRENT: usize = real::MAX_CONCURRENT;

    pub struct Harness(real::RealDevice);

    impl Harness {
        pub async fn connect() -> Result<Self> {
            Ok(Self(real::RealDevice::connect().await?))
        }

        /// Phase 1 fetch: bundle sizes only, single round-trip. Use this once
        /// per benchmark run and reuse the returned list across sweeps so
        /// every combo enriches the same input.
        pub async fn apps(&self) -> Result<Vec<App>> {
            <real::RealDevice as Device>::apps(&self.0).await
        }

        /// Run one parameterized enrichment and return wall time. Discards
        /// per-batch progress — the bench only cares about end-to-end cost.
        pub async fn enrich_timed(
            &self,
            apps: Vec<App>,
            batch_size: usize,
            max_concurrent: usize,
        ) -> Result<Duration> {
            let start = Instant::now();
            real::enrich_with_dynamic_sizes_tuned(
                self.0.provider_ref(),
                apps,
                Box::new(|_| {}),
                batch_size,
                max_concurrent,
            )
            .await?;
            Ok(start.elapsed())
        }
    }
}
