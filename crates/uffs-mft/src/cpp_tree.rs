//! C++ Tree Algorithm Port
//!
//! This module implements a 100% faithful port of the C++ tree metrics
//! algorithm. It uses structures that match the C++ layout and implements the
//! exact same algorithm flow for computing descendants, treesize, and
//! `tree_allocated`.
//!
//! # Design
//!
//! This is implemented as a **transformer** approach:
//! 1. Transform `MftIndex` data into C++ port structures
//! 2. Run the C++ tree algorithm
//! 3. Write results back to `FileRecord`
//!
//! This keeps the existing code untouched and allows A/B testing between
//! algorithms.
//!
//! # Reference
//!
//! See `docs/architecture/CPP_TREE_ALGORITHM_PORT.md` for full documentation.

use crate::index::{MftIndex, NO_ENTRY};

// ============================================================================
// Constants
// ============================================================================

/// Sentinel value for end of linked list (matches C++ `negative_one`)
pub const CPP_NO_ENTRY: u32 = u32::MAX;

// ============================================================================
// C++ Port Structures
// ============================================================================

/// Result of preprocessing a subtree (matches C++ `PreprocessResult`).
///
/// This accumulates the tree metrics as we traverse the tree.
#[derive(Debug, Clone, Copy, Default)]
pub struct PreprocessResult {
    /// Total logical size of subtree
    pub length: u64,
    /// Total allocated size of subtree
    pub allocated: u64,
    /// Total bulkiness of subtree (for fragmentation penalty)
    pub bulkiness: u64,
    /// Number of streams in subtree (each stream adds +1)
    pub treesize: u32,
    /// Number of descendants (files + directories)
    pub descendants: u32,
}

impl PreprocessResult {
    /// Add another result to this one (for accumulating children)
    #[inline]
    #[allow(clippy::missing_const_for_fn)] // const fn with mutable self is unstable
    pub fn accumulate(&mut self, other: &Self) {
        self.length += other.length;
        self.allocated += other.allocated;
        self.bulkiness += other.bulkiness;
        self.treesize += other.treesize;
        self.descendants += other.descendants;
    }
}

// ============================================================================
// Delta Formula (C++ Accumulator::delta)
// ============================================================================

/// C++ delta formula for proportional hardlink share calculation.
///
/// This ensures no rounding errors when dividing a file's size among multiple
/// hardlinks. The formula distributes the value such that the sum of all
/// shares equals the original value exactly.
///
/// # Formula
///
/// ```text
/// delta(value, i, n) = value * (i + 1) / n - value * i / n
/// ```
///
/// # Arguments
///
/// * `value` - The total value to distribute (e.g., file size)
/// * `name_info` - Which hardlink this is (0-indexed from the perspective of
///   traversal)
/// * `total_names` - Total number of hardlinks for this file
///
/// # Example
///
/// For a 1001-byte file with 2 hardlinks:
/// - `delta(1001, 0, 2)` = 500 (first hardlink)
/// - `delta(1001, 1, 2)` = 501 (second hardlink)
/// - Sum = 1001 ✓
#[inline]
#[must_use]
pub fn delta(value: u64, name_info: u16, total_names: u16) -> u64 {
    if total_names == 0 {
        return 0;
    }
    if total_names == 1 {
        return value;
    }
    let n = u64::from(total_names);
    let i = u64::from(name_info);
    value * (i + 1) / n - value * i / n
}

// ============================================================================
// Tree Traversal State
// ============================================================================

/// State for the C++ tree traversal algorithm.
///
/// This holds references to the `MftIndex` and provides methods for
/// traversing the tree and computing metrics.
pub struct CppTreeTraversal<'a> {
    /// Reference to the `MftIndex` being processed
    index: &'a mut MftIndex,
    /// Debug mode flag
    debug: bool,
    /// Count of records processed (for debugging)
    processed_count: u32,
}

impl<'a> CppTreeTraversal<'a> {
    /// Create a new traversal state
    #[allow(clippy::missing_const_for_fn)] // const fn with mutable ref is unstable
    pub fn new(index: &'a mut MftIndex, debug: bool) -> Self {
        Self {
            index,
            debug,
            processed_count: 0,
        }
    }

    /// Run the C++ tree algorithm starting from root (FRS 5)
    pub fn run(&mut self) {
        // Find root directory (FRS 5)
        let root_idx = self.index.frs_to_idx.get(5).copied().unwrap_or(NO_ENTRY);
        if root_idx == NO_ENTRY {
            tracing::warn!("Root directory (FRS 5) not found, skipping tree metrics");
            return;
        }

        if self.debug {
            tracing::info!(root_idx, "Starting C++ tree traversal from root (FRS 5)");
        }

        // Start recursive traversal from root
        // Root has name_info=0, total_names=1 (single entry point)
        let _result = self.preprocess(root_idx as usize, 0, 1);

        if self.debug {
            tracing::info!(
                processed_count = self.processed_count,
                "C++ tree traversal complete"
            );
        }
    }

