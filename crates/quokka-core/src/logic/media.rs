//! Pure media-survey logic: kind classification, per-month bucketing,
//! duplicate detection, and the `MediaReport` DTO. No device, no terminal —
//! the `media` command's `run`/render lives in `crate::commands::media` and
//! consumes these.

use crate::device::MediaFile;
use crate::logic::analyze::ext_lower;

const MONTHS_SHOWN: usize = 12;
const TOP_LARGEST: usize = 10;
const TOP_DUPLICATES: usize = 10;

/// Human-readable label for a set of AFC roots: each root's basename joined
/// with commas (e.g. `"DCIM, Downloads, Recordings, Books"`). Derived from the
/// roots so the label tracks whatever paths the active device exposes.
fn roots_label(roots: &[&str]) -> String {
    roots
        .iter()
        .map(|r| r.trim_start_matches('/'))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
    let (year, month, _, _, _, _) = crate::fmt::civil_from_unix(unix_seconds);
    YearMonth { year, month }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaReport {
    pub total_files: usize,
    pub total_bytes: u64,
    pub device_name: Option<String>,
    /// Display label for the roots that were walked, derived from them. The
    /// report owns it so the CLI's `media` renderer stays free of platform
    /// path assumptions.
    pub roots_label: String,
    pub by_kind: [(Kind, usize, u64); 4],
    pub by_month: Vec<(YearMonth, usize, u64)>,
    pub unknown_month: Option<(usize, u64)>,
    pub largest: Vec<MediaFile>,
    pub duplicates: Option<DuplicateReport>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReport {
    pub group_count: usize,
    pub file_count: usize,
    pub potential_savings_bytes: u64,
    pub top_groups: Vec<DuplicateGroup>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
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
    roots: &[&str],
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
        roots_label: roots_label(roots),
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ROOTS: &[&str] = &["/DCIM", "/Downloads", "/Recordings", "/Books"];

    #[test]
    fn roots_label_strips_slash_and_joins() {
        assert_eq!(
            roots_label(TEST_ROOTS),
            "DCIM, Downloads, Recordings, Books"
        );
        assert_eq!(roots_label(&["/sdcard/DCIM"]), "sdcard/DCIM");
        assert_eq!(roots_label(&[]), "");
    }

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
}
