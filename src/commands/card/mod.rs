//! `quokka card` — render a 1080×1080 PNG snapshot of the connected iPhone.
//!
//! Pipeline: device.status() → CardData (pure projection) → render_svg
//! (pure) → svg_to_png (resvg) → write to disk → print share URL → open in
//! Preview (unless `--no-open`).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::device::Device;
use crate::ui::{civil_from_unix, spinner};

pub mod badges;
pub mod data;
pub mod emoji;
pub mod png;
pub mod render;
pub mod share;

/// CLI arguments wired from clap in `src/lib.rs`.
#[derive(Debug, Clone)]
pub struct CardArgs {
    /// Where to write the PNG. `None` → `~/Desktop/qk-card-<ts>.png`.
    pub output: Option<PathBuf>,
    /// Skip the `open <path>` call (always skipped off macOS too).
    pub no_open: bool,
    /// Mask anything potentially personal (build number, exact dates,
    /// oldest-app name).
    pub redact: bool,
}

pub async fn run(device: &dyn Device, now_unix: i64, args: CardArgs) -> Result<()> {
    // Spinner stays up across the whole collect — the slow phase is
    // `with_dynamic_sizes` (cache+downloads sizing of the top 10 apps).
    // Up to ~5s on iOS 26 in practice.
    let bar = spinner("Reading device info & sizing apps...");
    let card_data = data::collect(device, now_unix, args.redact).await;
    bar.finish_and_clear();
    let card_data = card_data?;

    let svg = render::render_svg(&card_data);
    let png_bytes = match png::svg_to_png(&svg) {
        Ok(b) => b,
        Err(e) => {
            // Persist the generated SVG so the user (or Lucas) can attach
            // it to a bug report. Don't fail this fallback path silently.
            let dump = std::env::temp_dir().join(format!("qk-card-{}.svg", now_unix));
            if let Err(write_err) = std::fs::write(&dump, &svg) {
                eprintln!("(also failed to write SVG dump: {write_err})");
            } else {
                eprintln!(
                    "{}: render failed — SVG dumped to {}",
                    "error".red(),
                    dump.display()
                );
                eprintln!(
                    "Please file an issue at https://github.com/dutradotdev/quokka/issues and attach the SVG."
                );
            }
            return Err(e);
        }
    };

    let output_path = match args.output {
        Some(p) => p,
        None => default_output_path(now_unix)?,
    };
    write_png(&output_path, &png_bytes)?;

    print_success(&output_path)?;

    if !args.no_open && cfg!(target_os = "macos") {
        let _ = open_in_preview(&output_path);
    }

    // Growth hook — if stdin is a TTY, offer a 1-keypress shortcut to
    // open the repo and star it. Non-TTY (piped, CI) silently skips.
    let _ = prompt_for_star();

    Ok(())
}

const REPO_URL: &str = "https://github.com/dutradotdev/quokka";

/// Print the prompt and wait for a single keypress (no Enter required).
/// On `S`/`s` → open the repo URL in the default browser. Any other key
/// (including Enter, Esc, Q) exits cleanly. Skipped entirely if stdin
/// isn't a terminal.
fn prompt_for_star() -> Result<()> {
    if !crate::ui::stdin_is_interactive() {
        return Ok(());
    }

    use crossterm::event::{self, Event, KeyCode};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use owo_colors::OwoColorize;

    let mut out = anstream::stdout();
    writeln!(out)?;
    writeln!(
        out,
        "  {} {} {} {}",
        "🌟".bright_yellow(),
        "like it? press".dimmed(),
        "S".bold(),
        "to star the repo · any other key to go back to the menu".dimmed(),
    )?;
    out.flush()?;

    enable_raw_mode()?;
    let pressed_s = loop {
        match event::read() {
            Ok(Event::Key(k)) => match k.code {
                KeyCode::Char('s') | KeyCode::Char('S') => break true,
                _ => break false,
            },
            // Anything that isn't a key event (resize, paste, focus): keep
            // waiting. A read error breaks out as "no star" so we don't
            // hang the terminal in raw mode.
            Ok(_) => continue,
            Err(_) => break false,
        }
    };
    disable_raw_mode()?;

    if pressed_s {
        // `open` is macOS-only; on other platforms print the URL so the
        // user can paste it themselves rather than fail silently.
        if cfg!(target_os = "macos") {
            let _ = std::process::Command::new("open").arg(REPO_URL).status();
            writeln!(out, "  thanks! ⭐  opened {REPO_URL} in your browser")?;
        } else {
            writeln!(out, "  thanks! ⭐  {REPO_URL}")?;
        }
    }
    Ok(())
}

fn default_output_path(now_unix: i64) -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .context("$HOME is not set — pass an explicit `--output PATH` for the PNG location")?;
    let (y, mo, d, h, mi, _) = civil_from_unix(now_unix);
    let stamp = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}");
    Ok(PathBuf::from(home)
        .join("Desktop")
        .join(format!("qk-card-{stamp}.png")))
}

fn write_png(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("write PNG to {}", path.display()))
}

fn print_success(path: &Path) -> Result<()> {
    let mut out = anstream::stdout();
    writeln!(out, "{} saved to {}", "✓".green(), path.display())?;
    writeln!(out)?;
    writeln!(out, "share it:")?;
    writeln!(out, "  {}", share::tweet_intent_url().dimmed())?;
    Ok(())
}

fn open_in_preview(path: &Path) -> Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .status()
        .with_context(|| format!("spawn `open {}`", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_path_is_desktop_qk_card_with_timestamp() {
        // 2024-05-28 00:00:00 UTC
        let path = default_output_path(1_716_854_400).expect("$HOME should be set in tests");
        let s = path.to_string_lossy();
        assert!(s.contains("Desktop"), "got `{s}`");
        assert!(s.contains("qk-card-20240528-"), "got `{s}`");
        assert!(s.ends_with(".png"), "got `{s}`");
    }
}
