//! Gap-aware range tracking for cached block ranges.
//!
//! The [`RMap`] data structure efficiently tracks which block ranges have been cached,
//! supporting operations for inserting ranges, checking if blocks are cached, and
//! finding gaps in coverage.

use std::collections::BTreeMap;

use alloy_primitives::BlockNumber;

/// A gap-aware range map for tracking cached block ranges.
///
/// `RMap` maintains a set of non-overlapping, non-adjacent ranges and provides
/// efficient operations for:
/// - Inserting new ranges (with automatic merging of adjacent/overlapping ranges)
/// - Checking if a specific block is cached
/// - Finding gaps within a query range
///
/// # Example
///
/// ```
/// use roxy_cache::RMap;
///
/// let mut rmap = RMap::new();
/// rmap.insert(100, 200);  // Cache blocks 100-200
/// rmap.insert(300, 400);  // Cache blocks 300-400
///
/// assert!(rmap.contains(150));
/// assert!(!rmap.contains(250));
///
/// let gaps = rmap.gaps_in(50, 450);
/// assert_eq!(gaps, vec![(50, 99), (201, 299), (401, 450)]);
/// ```
#[derive(Debug, Clone, Default)]
pub struct RMap {
    /// Map from range start to range end (inclusive).
    /// Invariant: ranges are non-overlapping and non-adjacent.
    ranges: BTreeMap<BlockNumber, BlockNumber>,
}

impl RMap {
    /// Create a new empty range map.
    #[must_use]
    pub const fn new() -> Self {
        Self { ranges: BTreeMap::new() }
    }

    /// Insert a range of blocks into the map.
    ///
    /// The range is inclusive: `insert(100, 200)` includes blocks 100 through 200.
    /// Adjacent and overlapping ranges are automatically merged.
    ///
    /// # Panics
    ///
    /// Panics if `start > end`.
    pub fn insert(&mut self, start: BlockNumber, end: BlockNumber) {
        assert!(start <= end, "start must be <= end");

        // Find the effective start and end after merging
        let mut new_start = start;
        let mut new_end = end;

        // Collect ranges to remove (those that overlap or are adjacent)
        let mut to_remove = Vec::new();

        // Check ranges that might overlap or be adjacent
        for (&range_start, &range_end) in &self.ranges {
            // A range overlaps or is adjacent if:
            // - It starts before or at (new_end + 1), AND
            // - It ends after or at (new_start - 1) (accounting for adjacency)
            let adjacent_or_overlaps = range_start <= new_end.saturating_add(1)
                && range_end >= new_start.saturating_sub(1);

            if adjacent_or_overlaps {
                to_remove.push(range_start);
                new_start = new_start.min(range_start);
                new_end = new_end.max(range_end);
            }
        }

        // Remove merged ranges
        for start in to_remove {
            self.ranges.remove(&start);
        }

        // Insert the merged range
        self.ranges.insert(new_start, new_end);
    }

    /// Check if a specific block is contained in any cached range.
    #[must_use]
    pub fn contains(&self, block: BlockNumber) -> bool {
        // Find the range that starts at or before this block
        // Only need to check the first (highest) range that starts at or before block
        if let Some((&range_start, &range_end)) = self.ranges.range(..=block).next_back() {
            return block >= range_start && block <= range_end;
        }
        false
    }

    /// Check if an entire range of blocks is contained in cached ranges.
    ///
    /// Returns `true` if all blocks from `start` to `end` (inclusive) are cached.
    #[must_use]
    pub fn contains_range(&self, start: BlockNumber, end: BlockNumber) -> bool {
        if start > end {
            return false;
        }
        self.gaps_in(start, end).is_empty()
    }

    /// Find all gaps (uncached ranges) within the query range.
    ///
    /// Returns a vector of `(gap_start, gap_end)` tuples representing uncached
    /// block ranges within the query range. Both bounds are inclusive.
    ///
    /// # Example
    ///
    /// ```
    /// use roxy_cache::RMap;
    ///
    /// let mut rmap = RMap::new();
    /// rmap.insert(100, 200);
    /// rmap.insert(300, 400);
    ///
    /// // Query from 50 to 450
    /// let gaps = rmap.gaps_in(50, 450);
    /// assert_eq!(gaps, vec![(50, 99), (201, 299), (401, 450)]);
    /// ```
    #[must_use]
    pub fn gaps_in(&self, start: BlockNumber, end: BlockNumber) -> Vec<(BlockNumber, BlockNumber)> {
        if start > end {
            return Vec::new();
        }

        let mut gaps = Vec::new();
        let mut cursor = start;

        // Iterate through ranges that might affect our query
        for (&range_start, &range_end) in &self.ranges {
            // Skip ranges that end before our query starts
            if range_end < start {
                continue;
            }

            // Stop if range starts after our query ends
            if range_start > end {
                break;
            }

            // If there's a gap before this range
            if cursor < range_start {
                let gap_end = (range_start - 1).min(end);
                if cursor <= gap_end {
                    gaps.push((cursor, gap_end));
                }
            }

            // Move cursor past this range
            cursor = cursor.max(range_end.saturating_add(1));

            // If we've passed the end of our query, we're done
            if cursor > end {
                break;
            }
        }

        // Check for trailing gap
        if cursor <= end {
            gaps.push((cursor, end));
        }

        gaps
    }

