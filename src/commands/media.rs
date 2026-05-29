//! `quokka media` — read-only survey of the AFC media area.

use std::io::Write;

use anyhow::Result;

use crate::commands::analyze::{ext_lower, kind_from_ext};
use crate::device::{Device, MediaFile, WalkCallback, WalkProgress};
use crate::ui::{format_bytes, spinner};

const MEDIA_ROOTS: &[&str] = &["/DCIM", "/Downloads", "/Recordings", "/Books"];
const MEDIA_ROOTS_LABEL: &str = "DCIM, Recordings, Books, Downloads";
const BUCKET_BAR_WIDTH: usize = 10;
const MONTHS_SHOWN: usize = 12;
const TOP_LARGEST: usize = 10;
const TOP_DUPLICATES: usize = 10;

pub async fn run(device: &dyn Device, find_duplicates: bool) -> Result<()> {
    let files = collect_media(device).await?;
    let now_unix = crate::ui::now_unix();
    let report = build_report(&files, find_duplicates, now_unix, None);
    let mut out = anstream::stdout();
    write!(out, "{}", render(&report))?;
    Ok(())
}

async fn collect_media(device: &dyn Device) -> Result<Vec<MediaFile>> {
    let bar = spinner("Walking media files...");
    let bar_for_cb = bar.clone();
    let on_progress: WalkCallback = Box::new(move |p: WalkProgress| {
        bar_for_cb.set_message(format!(
            "Walking media files... {} files, {}",
            p.files_seen,
            format_bytes(p.bytes_seen)
        ));
    });
    let result = device.afc_walk(MEDIA_ROOTS, on_progress).await;
    bar.finish_and_clear();
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Photo,
    Video,
    Audio,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Photo => "Photos",
            Kind::Video => "Videos",
            Kind::Audio => "Audio",
            Kind::Other => "Other",
        }
    }

    pub fn from_path(path: &str) -> Self {
        match ext_lower(path).as_str() {
            "mov" | "mp4" | "m4v" | "hevc" => Kind::Video,
            "jpg" | "jpeg" | "heic" | "png" | "gif" => Kind::Photo,
            "m4a" | "mp3" | "aac" | "wav" => Kind::Audio,
            _ => Kind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct YearMonth {
    pub year: i32,
    pub month: u32,
}

impl std::fmt::Display for YearMonth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// Convert unix epoch seconds to (year, month) in UTC. Pure math —
/// no chrono needed.
fn epoch_to_year_month(unix_seconds: i64) -> YearMonth {
    let (year, month, _, _, _, _) = crate::ui::civil_from_unix(unix_seconds);
    YearMonth { year, month }
}

#[derive(Debug, Clone)]
pub struct MediaReport {
    pub total_files: usize,
    pub total_bytes: u64,
    pub device_name: Option<String>,
    pub by_kind: [(Kind, usize, u64); 4],
    pub by_month: Vec<(YearMonth, usize, u64)>,
    pub unknown_month: Option<(usize, u64)>,
    pub largest: Vec<MediaFile>,
    pub duplicates: Option<DuplicateReport>,
}

#[derive(Debug, Clone)]
pub struct DuplicateReport {
    pub group_count: usize,
    pub file_count: usize,
    pub potential_savings_bytes: u64,
    pub top_groups: Vec<DuplicateGroup>,
}

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub size_bytes: u64,
    pub kind: Kind,
    pub paths: Vec<String>,
}

pub fn build_report(
    files: &[MediaFile],
    find_duplicates: bool,
    now_unix: i64,
    device_name: Option<String>,
) -> MediaReport {
    let total_files = files.len();
    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    let by_kind = classify_by_kind(files);
    let (by_month, unknown_month) = bucket_by_month(files, now_unix);
    let largest = super::top_n_by_size(files, TOP_LARGEST);
    let duplicates = if find_duplicates {
        Some(find_duplicate_groups(files, TOP_DUPLICATES))
    } else {
        None
    };
    MediaReport {
        total_files,
        total_bytes,
        device_name,
        by_kind,
        by_month,
        unknown_month,
        largest,
        duplicates,
    }
}

pub fn classify_by_kind(files: &[MediaFile]) -> [(Kind, usize, u64); 4] {
    let mut counts = [
        (Kind::Photo, 0usize, 0u64),
        (Kind::Video, 0usize, 0u64),
        (Kind::Audio, 0usize, 0u64),
        (Kind::Other, 0usize, 0u64),
    ];
    for f in files {
        let k = Kind::from_path(&f.path);
        let idx = counts.iter().position(|(kk, _, _)| *kk == k).unwrap();
        counts[idx].1 += 1;
        counts[idx].2 += f.size_bytes;
    }
    counts
}

pub type MonthBuckets = (Vec<(YearMonth, usize, u64)>, Option<(usize, u64)>);

pub fn bucket_by_month(files: &[MediaFile], now_unix: i64) -> MonthBuckets {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<YearMonth, (usize, u64)> = BTreeMap::new();
    let mut unknown = (0usize, 0u64);
    for f in files {
        if f.modified_unix == 0 {
            unknown.0 += 1;
            unknown.1 += f.size_bytes;
            continue;
        }
        let ym = epoch_to_year_month(f.modified_unix);
        let slot = buckets.entry(ym).or_insert((0, 0));
        slot.0 += 1;
        slot.1 += f.size_bytes;
    }
    let now_ym = epoch_to_year_month(now_unix);
    let mut window: Vec<YearMonth> = Vec::with_capacity(MONTHS_SHOWN);
    let mut ym = now_ym;
    for _ in 0..MONTHS_SHOWN {
        window.push(ym);
        ym = previous_month(ym);
    }
    let mut by_month: Vec<(YearMonth, usize, u64)> = window
        .into_iter()
        .filter_map(|ym| buckets.get(&ym).map(|(c, b)| (ym, *c, *b)))
        .collect();
    by_month.sort_by_key(|b| std::cmp::Reverse(b.0));
    let unknown_opt = if unknown.0 == 0 { None } else { Some(unknown) };
    (by_month, unknown_opt)
}

fn previous_month(ym: YearMonth) -> YearMonth {
    if ym.month == 1 {
        YearMonth {
            year: ym.year - 1,
            month: 12,
        }
    } else {
        YearMonth {
            year: ym.year,
            month: ym.month - 1,
        }
    }
}

pub fn find_duplicate_groups(files: &[MediaFile], top_n: usize) -> DuplicateReport {
    use std::collections::HashMap;
    let mut groups: HashMap<(u64, Kind), Vec<String>> = HashMap::new();
    for f in files {
        let k = Kind::from_path(&f.path);
        groups
            .entry((f.size_bytes, k))
            .or_default()
            .push(f.path.clone());
    }
    let mut dup_groups: Vec<DuplicateGroup> = groups
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|((size, kind), paths)| DuplicateGroup {
            size_bytes: size,
            kind,
            paths,
        })
        .collect();
    let group_count = dup_groups.len();
    let file_count: usize = dup_groups.iter().map(|g| g.paths.len()).sum();
    let potential_savings_bytes: u64 = dup_groups
        .iter()
        .map(|g| g.size_bytes * (g.paths.len() as u64 - 1))
        .sum();
    dup_groups.sort_by_key(|g| std::cmp::Reverse(g.size_bytes * (g.paths.len() as u64 - 1)));
    dup_groups.truncate(top_n);
    DuplicateReport {
        group_count,
        file_count,
        potential_savings_bytes,
        top_groups: dup_groups,
    }
}