    /// Recursive preprocessing of a subtree (matches C++
    /// `preprocessor::operator()`).
    ///
    /// This is the core of the C++ tree algorithm:
    /// 1. Recursively process all children
    /// 2. Accumulate children's metrics
    /// 3. Add own streams with delta formula for proportional hardlink shares
    /// 4. Store results in the record
    #[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
    fn preprocess(&mut self, idx: usize, name_info: u16, total_names: u16) -> PreprocessResult {
        self.processed_count += 1;

        // Get record info we need before mutable borrow
        let record = &self.index.records[idx];
        let first_child = record.first_child;
        let is_directory = record.is_directory();
        // Use total_stream_count for tree metrics (includes internal streams like
        // $REPARSE_POINT) This matches C++ behavior where ALL streams
        // contribute to treesize (line 4788)
        let stream_count = record.total_stream_count;
        let first_stream_length = record.first_stream.size.length;
        let first_stream_allocated = record.first_stream.size.allocated;
        let record_frs = record.frs;

        // Compute total own size (all streams) for storing in record
        // This is the record's own size without delta formula
        let mut own_total_length = first_stream_length;
        let mut own_total_allocated = first_stream_allocated;
        if stream_count > 1 {
            let mut stream_idx = record.first_stream.next_entry;
            let mut streams_counted = 1_u16;
            while stream_idx != NO_ENTRY && streams_counted < stream_count {
                if let Some(stream) = self.index.streams.get(stream_idx as usize) {
                    own_total_length += stream.size.length;
                    own_total_allocated += stream.size.allocated;
                    stream_idx = stream.next_entry;
                    streams_counted += 1;
                } else {
                    break;
                }
            }
        }

        // =====================================================================
        // Step 1: Recursively process all children
        // =====================================================================
        let mut children_size = PreprocessResult::default();

        let mut child_entry_idx = first_child;
        while child_entry_idx != NO_ENTRY {
            let child_info = self.index.children[child_entry_idx as usize];
            let child_frs = child_info.child_frs;

            // Skip self-reference (root directory has parent = itself)
            if child_frs == record_frs {
                child_entry_idx = child_info.next_entry;
                continue;
            }

            // Look up child record
            let child_idx = if child_frs < self.index.frs_to_idx.len() as u64 {
                self.index.frs_to_idx[child_frs as usize]
            } else {
                NO_ENTRY
            };

            if child_idx != NO_ENTRY {
                let child_record = &self.index.records[child_idx as usize];
                let child_name_count = child_record.name_count;

                // C++ formula: name_info = name_count - 1 - child_info.name_index
                let child_name_info = if child_name_count > 0 {
                    child_name_count
                        .saturating_sub(1)
                        .saturating_sub(child_info.name_index)
                } else {
                    0
                };

                // Recursive call
                let subresult =
                    self.preprocess(child_idx as usize, child_name_info, child_name_count.max(1));

                children_size.accumulate(&subresult);
            }

            child_entry_idx = child_info.next_entry;
        }

        // =====================================================================
        // Step 2: Process own streams with delta formula
        // =====================================================================
        let mut result = children_size;

        // Add own size with proportional share (delta formula)
        let length_delta = delta(first_stream_length, name_info, total_names);
        let allocated_delta = delta(first_stream_allocated, name_info, total_names);

        result.length += length_delta;
        result.allocated += allocated_delta;
        result.bulkiness += allocated_delta;
        result.treesize += 1;
        result.descendants += 1;

        // Process additional streams (ADS)
        if stream_count > 1 {
            let first_stream_idx = self.index.records[idx].first_stream.next_entry;
            let mut stream_idx = first_stream_idx;
            let mut streams_processed = 1_u16;

            while stream_idx != NO_ENTRY && streams_processed < stream_count {
                let stream = &self.index.streams[stream_idx as usize];

                let stream_length_delta = delta(stream.size.length, name_info, total_names);
                let stream_allocated_delta = delta(stream.size.allocated, name_info, total_names);

                result.length += stream_length_delta;
                result.allocated += stream_allocated_delta;
                result.bulkiness += stream_allocated_delta;
                result.treesize += 1;

                stream_idx = stream.next_entry;
                streams_processed += 1;
            }
        }

        // =====================================================================
        // Step 3: Store results in the record
        // =====================================================================
        let record_mut = &mut self.index.records[idx];

        if is_directory {
            // C++ outputs sizeinfo.treesize (stream count) as "Descendants" column.
            // result.treesize contains the accumulated stream count (children + own
            // streams). This matches C++ line 885: k->treesize +=
            // children_size.treesize
            record_mut.descendants = result.treesize;
            // C++ stores accumulated length (sum of file sizes) in the default stream's
            // length field, which becomes the directory's "Size" in output.
            // We store children's accumulated size + own size (all streams).
            record_mut.treesize = children_size.length + own_total_length;
            record_mut.tree_allocated = children_size.allocated + own_total_allocated;
        } else {
            record_mut.descendants = 0;
            // Files: treesize = own size (all streams, not stream count)
            record_mut.treesize = own_total_length;
            record_mut.tree_allocated = own_total_allocated;
        }

        result
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Compute tree metrics using the C++ port algorithm.
///
/// This is the main entry point for the C++ tree algorithm.
pub fn compute_tree_metrics_cpp_port(index: &mut MftIndex, debug: bool) {
    let n = index.records.len();
    if n == 0 {
        tracing::debug!("⏭️  Skipping tree metrics (cpp_port) - no records");
        return;
    }

    // Count files with multiple names (hard links)
    let multi_name_count = index
        .records
        .iter()
        .filter(|rec| rec.name_count > 1)
        .count();
    let total_names: u32 = index
        .records
        .iter()
        .map(|rec| u32::from(rec.name_count.max(1)))
        .sum();

    if debug {
        tracing::info!("=== TREE METRICS DEBUG (C++ PORT) ===");
        tracing::info!(total_records = n, "Total records");
        tracing::info!(
            children_entries = index.children.len(),
            "Total children entries"
        );
        tracing::info!(
            multi_name_files = multi_name_count,
            total_names = total_names,
            "Hard link statistics"
        );
    }

    tracing::debug!(
        records = n,
        children = index.children.len(),
        multi_name_files = multi_name_count,
        "🔨 Computing tree metrics (C++ port algorithm)..."
    );

    let mut traversal = CppTreeTraversal::new(index, debug);
    traversal.run();

    // Debug: show root descendants
    if let Some(&root_idx) = index.frs_to_idx.get(5) {
        if root_idx != NO_ENTRY {
            if let Some(root) = index.records.get(root_idx as usize) {
                tracing::debug!(
                    root_descendants = root.descendants,
                    root_treesize = root.treesize,
                    "Root directory metrics"
                );
            }
        }
    }

    tracing::debug!(records = n, "✅ Tree metrics computed (C++ port algorithm)");
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Delta Formula Tests
    // ========================================================================

    #[test]
    fn test_delta_single_hardlink() {
        // Single hardlink: file gets 100% of its size
        assert_eq!(delta(1000, 0, 1), 1000);
        assert_eq!(delta(u64::MAX, 0, 1), u64::MAX);
    }

    #[test]
    fn test_delta_two_hardlinks_even() {
        // Two hardlinks, even split: 1000 / 2 = 500 each
        assert_eq!(delta(1000, 0, 2), 500);
        assert_eq!(delta(1000, 1, 2), 500);
        assert_eq!(delta(1000, 0, 2) + delta(1000, 1, 2), 1000);
    }

    #[test]
    fn test_delta_two_hardlinks_odd() {
        // Two hardlinks, odd value: 1001 / 2 = 500 + 501
        assert_eq!(delta(1001, 0, 2), 500);
        assert_eq!(delta(1001, 1, 2), 501);
        assert_eq!(delta(1001, 0, 2) + delta(1001, 1, 2), 1001);
    }

    #[test]
    fn test_delta_three_hardlinks() {
        // Three hardlinks: 100 / 3 = 33 + 33 + 34
        assert_eq!(delta(100, 0, 3), 33);
        assert_eq!(delta(100, 1, 3), 33);
        assert_eq!(delta(100, 2, 3), 34);
        assert_eq!(delta(100, 0, 3) + delta(100, 1, 3) + delta(100, 2, 3), 100);
    }

    #[test]
    fn test_delta_max_hardlinks() {
        // Maximum hardlinks (1023 per C++ limit)
        let value = 1_000_000_u64;
        let total_names = 1023_u16;
        let mut sum = 0_u64;
        for idx in 0..total_names {
            sum += delta(value, idx, total_names);
        }
        assert_eq!(sum, value);
    }

    #[test]
    fn test_delta_large_values() {
        // Large file sizes (petabyte scale)
        let petabyte = 1_000_000_000_000_000_u64;
        assert_eq!(delta(petabyte, 0, 2) + delta(petabyte, 1, 2), petabyte);
    }

    #[test]
    fn test_delta_zero_total_names() {
        assert_eq!(delta(1000, 0, 0), 0);
    }

    #[test]
    fn test_delta_zero_value() {
        assert_eq!(delta(0, 0, 1), 0);
        assert_eq!(delta(0, 0, 2), 0);
        assert_eq!(delta(0, 1, 2), 0);
    }

    // ========================================================================
    // PreprocessResult Tests
    // ========================================================================

    #[test]
    fn test_preprocess_result_accumulate() {
        let mut result_a = PreprocessResult {
            length: 100,
            allocated: 200,
            bulkiness: 200,
            treesize: 5,
            descendants: 10,
        };
        let result_b = PreprocessResult {
            length: 50,
            allocated: 100,
            bulkiness: 100,
            treesize: 3,
            descendants: 5,
        };
        result_a.accumulate(&result_b);
        assert_eq!(result_a.length, 150);
        assert_eq!(result_a.allocated, 300);
        assert_eq!(result_a.bulkiness, 300);
        assert_eq!(result_a.treesize, 8);
        assert_eq!(result_a.descendants, 15);
    }
}
