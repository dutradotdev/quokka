pub mod analyze;
pub mod apps;
pub mod capture;
pub mod card;
pub mod dashboard;
pub mod devices;
pub mod info;
pub mod logs;
pub mod media;
pub mod menu;
pub mod power;
pub mod status;
pub mod update;

use crate::device::MediaFile;

/// Top `n` media files by size, largest first. Uses a bounded min-heap so it
/// runs in O(N log n) instead of a full O(N log N) sort — both the read-only
/// `analyze` view and the `media` report only ever surface a handful out of
/// libraries that can hold tens of thousands of files. Ties on size keep the
/// lower-indexed file first, so the order is deterministic.
pub fn top_n_by_size(files: &[MediaFile], n: usize) -> Vec<MediaFile> {
    if n == 0 {
        return Vec::new();
    }
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    // A max-heap of `Reverse(key)` behaves as a min-heap on `key`, so `pop()`
    // discards the current worst candidate once we exceed `n`. "Worst" is the
    // smallest size; ties break toward the larger index (`Reverse(index)`) so
    // the survivors keep the earliest files.
    let mut heap: BinaryHeap<Reverse<(u64, Reverse<usize>)>> = BinaryHeap::with_capacity(n + 1);
    for (index, file) in files.iter().enumerate() {
        heap.push(Reverse((file.size_bytes, Reverse(index))));
        if heap.len() > n {
            heap.pop();
        }
    }
    let mut survivors: Vec<(u64, usize)> = heap
        .into_iter()
        .map(|Reverse((size, Reverse(index)))| (size, index))
        .collect();
    // Largest size first; ties by ascending original index for stable output.
    survivors.sort_by_key(|&(size, index)| (Reverse(size), index));
    survivors
        .into_iter()
        .map(|(_, index)| files[index].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::top_n_by_size;
    use crate::device::MediaFile;

    fn mf(path: &str, size_bytes: u64) -> MediaFile {
        MediaFile {
            path: path.into(),
            size_bytes,
            modified_unix: 0,
        }
    }

    #[test]
    fn returns_largest_first() {
        let files = vec![mf("/a", 10), mf("/b", 100), mf("/c", 50), mf("/d", 1000)];
        let top = top_n_by_size(&files, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].path, "/d");
        assert_eq!(top[1].path, "/b");
    }

    #[test]
    fn caps_at_n_and_handles_zero() {
        let files = vec![mf("/a", 10), mf("/b", 100)];
        assert_eq!(top_n_by_size(&files, 50).len(), 2);
        assert!(top_n_by_size(&files, 0).is_empty());
    }

    #[test]
    fn breaks_ties_by_original_order() {
        // Three files tie at 100 bytes; the two kept must be the earliest.
        let files = vec![mf("/first", 100), mf("/second", 100), mf("/third", 100)];
        let top = top_n_by_size(&files, 2);
        assert_eq!(top[0].path, "/first");
        assert_eq!(top[1].path, "/second");
    }
}
