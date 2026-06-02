//! `quokka media` — read-only survey of the AFC media area.

use std::io::Write;

use anyhow::Result;

use crate::device::{Device, MediaFile, WalkCallback, WalkProgress};
use crate::logic::analyze::kind_from_ext;
use crate::ui::{format_bytes, spinner};

// The report DTO and its pure builders (`build_report`, `classify_by_kind`,
// `bucket_by_month`, `find_duplicate_groups`, `Kind`, …) live in the core
// (`crate::logic::media`). Re-export them at the historical
// `commands::media::*` paths so `report`/`render` and tests keep resolving.
pub use crate::logic::media::*;

const BUCKET_BAR_WIDTH: usize = 10;

pub async fn run(device: &dyn Device, find_duplicates: bool) -> Result<()> {
    let bar = spinner("Walking media files...");
    let bar_for_cb = bar.clone();
    let on_progress: WalkCallback = Box::new(move |p: WalkProgress| {
        bar_for_cb.set_message(format!(
            "Walking media files... {} files, {}",
            p.files_seen,
            format_bytes(p.bytes_seen)
        ));
    });
    let report = crate::app::media(device, find_duplicates, on_progress).await;
    bar.finish_and_clear();
    let mut out = anstream::stdout();
    write!(out, "{}", render(&report?))?;
    Ok(())
}

/// Build and render the survey report for already-walked `files`. Shared by
/// [`run`] (which walks first with a spinner) and the sidebar launcher (which
/// walks inline with progress, then hands the files here).
pub fn report(files: &[MediaFile], find_duplicates: bool, roots: &[&str]) -> String {
    let report = build_report(files, find_duplicates, crate::ui::now_unix(), None, roots);
    render(&report)
}

pub fn render(report: &MediaReport) -> String {
    if report.total_files == 0 {
        return format!("No files in {}.\n", report.roots_label);
    }
    let mut out = String::new();
    let header_name = report
        .device_name
        .as_deref()
        .map(|n| format!("Media on {n}"))
        .unwrap_or_else(|| "Media".to_string());
    out.push_str(&header_name);
    out.push('\n');
    out.push_str(&format!(
        "Scanned {} files ({}) under {}\n\n",
        report.total_files,
        format_bytes(report.total_bytes),
        report.roots_label
    ));

    // By kind
    out.push_str("By kind\n");
    let max_kind_bytes = report.by_kind.iter().map(|(_, _, b)| *b).max().unwrap_or(0);
    for (kind, count, bytes) in &report.by_kind {
        let bar = bar_for(*bytes, max_kind_bytes, BUCKET_BAR_WIDTH);
        out.push_str(&format!(
            "  {:<10} {bar}  {:>6} files   ·   {:>8}\n",
            kind.label(),
            count,
            format_bytes(*bytes),
        ));
    }
    out.push('\n');

    // By month
    out.push_str("By month (last 12)\n");
    if report.by_month.is_empty() && report.unknown_month.is_none() {
        out.push_str("  no files\n");
    } else {
        let max_bytes = report
            .by_month
            .iter()
            .map(|(_, _, b)| *b)
            .max()
            .unwrap_or(0);
        for (ym, count, bytes) in &report.by_month {
            let bar = bar_for(*bytes, max_bytes, BUCKET_BAR_WIDTH);
            out.push_str(&format!(
                "  {ym}    {count} files   ·   {}  {bar}\n",
                format_bytes(*bytes)
            ));
        }
        if let Some((count, bytes)) = report.unknown_month {
            out.push_str(&format!(
                "  Unknown    {count} files   ·   {}\n",
                format_bytes(bytes)
            ));
        }
    }
    out.push('\n');

    // Largest
    out.push_str("Largest 10\n");
    if report.largest.is_empty() {
        out.push_str("  no files\n");
    } else {
        for f in &report.largest {
            out.push_str(&format!(
                "  {:>8}  {:<6}  {}\n",
                format_bytes(f.size_bytes),
                kind_from_ext(&f.path),
                f.path,
            ));
        }
    }

    if let Some(d) = &report.duplicates {
        out.push('\n');
        out.push_str(
            "Likely duplicates  (exact size match — heuristic, may include false positives)\n",
        );
        out.push_str(&format!(
            "  {} groups, {} files, {} potential savings\n",
            d.group_count,
            d.file_count,
            format_bytes(d.potential_savings_bytes)
        ));
        for g in &d.top_groups {
            let first = g.paths.first().map(String::as_str).unwrap_or("");
            let extras = g.paths.len().saturating_sub(1);
            let extras_label = if extras == 1 {
                "1 other".to_string()
            } else {
                format!("{extras} others")
            };
            out.push_str(&format!(
                "    {:>8}  {:<6}  × {}  {} + {}\n",
                format_bytes(g.size_bytes),
                g.kind.label(),
                g.paths.len(),
                first,
                extras_label,
            ));
        }
    }

    out
}

fn bar_for(value: u64, max: u64, width: usize) -> String {
    if max == 0 {
        return "░".repeat(width);
    }
    let pct = ((value as f64 / max as f64) * width as f64) as usize;
    let pct = pct.min(width);
    let mut s = String::new();
    for _ in 0..pct {
        s.push('█');
    }
    for _ in pct..width {
        s.push('░');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ROOTS: &[&str] = &["/DCIM", "/Downloads", "/Recordings", "/Books"];

    fn mf(path: &str, size: u64, mtime: i64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix: mtime,
        }
    }

    #[test]
    fn render_with_no_files_prints_short_line() {
        let report = build_report(&[], false, 1_700_000_000, None, TEST_ROOTS);
        let out = render(&report);
        assert!(out.contains("No files"));
        assert!(out.contains("DCIM"));
    }

    #[test]
    fn render_omits_duplicates_when_not_requested() {
        let files = vec![mf("/DCIM/a.HEIC", 100, 1_700_000_000)];
        let report = build_report(&files, false, 1_700_000_000, None, TEST_ROOTS);
        let out = render(&report);
        assert!(!out.contains("Likely duplicates"));
        assert!(out.contains("By kind"));
        assert!(out.contains("Largest 10"));
    }

    #[test]
    fn render_includes_duplicates_section_when_present() {
        let files = vec![
            mf("/DCIM/a.HEIC", 100, 1_700_000_000),
            mf("/Downloads/b.HEIC", 100, 1_700_000_000),
        ];
        let report = build_report(&files, true, 1_700_000_000, None, TEST_ROOTS);
        let out = render(&report);
        assert!(out.contains("Likely duplicates"));
    }
}
