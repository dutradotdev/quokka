//! `qk update` — check GitHub for a newer release and re-run the
//! cargo-dist installer script to replace this binary.
//!
//! Shells out to `curl` and `sh` so we don't need an HTTP client crate.
//! Every macOS and every distro we ship to already has both.

use std::io::Write;

use anyhow::{anyhow, bail, Context, Result};
use owo_colors::OwoColorize;
use tokio::process::Command;

const RELEASES_API: &str = "https://api.github.com/repos/dutradotdev/quokka/releases/latest";

/// Installer URL pinned to a specific release tag. We deliberately avoid the
/// `releases/latest/download/...` alias: `fetch_latest_tag` already resolved
/// the exact version we decided to install, and `latest` could move between
/// that check and this download (a TOCTOU window). Pinning the tag means the
/// script that runs is the one we confirmed. The cargo-dist installer it runs
/// then verifies the per-artifact `.sha256` of the binaries it downloads — the
/// release does not publish a checksum for the installer script itself, so we
/// can't verify it beyond pinning the version and requiring TLS.
fn installer_url(tag: &str) -> String {
    format!("https://github.com/dutradotdev/quokka/releases/download/{tag}/quokka-cli-installer.sh")
}

pub async fn run(check_only: bool, assume_yes: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let mut out = anstream::stdout();

    writeln!(out, "current: v{current}")?;
    write!(out, "checking GitHub for latest release... ")?;
    out.flush()?;

    let latest = fetch_latest_tag().await?;
    writeln!(out, "{latest}")?;

    let latest_stripped = latest.strip_prefix('v').unwrap_or(&latest);
    if latest_stripped == current {
        writeln!(out, "{}", "already on the latest version.".dimmed())?;
        return Ok(());
    }

    if !is_newer(latest_stripped, current) {
        writeln!(
            out,
            "{}",
            format!("current v{current} is ahead of the latest release {latest}; nothing to do.")
                .dimmed()
        )?;
        return Ok(());
    }

    writeln!(
        out,
        "{} v{current} → {}",
        "update available:".green(),
        latest
    )?;

    if check_only {
        let url = installer_url(&latest);
        writeln!(
            out,
            "run `qk update` (without --check) to install, or:\n  curl --proto '=https' --tlsv1.2 -LsSf {url} | sh"
        )?;
        return Ok(());
    }

    // The cargo-dist installer always writes to `~/.cargo/bin`. If the running
    // `qk` came from another package manager (Homebrew), that copy sits earlier
    // on PATH and shadows the freshly installed one — the update "succeeds" but
    // running `qk` still reports the old version. Detect this up front and point
    // the user at the manager that actually owns the binary, instead of silently
    // installing into a shadowed location.
    if let Some(mgr) = external_manager(current_exe_path().as_deref()) {
        bail!("{}", mgr.update_guidance());
    }

    if !assume_yes {
        if !crate::ui::stdin_is_interactive() {
            bail!("refusing to update without a TTY; pass --yes to override");
        }
        write!(out, "proceed with install? [y/N] ")?;
        out.flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            writeln!(out, "aborted.")?;
            return Ok(());
        }
    }

    run_installer(&latest).await?;
    writeln!(
        out,
        "{}",
        "done. Restart any running `quokka` to pick up the new version.".green()
    )?;
    Ok(())
}

async fn fetch_latest_tag() -> Result<String> {
    // `-f` makes curl exit non-zero on HTTP errors (otherwise rate-limited
    // 403s return a JSON error body we'd then fail to parse).
    // The User-Agent is required by api.github.com.
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "User-Agent: quokka-cli",
            "-H",
            "Accept: application/vnd.github+json",
            RELEASES_API,
        ])
        .output()
        .await
        .context("failed to invoke curl")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("GitHub API request failed: {}", stderr.trim());
    }

    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("GitHub API returned non-JSON body")?;
    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("GitHub API response missing tag_name"))
}

async fn run_installer(tag: &str) -> Result<()> {
    // Hand the pipe off to /bin/sh so we don't have to manage two
    // child processes and their fd plumbing from Rust. `pipefail` makes
    // a curl failure surface as a non-zero exit even though sh runs last.
    let url = installer_url(tag);
    let script =
        format!("set -o pipefail 2>/dev/null; curl --proto '=https' --tlsv1.2 -LsSf {url} | sh");
    let status = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .status()
        .await
        .context("failed to spawn installer pipeline")?;
    if !status.success() {
        bail!("installer pipeline failed (exit {status})");
    }
    Ok(())
}

