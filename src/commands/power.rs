//! `quokka reboot` and `quokka shutdown` — destructive companion commands
//! that drive the iPhone's power state through `diagnostics_relay`.

use std::io::Write;

use anyhow::{anyhow, bail, Result};
use dialoguer::{theme::ColorfulTheme, Confirm};
use owo_colors::OwoColorize;

use crate::device::{Device, DeviceStatus};
use crate::ui::spinner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reboot,
    Shutdown,
}

struct ActionLabels {
    confirm_verb: &'static str,
    spinner: &'static str,
    success: &'static str,
}

const REBOOT_LABELS: ActionLabels = ActionLabels {
    confirm_verb: "Reboot",
    spinner: "Sending restart...",
    success: "Restart requested. The device will disconnect shortly.",
};

const SHUTDOWN_LABELS: ActionLabels = ActionLabels {
    confirm_verb: "Shutdown",
    spinner: "Sending shutdown...",
    success: "Shutdown requested. The device will power off shortly.",
};

fn labels(action: Action) -> &'static ActionLabels {
    match action {
        Action::Reboot => &REBOOT_LABELS,
        Action::Shutdown => &SHUTDOWN_LABELS,
    }
}

pub async fn run(device: &dyn Device, action: Action, yes: bool) -> Result<()> {
    let labels = labels(action);

    if !yes {
        // The confirm prompt reads stdin, so that's the stream that has to
        // be a TTY. Checking stdout was a bug: `qk reboot | tee` in an
        // interactive shell would refuse even though the user could type.
        if !crate::ui::stdin_is_interactive() {
            bail!(
                "refusing to run a destructive action without confirmation. \
                 Re-run with `--yes` or run from an interactive terminal."
            );
        }
        // Read status only when we'll actually use it for the prompt label.
        // With `--yes` we skip the extra lockdown round-trip entirely.
        let status = device.status().await.ok();
        let prompt = build_confirm_prompt(labels.confirm_verb, status.as_ref());
        let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .default(false)
            .interact_opt()?;
        if !matches!(confirmed, Some(true)) {
            let mut out = anstream::stdout();
            writeln!(out, "Aborted.")?;
            return Ok(());
        }
    }

    let bar = spinner(labels.spinner);
    let result = match action {
        Action::Reboot => device.reboot().await,
        Action::Shutdown => device.shutdown().await,
    };
    bar.finish_and_clear();
    result.map_err(|e| {
        anyhow!(
            "{} request failed: {e}. The device did not act.",
            labels.confirm_verb.to_lowercase()
        )
    })?;

    let mut out = anstream::stdout();
    writeln!(out, "{} {}", "✓".green(), labels.success)?;
    Ok(())
}

pub fn build_confirm_prompt(verb: &str, status: Option<&DeviceStatus>) -> String {
    let target = device_label(status);
    format!("{verb} {target}?")
}

fn device_label(status: Option<&DeviceStatus>) -> String {
    let Some(s) = status else {
        return "this iPhone".to_string();
    };
    match (s.name.as_deref(), s.model_friendly.as_deref()) {
        (Some(name), Some(model)) => format!("{name} ({model})"),
        (Some(name), None) => name.to_string(),
        (None, Some(model)) => model.to_string(),
        (None, None) => "this iPhone".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceStatus;

    #[test]
    fn labels_distinct_and_nonempty() {
        let r = labels(Action::Reboot);
        let s = labels(Action::Shutdown);
        assert!(!r.confirm_verb.is_empty());
        assert!(!r.spinner.is_empty());
        assert!(!r.success.is_empty());
        assert_ne!(r.confirm_verb, s.confirm_verb);
        assert_ne!(r.spinner, s.spinner);
        assert_ne!(r.success, s.success);
    }

    #[test]
    fn confirm_prompt_includes_device_name() {
        let status = DeviceStatus {
            name: Some("Lucas's iPhone".into()),
            model_friendly: Some("iPhone 15 Pro Max".into()),
            ..Default::default()
        };
        let prompt = build_confirm_prompt("Reboot", Some(&status));
        assert!(prompt.contains("Lucas's iPhone"));
        assert!(prompt.contains("iPhone 15 Pro Max"));
        assert!(prompt.contains("Reboot"));
    }

    #[test]
    fn confirm_prompt_falls_back_when_status_missing() {
        let prompt = build_confirm_prompt("Shutdown", None);
        assert!(prompt.contains("this iPhone"));
        assert!(prompt.contains("Shutdown"));
    }
}