    /// Returns the number of distinct ranges stored in the map.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Returns `true` if the map contains no ranges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Returns the total number of blocks covered by all ranges.
    #[must_use]
    pub fn covered_blocks(&self) -> u64 {
        self.ranges.iter().map(|(&start, &end)| end - start + 1).sum()
    }

    /// Clear all ranges from the map.
    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Returns an iterator over all ranges in ascending order.
    ///
    /// Each item is a `(start, end)` tuple representing an inclusive range.
    pub fn iter(&self) -> impl Iterator<Item = (BlockNumber, BlockNumber)> + '_ {
        self.ranges.iter().map(|(&start, &end)| (start, end))
    }

    /// Returns the minimum block number covered by any range, if any.
    #[must_use]
    pub fn min_block(&self) -> Option<BlockNumber> {
        self.ranges.first_key_value().map(|(&start, _)| start)
    }

    /// Returns the maximum block number covered by any range, if any.
    #[must_use]
    pub fn max_block(&self) -> Option<BlockNumber> {
        self.ranges.last_key_value().map(|(_, &end)| end)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_new_map_is_empty() {
        let rmap = RMap::new();
        assert!(rmap.is_empty());
        assert_eq!(rmap.len(), 0);
        assert_eq!(rmap.covered_blocks(), 0);
    }

    #[test]
    fn test_default_is_empty() {
        let rmap = RMap::default();
        assert!(rmap.is_empty());
    }

    #[test]
    fn test_insert_single_range() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 101);
        assert!(rmap.contains(100));
        assert!(rmap.contains(150));
        assert!(rmap.contains(200));
        assert!(!rmap.contains(99));
        assert!(!rmap.contains(201));
    }

    #[test]
    fn test_insert_disjoint_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        rmap.insert(300, 400);

        assert_eq!(rmap.len(), 2);
        assert_eq!(rmap.covered_blocks(), 202);
        assert!(rmap.contains(150));
        assert!(rmap.contains(350));
        assert!(!rmap.contains(250));
    }

    #[test]
    fn test_insert_overlapping_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        rmap.insert(150, 250);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 151); // 100-250 inclusive
        assert!(rmap.contains(100));
        assert!(rmap.contains(250));
    }

    #[test]
    fn test_insert_adjacent_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(100, 199);
        rmap.insert(200, 300);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 201); // 100-300 inclusive
    }

    #[test]
    fn test_insert_contained_range() {
        let mut rmap = RMap::new();
        rmap.insert(100, 300);
        rmap.insert(150, 200);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 201);
    }

    #[test]
    fn test_insert_containing_range() {
        let mut rmap = RMap::new();
        rmap.insert(150, 200);
        rmap.insert(100, 300);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 201);
    }

    #[test]
    fn test_insert_merges_multiple_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(100, 150);
        rmap.insert(200, 250);
        rmap.insert(300, 350);
        assert_eq!(rmap.len(), 3);

        // Insert a range that bridges all three
        rmap.insert(140, 310);
        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 251); // 100-350 inclusive
    }

    #[test]
    fn test_insert_single_block() {
        let mut rmap = RMap::new();
        rmap.insert(100, 100);

        assert_eq!(rmap.len(), 1);
        assert_eq!(rmap.covered_blocks(), 1);
        assert!(rmap.contains(100));
        assert!(!rmap.contains(99));
        assert!(!rmap.contains(101));
    }

    #[rstest]
    #[case::within_range(100, 200, 150, true)]
    #[case::at_start(100, 200, 100, true)]
    #[case::at_end(100, 200, 200, true)]
    #[case::before_range(100, 200, 99, false)]
    #[case::after_range(100, 200, 201, false)]
    #[case::far_before(100, 200, 0, false)]
    #[case::far_after(100, 200, 1000, false)]
    fn test_contains_single_range(
        #[case] range_start: BlockNumber,
        #[case] range_end: BlockNumber,
        #[case] query: BlockNumber,
        #[case] expected: bool,
    ) {
        let mut rmap = RMap::new();
        rmap.insert(range_start, range_end);
        assert_eq!(rmap.contains(query), expected);
    }

    #[test]
    fn test_contains_empty_map() {
        let rmap = RMap::new();
        assert!(!rmap.contains(0));
        assert!(!rmap.contains(100));
        assert!(!rmap.contains(u64::MAX));
    }

    #[test]
    fn test_gaps_in_empty_map() {
        let rmap = RMap::new();
        let gaps = rmap.gaps_in(100, 200);
        assert_eq!(gaps, vec![(100, 200)]);
    }

    #[test]
    fn test_gaps_in_fully_covered() {
        let mut rmap = RMap::new();
        rmap.insert(50, 250);
        let gaps = rmap.gaps_in(100, 200);
        assert!(gaps.is_empty());
    }

    #[test]
    fn test_gaps_in_example_from_docs() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        rmap.insert(300, 400);
        let gaps = rmap.gaps_in(50, 450);
        assert_eq!(gaps, vec![(50, 99), (201, 299), (401, 450)]);
    }

    #[test]
    fn test_gaps_in_leading_gap_only() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        let gaps = rmap.gaps_in(50, 200);
        assert_eq!(gaps, vec![(50, 99)]);
    }

    #[test]
    fn test_gaps_in_trailing_gap_only() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        let gaps = rmap.gaps_in(100, 250);
        assert_eq!(gaps, vec![(201, 250)]);
    }

    #[test]
    fn test_gaps_in_middle_gap_only() {
        let mut rmap = RMap::new();
        rmap.insert(100, 150);
        rmap.insert(200, 250);
        let gaps = rmap.gaps_in(100, 250);
        assert_eq!(gaps, vec![(151, 199)]);
    }

    #[test]
    fn test_gaps_in_query_within_single_range() {
        let mut rmap = RMap::new();
        rmap.insert(0, 1000);
        let gaps = rmap.gaps_in(100, 200);
        assert!(gaps.is_empty());
    }

    #[test]
    fn test_gaps_in_query_outside_all_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(500, 600);
        let gaps = rmap.gaps_in(100, 200);
        assert_eq!(gaps, vec![(100, 200)]);
    }

    #[rstest]
    #[case::empty_range(100, 99, vec![])]
    #[case::single_block_covered(100, 100, vec![])]
    #[case::single_block_not_covered(50, 50, vec![(50, 50)])]
    fn test_gaps_in_edge_cases(
        #[case] start: BlockNumber,
        #[case] end: BlockNumber,
        #[case] expected: Vec<(BlockNumber, BlockNumber)>,
    ) {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        let gaps = rmap.gaps_in(start, end);
        assert_eq!(gaps, expected);
    }

    #[test]
    fn test_contains_range_fully_covered() {
        let mut rmap = RMap::new();
        rmap.insert(100, 300);
        assert!(rmap.contains_range(150, 250));
        assert!(rmap.contains_range(100, 300));
    }

    #[test]
    fn test_contains_range_partially_covered() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        assert!(!rmap.contains_range(50, 150));
        assert!(!rmap.contains_range(150, 250));
    }

    #[test]
    fn test_contains_range_not_covered() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        assert!(!rmap.contains_range(300, 400));
    }

    #[test]
    fn test_contains_range_invalid() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        assert!(!rmap.contains_range(200, 100)); // Invalid range
    }

    #[test]
    fn test_clear() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        rmap.insert(300, 400);
        assert_eq!(rmap.len(), 2);

        rmap.clear();
        assert!(rmap.is_empty());
        assert_eq!(rmap.covered_blocks(), 0);
        assert!(!rmap.contains(150));
    }

    #[test]
    fn test_iter() {
        let mut rmap = RMap::new();
        rmap.insert(300, 400);
        rmap.insert(100, 200);
        rmap.insert(500, 600);

        let ranges: Vec<_> = rmap.iter().collect();
        assert_eq!(ranges, vec![(100, 200), (300, 400), (500, 600)]);
    }

    #[test]
    fn test_min_max_block() {
        let rmap = RMap::new();
        assert_eq!(rmap.min_block(), None);
        assert_eq!(rmap.max_block(), None);

        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        assert_eq!(rmap.min_block(), Some(100));
        assert_eq!(rmap.max_block(), Some(200));

        rmap.insert(300, 400);
        assert_eq!(rmap.min_block(), Some(100));
        assert_eq!(rmap.max_block(), Some(400));

        rmap.insert(50, 80);
        assert_eq!(rmap.min_block(), Some(50));
        assert_eq!(rmap.max_block(), Some(400));
    }

    #[test]
    fn test_clone() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);

        let cloned = rmap.clone();
        assert!(cloned.contains(150));
        assert_eq!(cloned.len(), 1);
    }

    #[test]
    fn test_debug_format() {
        let mut rmap = RMap::new();
        rmap.insert(100, 200);
        let debug_str = format!("{:?}", rmap);
        assert!(debug_str.contains("RMap"));
    }

    #[test]
    #[should_panic(expected = "start must be <= end")]
    fn test_insert_invalid_range_panics() {
        let mut rmap = RMap::new();
        rmap.insert(200, 100);
    }

    #[test]
    fn test_large_ranges() {
        let mut rmap = RMap::new();
        rmap.insert(0, 1_000_000);
        rmap.insert(2_000_000, 3_000_000);

        assert!(rmap.contains(500_000));
        assert!(rmap.contains(2_500_000));
        assert!(!rmap.contains(1_500_000));

        let gaps = rmap.gaps_in(0, 3_000_000);
        assert_eq!(gaps, vec![(1_000_001, 1_999_999)]);
    }

    #[test]
    fn test_boundary_blocks() {
        let mut rmap = RMap::new();
        rmap.insert(0, 10);

        assert!(rmap.contains(0));
        assert!(rmap.contains(10));
        assert!(!rmap.contains(11));
    }

    #[test]
    fn test_saturating_operations() {
        // Test with values near u64::MAX to ensure no overflow
        let mut rmap = RMap::new();
        rmap.insert(u64::MAX - 10, u64::MAX);

        assert!(rmap.contains(u64::MAX));
        assert!(rmap.contains(u64::MAX - 5));
        assert!(!rmap.contains(u64::MAX - 11));
    }

    #[rstest]
    #[case::no_ranges(vec![], 0)]
    #[case::single_range(vec![(100, 200)], 1)]
    #[case::two_disjoint(vec![(100, 200), (300, 400)], 2)]
    #[case::adjacent_merged(vec![(100, 199), (200, 300)], 1)]
    #[case::overlapping_merged(vec![(100, 200), (150, 250)], 1)]
    fn test_range_count_after_inserts(
        #[case] inserts: Vec<(BlockNumber, BlockNumber)>,
        #[case] expected_len: usize,
    ) {
        let mut rmap = RMap::new();
        for (start, end) in inserts {
            rmap.insert(start, end);
        }
        assert_eq!(rmap.len(), expected_len);
    }

    #[rstest]
    #[case::single_block(100, 100, 1)]
    #[case::two_blocks(100, 101, 2)]
    #[case::hundred_blocks(100, 199, 100)]
    #[case::thousand_blocks(0, 999, 1000)]
    fn test_covered_blocks(
        #[case] start: BlockNumber,
        #[case] end: BlockNumber,
        #[case] expected: u64,
    ) {
        let mut rmap = RMap::new();
        rmap.insert(start, end);
        assert_eq!(rmap.covered_blocks(), expected);
    }

    #[test]
    fn test_insert_order_independence() {
        // Forward order
        let mut rmap1 = RMap::new();
        rmap1.insert(100, 200);
        rmap1.insert(300, 400);
        rmap1.insert(500, 600);

        // Reverse order
        let mut rmap2 = RMap::new();
        rmap2.insert(500, 600);
        rmap2.insert(300, 400);
        rmap2.insert(100, 200);

        // Random order
        let mut rmap3 = RMap::new();
        rmap3.insert(300, 400);
        rmap3.insert(100, 200);
        rmap3.insert(500, 600);

        // All should have the same result
        assert_eq!(rmap1.len(), rmap2.len());
        assert_eq!(rmap1.len(), rmap3.len());
        assert_eq!(rmap1.covered_blocks(), rmap2.covered_blocks());
        assert_eq!(rmap1.covered_blocks(), rmap3.covered_blocks());

        let ranges1: Vec<_> = rmap1.iter().collect();
        let ranges2: Vec<_> = rmap2.iter().collect();
        let ranges3: Vec<_> = rmap3.iter().collect();
        assert_eq!(ranges1, ranges2);
        assert_eq!(ranges1, ranges3);
    }

    #[test]
    fn test_complex_merge_scenario() {
        let mut rmap = RMap::new();

        // Insert many small ranges
        rmap.insert(100, 110);
        rmap.insert(120, 130);
        rmap.insert(140, 150);
        rmap.insert(160, 170);
        rmap.insert(180, 190);
        assert_eq!(rmap.len(), 5);

        // Insert a range that should merge some but not all
        rmap.insert(105, 145);
        assert_eq!(rmap.len(), 3); // (100-150), (160-170), (180-190)

        // Insert a range that merges the rest
        rmap.insert(150, 180);
        assert_eq!(rmap.len(), 1); // (100-190)
        assert_eq!(rmap.covered_blocks(), 91); // 100-190 inclusive
    }
}
