//! Pure analyze logic: file sorting, extension classification, and the
//! auto-mark heuristics. No device, no terminal — the `analyze` command's
//! `run`/TUI lives in `crate::commands::analyze` and consumes these.

use crate::device::MediaFile;

pub fn sort_by_size(mut files: Vec<MediaFile>) -> Vec<MediaFile> {
    files.sort_by_key(|f| std::cmp::Reverse(f.size_bytes));
    files
}

pub(crate) fn ext_lower(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

pub fn kind_from_ext(path: &str) -> &'static str {
    match ext_lower(path).as_str() {
        "mov" | "mp4" | "m4v" | "hevc" => "Video",
        "jpg" | "jpeg" | "heic" | "png" | "gif" => "Photo",
        "m4a" | "mp3" | "aac" | "wav" => "Audio",
        "pdf" | "epub" => "Doc",
        _ => "Other",
    }
}

pub mod heuristics {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    use super::{ext_lower, MediaFile};

    /// Edited variants on iPhone are stored as `IMG_E<digits>` next to the
    /// original `IMG_<digits>`. Used to detect the original/edited pair.
    const EDITED_PREFIX: &str = "IMG_E";
    const ORIGINAL_PREFIX: &str = "IMG_";
    const ONE_YEAR_SECS: i64 = 365 * 24 * 60 * 60;

    pub struct Match {
        pub label: &'static str,
        pub description: &'static str,
        pub enabled: bool,
        indices: Vec<usize>,
    }

    impl Match {
        pub fn count(&self) -> usize {
            self.indices.len()
        }

        pub fn indices(&self) -> &[usize] {
            &self.indices
        }

        pub fn bytes(&self, files: &[MediaFile]) -> u64 {
            self.indices.iter().map(|&i| files[i].size_bytes).sum()
        }
    }

    fn build(label: &'static str, description: &'static str, indices: Vec<usize>) -> Match {
        let enabled = !indices.is_empty();
        Match {
            label,
            description,
            enabled,
            indices,
        }
    }

    pub fn detect_all(files: &[MediaFile], now_unix: i64) -> Vec<Match> {
        vec![
            build(
                "Live Photo videos with photo sibling",
                ".MOV with matching .HEIC/.JPG — rarely watched",
                live_photo_motion(files),
            ),
            build(
                "Originals when edited version exists",
                "IMG_X kept when IMG_EX is in the same folder",
                originals_with_edited(files),
            ),
            build(
                "Old screenshots (> 1 year)",
                ".PNG in DCIM modified more than a year ago",
                old_screenshots(files, now_unix - ONE_YEAR_SECS),
            ),
            build(
                "Exact duplicates (name + size)",
                "Same filename and size in different folders",
                exact_duplicates(files),
            ),
        ]
    }

    fn parent_dir(path: &str) -> &str {
        Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("")
    }

    fn stem(path: &str) -> &str {
        Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    }

    fn basename(path: &str) -> &str {
        Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    }

    pub fn live_photo_motion(files: &[MediaFile]) -> Vec<usize> {
        let photo_keys: HashSet<(String, String)> = files
            .iter()
            .filter(|f| matches!(ext_lower(&f.path).as_str(), "heic" | "jpg" | "jpeg"))
            .map(|f| (parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            .collect();
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                ext_lower(&f.path) == "mov"
                    && photo_keys
                        .contains(&(parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn originals_with_edited(files: &[MediaFile]) -> Vec<usize> {
        let unedited_form: HashSet<(String, String)> = files
            .iter()
            .filter_map(|f| {
                stem(&f.path).strip_prefix(EDITED_PREFIX).map(|rest| {
                    (
                        parent_dir(&f.path).to_string(),
                        format!("{ORIGINAL_PREFIX}{rest}"),
                    )
                })
            })
            .collect();
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                unedited_form
                    .contains(&(parent_dir(&f.path).to_string(), stem(&f.path).to_string()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn old_screenshots(files: &[MediaFile], cutoff_unix: i64) -> Vec<usize> {
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                ext_lower(&f.path) == "png"
                    && f.path.starts_with("/DCIM/")
                    && f.modified_unix < cutoff_unix
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn exact_duplicates(files: &[MediaFile]) -> Vec<usize> {
        let mut groups: HashMap<(String, u64), Vec<usize>> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            groups
                .entry((basename(&f.path).to_string(), f.size_bytes))
                .or_default()
                .push(i);
        }
        let mut out: Vec<usize> = groups
            .into_values()
            .filter(|idxs| idxs.len() > 1)
            .flat_map(|idxs| idxs.into_iter().skip(1))
            .collect();
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mf(path: &str, size: u64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix: 0,
        }
    }

    fn mf_at(path: &str, size: u64, modified_unix: i64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes: size,
            modified_unix,
        }
    }

    #[test]
    fn heuristics_live_photo_motion_pairs_mov_with_photo() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_0001.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_0001.MOV", 3_000_000),
            mf("/DCIM/100APPLE/IMG_0002.MOV", 500_000_000),
            mf("/DCIM/100APPLE/IMG_0003.JPG", 2_000_000),
            mf("/DCIM/100APPLE/IMG_0003.MOV", 1_500_000),
        ];
        let hits = heuristics::live_photo_motion(&files);
        assert_eq!(hits, vec![1, 4]);
    }

    #[test]
    fn heuristics_originals_with_edited_marks_originals() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_1234.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_E1234.HEIC", 4_500_000),
            mf("/DCIM/100APPLE/IMG_5555.HEIC", 1_000_000),
        ];
        let hits = heuristics::originals_with_edited(&files);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn heuristics_old_screenshots_respects_age_and_extension() {
        let files = vec![
            mf_at("/DCIM/100APPLE/IMG_0001.PNG", 1_000_000, 100),
            mf_at("/DCIM/100APPLE/IMG_0002.PNG", 1_000_000, 9999),
            mf_at("/DCIM/100APPLE/IMG_0003.HEIC", 1_000_000, 100),
            mf_at("/Downloads/foo.PNG", 1_000_000, 100),
        ];
        let hits = heuristics::old_screenshots(&files, 1000);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn heuristics_exact_duplicates_marks_extra_copies() {
        let files = vec![
            mf("/DCIM/100APPLE/IMG_0001.HEIC", 4_000_000),
            mf("/Downloads/IMG_0001.HEIC", 4_000_000),
            mf("/DCIM/100APPLE/IMG_0001.HEIC.bak", 4_000_000),
            mf("/DCIM/100APPLE/UNIQUE.HEIC", 1_000_000),
        ];
        let hits = heuristics::exact_duplicates(&files);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn kind_from_ext_classifies_known_extensions() {
        assert_eq!(kind_from_ext("/DCIM/IMG.MOV"), "Video");
        assert_eq!(kind_from_ext("/DCIM/IMG.mp4"), "Video");
        assert_eq!(kind_from_ext("/DCIM/IMG.HEIC"), "Photo");
        assert_eq!(kind_from_ext("/DCIM/IMG.jpg"), "Photo");
        assert_eq!(kind_from_ext("/Recordings/m.m4a"), "Audio");
        assert_eq!(kind_from_ext("/Downloads/x.pdf"), "Doc");
        assert_eq!(kind_from_ext("/Books/x.EPUB"), "Doc");
    }

    #[test]
    fn kind_from_ext_falls_back_to_other() {
        assert_eq!(kind_from_ext("/Downloads/NOEXT"), "Other");
        assert_eq!(kind_from_ext("/x.weird"), "Other");
        assert_eq!(kind_from_ext(""), "Other");
    }
}