pub fn render(report: &MediaReport) -> String {
    if report.total_files == 0 {
        return format!("No files in {MEDIA_ROOTS_LABEL}.\n");
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
        "Scanned {} files ({}) under {MEDIA_ROOTS_LABEL}\n\n",
        report.total_files,
        format_bytes(report.total_bytes)
    ));

    // By kind
    out.push_str("By kind\n");
    let max_kind_bytes = report.by_kind.iter().map(|(_, _, b)| *b).max().unwrap_or(0);
    for (kind, count, bytes) in &report.by_kind {
        let bar = if max_kind_bytes == 0 {
            "░".repeat(BUCKET_BAR_WIDTH)
        } else {
            let pct = ((*bytes as f64 / max_kind_bytes as f64) * BUCKET_BAR_WIDTH as f64) as usize;
            let pct = pct.min(BUCKET_BAR_WIDTH);
            let mut s = String::new();
            for _ in 0..pct {
                s.push('█');
            }
            for _ in pct..BUCKET_BAR_WIDTH {
                s.push('░');
            }
            s
        };
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

    fn mf(path: &str, size: u64, mtime: i64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix: mtime,
        }
    }

    #[test]
    fn classify_by_kind_counts_per_bucket() {
        let files = vec![
            mf("/DCIM/a.HEIC", 100, 0),
            mf("/DCIM/b.heic", 200, 0),
            mf("/DCIM/c.MOV", 1000, 0),
            mf("/Recordings/d.m4a", 50, 0),
            mf("/Downloads/e.pdf", 10, 0),
            mf("/Downloads/no_ext", 5, 0),
        ];
        let counts = classify_by_kind(&files);
        // Photo, Video, Audio, Other order
        assert_eq!(counts[0].1, 2);
        assert_eq!(counts[0].2, 300);
        assert_eq!(counts[1].1, 1);
        assert_eq!(counts[1].2, 1000);
        assert_eq!(counts[2].1, 1);
        assert_eq!(counts[2].2, 50);
        assert_eq!(counts[3].1, 2);
        assert_eq!(counts[3].2, 15);
    }

    #[test]
    fn bucket_by_month_returns_unknown_when_mtime_zero() {
        let files = vec![mf("/a", 100, 0), mf("/b", 200, 1_700_000_000)];
        let (by_month, unknown) = bucket_by_month(&files, 1_700_000_000);
        assert_eq!(by_month.len(), 1);
        assert_eq!(unknown, Some((1, 100)));
    }

    #[test]
    fn bucket_by_month_excludes_unknown_when_none() {
        let files = vec![mf("/a", 100, 1_700_000_000)];
        let (_, unknown) = bucket_by_month(&files, 1_700_000_000);
        assert!(unknown.is_none());
    }

    #[test]
    fn bucket_by_month_window_drops_files_older_than_12_months() {
        // `now` = 2026-05-15. A file from 2024-01 is 16 months back — outside
        // the 12-month window the dashboard shows.
        let now = 1_747_310_400; // 2025-05-15
        let in_window = 1_737_244_800; // 2025-01-19
        let too_old = 1_705_708_800; // 2024-01-20
        let files = vec![mf("/a", 100, in_window), mf("/b", 200, too_old)];
        let (by_month, unknown) = bucket_by_month(&files, now);
        assert!(unknown.is_none());
        assert_eq!(by_month.len(), 1, "file older than 12 months must be cut");
        assert_eq!(by_month[0].2, 100);
    }

    #[test]
    fn bucket_by_month_wraps_year_when_now_is_january() {
        // now = 2026-01-15; window should reach back into 2025-02, including
        // a December 2025 bucket. A regression in previous_month (e.g. not
        // decrementing year when month == 1) would drop these.
        let now = 1_768_521_600; // 2026-01-16
        let dec_2025 = 1_765_843_200; // 2025-12-16
        let nov_2025 = 1_763_251_200; // 2025-11-16
        let files = vec![mf("/a", 10, dec_2025), mf("/b", 20, nov_2025)];
        let (by_month, _) = bucket_by_month(&files, now);
        assert_eq!(by_month.len(), 2);
        // Sorted by YearMonth descending.
        assert_eq!(by_month[0].0.year, 2025);
        assert_eq!(by_month[0].0.month, 12);
        assert_eq!(by_month[1].0.year, 2025);
        assert_eq!(by_month[1].0.month, 11);
    }

    #[test]
    fn epoch_to_year_month_matches_known_dates() {
        // 2023-11-14 22:13:20 UTC = 1_700_000_000
        let ym = epoch_to_year_month(1_700_000_000);
        assert_eq!(ym.year, 2023);
        assert_eq!(ym.month, 11);
        // 1970-01-01
        let ym = epoch_to_year_month(0);
        assert_eq!(ym.year, 1970);
        assert_eq!(ym.month, 1);
    }

    #[test]
    fn find_duplicate_groups_does_not_merge_across_kinds() {
        // CRITICAL safety invariant: a photo and a video of the same size
        // must NOT show up as a single dup group. The picker offers to
        // delete extras of the group's first member — a regression here
        // would suggest the user delete unrelated files.
        let files = vec![
            mf("/DCIM/photo.HEIC", 1000, 0),
            mf("/DCIM/video.MOV", 1000, 0),
        ];
        let d = find_duplicate_groups(&files, 10);
        assert_eq!(
            d.group_count, 0,
            "different kinds with same size must not collapse into one group"
        );
        assert_eq!(d.file_count, 0);
        assert_eq!(d.potential_savings_bytes, 0);
    }

    #[test]
    fn find_duplicate_groups_top_n_keeps_biggest_savings_first() {
        // 3 photo duplicates of 1KB → savings = 2KB (2 extras × 1KB).
        // 2 photo duplicates of 10KB → savings = 10KB (1 extra × 10KB).
        // Both groups exist but with top_n = 1 we should keep the 10KB one.
        let files = vec![
            mf("/a1.HEIC", 1000, 0),
            mf("/a2.HEIC", 1000, 0),
            mf("/a3.HEIC", 1000, 0),
            mf("/b1.HEIC", 10_000, 0),
            mf("/b2.HEIC", 10_000, 0),
        ];
        let d = find_duplicate_groups(&files, 1);
        assert_eq!(d.group_count, 2, "group_count counts all, not just top_n");
        assert_eq!(d.top_groups.len(), 1);
        assert_eq!(d.top_groups[0].size_bytes, 10_000);
    }

    #[test]
    fn find_duplicate_groups_single_file_is_not_a_group() {
        let files = vec![mf("/lonely.HEIC", 999, 0)];
        let d = find_duplicate_groups(&files, 10);
        assert_eq!(d.group_count, 0);
        assert_eq!(d.file_count, 0);
        assert_eq!(d.potential_savings_bytes, 0);
    }

    #[test]
    fn find_duplicate_groups_aggregates_savings() {
        let files = vec![
            mf("/DCIM/a.HEIC", 100, 0),
            mf("/DCIM/b.HEIC", 100, 0),
            mf("/Downloads/c.HEIC", 100, 0),
            mf("/DCIM/d.MOV", 200, 0),
            mf("/Downloads/e.MOV", 200, 0),
            mf("/DCIM/unique.HEIC", 999, 0),
        ];
        let d = find_duplicate_groups(&files, 10);
        assert_eq!(d.group_count, 2);
        assert_eq!(d.file_count, 5);
        assert_eq!(d.potential_savings_bytes, 100 * 2 + 200);
    }

    #[test]
    fn render_with_no_files_prints_short_line() {
        let report = build_report(&[], false, 1_700_000_000, None);
        let out = render(&report);
        assert!(out.contains("No files"));
        assert!(out.contains("DCIM"));
    }

    #[test]
    fn render_omits_duplicates_when_not_requested() {
        let files = vec![mf("/DCIM/a.HEIC", 100, 1_700_000_000)];
        let report = build_report(&files, false, 1_700_000_000, None);
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
        let report = build_report(&files, true, 1_700_000_000, None);
        let out = render(&report);
        assert!(out.contains("Likely duplicates"));
    }
}
