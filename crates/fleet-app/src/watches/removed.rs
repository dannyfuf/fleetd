use fleet_core::watches::WatchId;
use std::collections::BTreeMap;

/// Exact inclusive ranges of dismissed daemon-local IDs. No tombstone is forgotten:
/// arbitrarily late list/tail replies remain rejected within this daemon generation.
/// AppState replaces Watches on daemon restart, when numeric IDs may be reused.
#[derive(Debug, Default)]
pub(super) struct RemovedWatches(BTreeMap<u64, u64>);

impl RemovedWatches {
    pub fn contains(&self, id: &WatchId) -> bool {
        self.0
            .range(..=id.0)
            .next_back()
            .is_some_and(|(_, end)| id.0 <= *end)
    }

    pub fn insert(&mut self, id: WatchId) {
        let mut start = id.0;
        let mut end = id.0;
        if let Some((&previous, &previous_end)) = self.0.range(..=start).next_back() {
            if start <= previous_end {
                return;
            }
            if previous_end.checked_add(1) == Some(start) {
                start = previous;
                self.0.remove(&previous);
            }
        }
        if let Some((&next, &next_end)) = self.0.range(id.0..).next()
            && end.checked_add(1) == Some(next)
        {
            end = next_end;
            self.0.remove(&next);
        }
        self.0.insert(start, end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_preserves_gaps_and_merges_both_neighbors() {
        let mut removed = RemovedWatches::default();
        for id in [4, 1, 2, 6, 5, 2] {
            removed.insert(WatchId(id));
        }
        assert_eq!(removed.0.len(), 2);
        assert!(!removed.contains(&WatchId(3)));
        removed.insert(WatchId(3));
        assert_eq!(removed.0.len(), 1);
        for id in 1..=6 {
            assert!(removed.contains(&WatchId(id)));
        }
        assert!(!removed.contains(&WatchId(0)));
        assert!(!removed.contains(&WatchId(7)));
    }

    #[test]
    fn sequential_dismissals_keep_one_range_including_id_limits() {
        let mut removed = RemovedWatches::default();
        for id in 0..100_000 {
            removed.insert(WatchId(id));
        }
        assert_eq!(removed.0.len(), 1);
        for id in [u64::MAX, u64::MAX - 1] {
            removed.insert(WatchId(id));
        }
        assert!(removed.contains(&WatchId(u64::MAX)));
        assert!(!removed.contains(&WatchId(100_000)));
        assert_eq!(removed.0.len(), 2);
    }
}