/// Absolute path of the running executable, as a lossy string. `None` when the
/// platform can't report it — callers treat that as "no external manager
/// detected" so a missing path never blocks an otherwise-valid update.
fn current_exe_path() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// A package manager other than the cargo-dist installer that owns the running
/// binary. The self-updater can't safely install over these, so it defers to
/// them instead.
#[derive(Debug, PartialEq, Eq)]
enum ExternalManager {
    Homebrew,
}

impl ExternalManager {
    /// Actionable message explaining why the self-update was skipped and how to
    /// update through the owning manager instead.
    fn update_guidance(&self) -> String {
        match self {
            ExternalManager::Homebrew => "this `qk` was installed by Homebrew, which shadows the \
                 self-updater's `~/.cargo/bin` target on your PATH — installing here would have no \
                 effect. Update it with:\n  brew upgrade quokka-cli\nor remove the Homebrew copy \
                 first to let `qk update` manage it:\n  brew uninstall quokka-cli"
                .to_string(),
        }
    }
}

/// Detect whether the running binary is owned by an external package manager.
/// Homebrew installs land under a `/Cellar/` prefix (the `bin` symlinks in
/// `/opt/homebrew/bin` resolve there), so a path containing `/Cellar/` is the
/// reliable signal on both Apple-silicon (`/opt/homebrew`) and Intel
/// (`/usr/local`) layouts.
fn external_manager(exe_path: Option<&str>) -> Option<ExternalManager> {
    let path = exe_path?;
    if path.contains("/Cellar/") {
        Some(ExternalManager::Homebrew)
    } else {
        None
    }
}

/// True if `a` is a strictly higher semver than `b`. Treats anything that
/// fails to parse as "not newer" so a malformed remote tag never triggers
/// an unwanted reinstall.
fn is_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Option<(u32, u32, u32)> {
        let core = s.split('-').next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        Some((major, minor, patch))
    };
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{external_manager, installer_url, is_newer, ExternalManager};

    #[test]
    fn installer_url_pins_the_confirmed_tag_not_latest() {
        let url = installer_url("v0.3.0");
        assert!(url.contains("/download/v0.3.0/"), "must pin the tag: {url}");
        assert!(
            !url.contains("/latest/"),
            "must not use the latest alias: {url}"
        );
    }

    #[test]
    fn newer_compares_semver_components() {
        assert!(is_newer("0.2.2", "0.2.1"));
        assert!(is_newer("0.3.0", "0.2.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.1"));
    }

    #[test]
    fn homebrew_install_is_detected_as_external() {
        // Apple-silicon brew bin symlinks resolve into the Cellar.
        assert_eq!(
            external_manager(Some("/opt/homebrew/Cellar/quokka-cli/0.2.3/bin/qk")),
            Some(ExternalManager::Homebrew)
        );
        // Intel layout uses the same Cellar prefix under /usr/local.
        assert_eq!(
            external_manager(Some("/usr/local/Cellar/quokka-cli/0.2.3/bin/qk")),
            Some(ExternalManager::Homebrew)
        );
    }

    #[test]
    fn cargo_bin_install_is_not_external() {
        // The self-updater's own target must never be flagged, or `qk update`
        // would refuse to ever update a cargo-dist install.
        assert_eq!(external_manager(Some("/Users/x/.cargo/bin/qk")), None);
    }

    #[test]
    fn missing_exe_path_is_not_external() {
        // An unknowable path must not block an otherwise-valid update.
        assert_eq!(external_manager(None), None);
    }

    #[test]
    fn homebrew_guidance_names_the_brew_commands() {
        let msg = ExternalManager::Homebrew.update_guidance();
        assert!(msg.contains("brew upgrade quokka-cli"));
        assert!(msg.contains("brew uninstall quokka-cli"));
    }

    #[test]
    fn newer_strips_prerelease_suffix() {
        // Anything that fails to parse the core as X.Y.Z should not be "newer".
        assert!(!is_newer("not-a-version", "0.2.1"));
        // Prerelease tags compare on the core only — good enough for an
        // "is there something to install" check.
        assert!(is_newer("0.3.0-rc.1", "0.2.1"));
    }
}
