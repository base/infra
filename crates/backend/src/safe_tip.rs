//! Byzantine-safe tip tracker.

use std::collections::{BTreeMap, HashMap};

use alloy_primitives::BlockNumber;

/// Byzantine-safe tip tracker.
///
/// Maintains two heaps to track the f+1 safe tip across backends.
#[derive(Debug)]
pub struct SafeTip {
    tips: HashMap<String, BlockNumber>,
    hi: BTreeMap<BlockNumber, usize>,
    lo: BTreeMap<BlockNumber, usize>,
    f: usize,
}

impl SafeTip {
    /// Create a new SafeTip tracker.
    ///
    /// # Arguments
    ///
    /// * `f` - Maximum number of Byzantine faulty backends.
    #[must_use]
    pub fn new(f: usize) -> Self {
        Self { tips: HashMap::new(), hi: BTreeMap::new(), lo: BTreeMap::new(), f }
    }

    /// Update a backend's reported block height.
    pub fn update(&mut self, backend: &str, height: BlockNumber) {
        // Remove old tip if exists
        if let Some(old_height) = self.tips.get(backend) {
            self.remove_from_heaps(*old_height);
        }

        // Insert new tip
        self.tips.insert(backend.to_string(), height);
        self.add_to_heaps(height);

        // Rebalance heaps
        self.rebalance();
    }

    #[must_use]
    /// Get Byzantine-safe tip (f+1 honest backends agree).
    pub fn get(&self) -> BlockNumber {
        self.lo.keys().max().copied().unwrap_or_default()
    }

    #[must_use]
    /// Get latest reported tip (any backend).
    pub fn latest(&self) -> BlockNumber {
        self.hi.keys().max().or_else(|| self.lo.keys().max()).copied().unwrap_or_default()
    }

    fn remove_from_heaps(&mut self, height: BlockNumber) {
        for heap in [&mut self.hi, &mut self.lo] {
            if let Some(count) = heap.get_mut(&height) {
                *count -= 1;
                if *count == 0 {
                    heap.remove(&height);
                }
                return;
            }
        }
    }

    fn add_to_heaps(&mut self, height: BlockNumber) {
        *self.lo.entry(height).or_insert(0) += 1;
    }

    fn rebalance(&mut self) {
        // lo should contain n-f elements (the honest majority)
        // hi should contain f elements (potentially Byzantine)
        // We move the HIGHEST values from lo to hi until lo has n-f elements
        let target_lo_count = self.tips.len().saturating_sub(self.f);

        // First, move everything from hi back to lo for fresh rebalancing
        for (height, count) in std::mem::take(&mut self.hi) {
            *self.lo.entry(height).or_insert(0) += count;
        }

        // Now move the highest values from lo to hi until lo has target_lo_count elements
        while self.lo.values().sum::<usize>() > target_lo_count {
            // Get the highest value in lo
            if let Some((&height, &count)) = self.lo.iter().next_back() {
                let lo_count: usize = self.lo.values().sum();
                let excess = lo_count - target_lo_count;

                if excess >= count {
                    // Move entire entry to hi
                    self.lo.remove(&height);
                    *self.hi.entry(height).or_insert(0) += count;
                } else {
                    // Move only part of the count
                    // height exists: obtained from iter().next_back() and only if-branch removes it
                    *self.lo.get_mut(&height).expect("height exists in lo") -= excess;
                    *self.hi.entry(height).or_insert(0) += excess;
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_safe_tip() {
        let tracker = SafeTip::new(1);
        assert_eq!(tracker.get(), 0);
        assert_eq!(tracker.latest(), 0);
    }

    #[test]
    fn test_single_backend() {
        let mut tracker = SafeTip::new(0);
        tracker.update("backend1", 100);
        assert_eq!(tracker.get(), 100);
        assert_eq!(tracker.latest(), 100);
    }

    #[test]
    fn test_update_same_backend() {
        let mut tracker = SafeTip::new(0);
        tracker.update("backend1", 100);
        tracker.update("backend1", 150);
        assert_eq!(tracker.get(), 150);
        assert_eq!(tracker.latest(), 150);
    }

    #[test]
    fn test_byzantine_safety_with_f1() {
        // With f=1, we need n-f backends to agree for safety
        // If we have 3 backends and f=1, we need 2 to agree
        let mut tracker = SafeTip::new(1);

        tracker.update("backend1", 100);
        tracker.update("backend2", 100);
        tracker.update("backend3", 200); // Byzantine/ahead

        // With f=1 and 3 backends:
        // - n=3, f=1, so lo should have n-f=2 elements total (by count)
        // - After rebalancing: lo={100:2}, hi={200:1}
        let safe = tracker.get();
        let latest = tracker.latest();

        // safe = max of lo = 100 (honest majority)
        // latest = max of hi = 200 (highest reported)
        assert_eq!(safe, 100, "safe tip should be 100 (honest majority)");
        assert_eq!(latest, 200, "latest should be 200 (highest reported)");
    }

    #[test]
    fn test_all_backends_same_height() {
        let mut tracker = SafeTip::new(1);

        tracker.update("backend1", 100);
        tracker.update("backend2", 100);
        tracker.update("backend3", 100);

        assert_eq!(tracker.get(), 100);
        assert_eq!(tracker.latest(), 100);
    }

    #[test]
    fn test_backends_at_different_heights() {
        let mut tracker = SafeTip::new(1);

        tracker.update("backend1", 100);
        tracker.update("backend2", 101);
        tracker.update("backend3", 102);
        tracker.update("backend4", 103);

        // With f=1 and 4 backends:
        // - n=4, f=1, so lo should have n-f=3 elements total
        // - After all updates: lo has 100, 101, 102, 103 (each count 1) = 4 total
        // - Rebalance moves highest (103) to hi
        // - Final: lo={100:1, 101:1, 102:1}, hi={103:1}
        let safe = tracker.get();
        let latest = tracker.latest();

        // safe = max of lo = 102
        // latest = max of hi = 103
        assert_eq!(safe, 102, "safe tip should be 102");
        assert_eq!(latest, 103, "latest should be 103");
    }

    #[test]
    fn test_f_zero_all_trusted() {
        // With f=0, all backends are trusted
        let mut tracker = SafeTip::new(0);

        tracker.update("backend1", 100);
        tracker.update("backend2", 150);

        // With f=0, both values should be in lo
        assert_eq!(tracker.latest(), 150);
    }
}
