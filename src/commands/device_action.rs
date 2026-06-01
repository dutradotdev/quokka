//! The set of device-scoped actions the interactive launchers offer, plus the
//! single dispatch that runs one against a connected [`Device`].
//!
//! Both launchers — the `dialoguer` menu (`menu.rs`) and the multi-device
//! ratatui sidebar (`sidebar.rs`) — share this so the action→command mapping
//! lives in exactly one place. Each launcher still owns its own row *ordering*
//! and its own non-action entries (Refresh / Switch / Quit), because those are
//! presentation concerns; only the dispatch is common.

use anyhow::Result;

use crate::commands::{analyze, apps, capture, card, info, logs, media, power, update};
use crate::device::Device;
use crate::ui::now_unix;

/// Rows shown by `analyze`'s interactive picker when launched from a menu.
const ANALYZE_TOP: usize = 20;

/// A device-scoped command reachable from a launcher. Excludes the launchers'
/// own control entries (Refresh, Switch, Quit) — those never touch the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    Apps,
    Analyze,
    Media,
    Logs,
    Capture,
    Info,
    Card,
    Reboot,
    Shutdown,
    Update,
}

impl DeviceAction {
    /// Short menu label.
    pub fn label(self) -> &'static str {
        match self {
            DeviceAction::Apps => "Apps",
            DeviceAction::Analyze => "Analyze",
            DeviceAction::Media => "Media",
            DeviceAction::Logs => "Logs",
            DeviceAction::Capture => "Capture",
            DeviceAction::Info => "Info",
            DeviceAction::Card => "Card",
            DeviceAction::Reboot => "Reboot",
            DeviceAction::Shutdown => "Shutdown",
            DeviceAction::Update => "Update",
        }
    }

    /// One-line description shown next to the label.
    pub fn description(self) -> &'static str {
        match self {
            DeviceAction::Apps => "List & uninstall user apps",
            DeviceAction::Analyze => "Find the heaviest media files",
            DeviceAction::Media => "Survey camera roll & downloads",
            DeviceAction::Logs => "Stream device syslog",
            DeviceAction::Capture => "Stream network packets per app",
            DeviceAction::Info => "Print device identity",
            DeviceAction::Card => "Render a shareable 1080² PNG",
            DeviceAction::Reboot => "Restart the device",
            DeviceAction::Shutdown => "Power off the device",
            DeviceAction::Update => "Check for a new quokka release",
        }
    }
}

/// The actions offered for a device, in canonical order. `capture_supported`
/// is `Device::as_capture().is_some()`; when false the capture entry is omitted
/// so a backend that cannot capture (Android) never offers an action that can
/// only fail with "iOS only".
pub fn actions_for(capture_supported: bool) -> Vec<DeviceAction> {
    [
        DeviceAction::Apps,
        DeviceAction::Analyze,
        DeviceAction::Media,
        DeviceAction::Logs,
        DeviceAction::Capture,
        DeviceAction::Info,
        DeviceAction::Card,
        DeviceAction::Reboot,
        DeviceAction::Shutdown,
        DeviceAction::Update,
    ]
    .into_iter()
    .filter(|a| capture_supported || *a != DeviceAction::Capture)
    .collect()
}

/// Run a single action against `device`. The one place a launcher choice turns
/// into a command call.
pub async fn run(device: &dyn Device, action: DeviceAction) -> Result<()> {
    match action {
        DeviceAction::Apps => {
            apps::run(
                device,
                apps::Options {
                    uninstall: None,
                    assume_yes: false,
                },
            )
            .await
        }
        DeviceAction::Analyze => analyze::run(device, ANALYZE_TOP, true).await,
        DeviceAction::Media => media::run(device, false).await,
        DeviceAction::Logs => logs::run(device, logs::Options::default()).await,
        DeviceAction::Capture => capture::run(device, capture::Options::default()).await,
        DeviceAction::Info => info::run(device, false, false).await,
        DeviceAction::Card => {
            card::run(
                device,
                now_unix(),
                card::CardArgs {
                    output: None,
                    no_open: false,
                    redact: false,
                },
            )
            .await
        }
        DeviceAction::Reboot => power::run(device, power::Action::Reboot, false).await,
        DeviceAction::Shutdown => power::run(device, power::Action::Shutdown, false).await,
        DeviceAction::Update => update::run(false, false).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_for_hides_capture_when_unsupported() {
        assert!(!actions_for(false).contains(&DeviceAction::Capture));
        assert!(actions_for(true).contains(&DeviceAction::Capture));
    }

    #[test]
    fn hiding_capture_removes_exactly_one_entry() {
        assert_eq!(actions_for(true).len(), actions_for(false).len() + 1);
    }

    #[test]
    fn every_action_has_a_nonempty_label_and_description() {
        for action in actions_for(true) {
            assert!(!action.label().is_empty());
            assert!(!action.description().is_empty());
        }
    }
}
