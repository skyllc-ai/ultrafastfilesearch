//! C++-port tree metrics computation.
//!
//! This module implements the tree-metrics algorithm used for parity with the
//! C++ reference implementation. This is the sole tree-metrics code path.
//!
//! Key properties:
//! - **Two-channel model**: Channel A is used for propagation up the tree
//!   (includes internal streams), while Channel B is what gets printed for
//!   directory rows (excludes internal streams).
//! - **Per-stream hardlink delta**: sizes are distributed per stream using the
//!   exact C++ floor-division delta formula.
//! - **Orphan sweep**: after traversing from the NTFS root (FRS 5), we sweep
//!   any unvisited records to ensure LIVE scans don't leave some directories
//!   with zeroed metrics.
//!
//! Notes:
//! - This code intentionally uses indexed access into internal vectors; indices
//!   are produced by the index builder and are expected to be valid.
#![allow(clippy::indexing_slicing)]

// AUTO-GENERATED DROP-IN REPLACEMENT
// This file fixes LIVE/ONLINE parity gaps by ensuring the C++-port tree
// metrics:
// - Use the two-channel model (propagation vs printed metrics)
// - Distribute internal stream deltas *per stream* (not as a single aggregate)
// - Always initializes every record (orphan sweep) so LIVE scans never leave
//   Size/Desc = 0
//
// Intended for: crates/uffs-mft/src/cpp_tree.rs
//
// Notes:
// - We intentionally do NOT memoize `preprocess` results because the delta
//   distribution depends on (name_info, total_names). Caching by record would
//   break hardlink accounting.
// - The "treesize" field in the returned aggregate is the C++ Channel-A
//   stream-count (counts streams in the subtree, including internal streams and
//   ADS, and can exceed row-count).

use crate::index::{InternalStreamInfo, MftIndex, NO_ENTRY};

/// Computes the delta share for a hardlink using the exact C++ floor-division
/// formula.
///
/// Distributes `value` across `total_names` hardlinks, returning the share for
/// hardlink index `name_info`. This matches the C++ implementation exactly.
///
/// **Important**: `name_info` must be the C++ transformed index, NOT the raw
/// `name_index`. Use [`compute_name_info`] to convert `name_index` to
/// `name_info`.
#[inline]
const fn delta(value: u64, name_info: u32, total_names: u32) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let n64 = total_names as u64;
    let i64 = name_info as u64;
    value * (i64 + 1) / n64 - value * i64 / n64
}

/// Computes the C++ `name_info` from a raw `name_index`.
///
/// C++ uses: `name_info = name_count - 1 - name_index`
///
/// This reverses the order so that the delta distribution matches C++ exactly.
/// The extra byte from floor-division goes to the *last* link (highest
/// `name_info`), not the first.
///
/// # Example
/// ```ignore
/// // For a file with 2 hardlinks:
/// // name_index=0 -> name_info=1 (gets the extra byte)
/// // name_index=1 -> name_info=0
/// let name_info = compute_name_info(0, 2); // returns 1
/// ```
#[inline]
#[cfg(test)] // Only used in tests now; production uses compute_name_info_checked
const fn compute_name_info(name_index: u32, total_names: u32) -> u32 {
    if total_names <= 1 {
        return 0;
    }
    // Clamp name_index to valid range to prevent underflow
    let clamped_index = if name_index >= total_names {
        total_names - 1
    } else {
        name_index
    };
    total_names - 1 - clamped_index
}

/// Computes `name_info` with optional debug logging when clamping occurs.
///
/// This is the debug-aware version of [`compute_name_info`] that logs when
/// `name_index >= total_names`, which indicates a potential parity issue
/// (two hardlinks mapping to the same `i` can skew totals).
#[inline]
#[allow(clippy::single_call_fn)] // Single call site but serves as parity diagnostic
fn compute_name_info_checked(
    name_index: u32,
    total_names: u32,
    child_frs: u64,
    debug: bool,
) -> u32 {
    if total_names <= 1 {
        return 0;
    }
    // Check for out-of-range name_index (parity risk)
    if name_index >= total_names {
        // Always log in release mode too - this is a parity diagnostic
        tracing::warn!(
            child_frs,
            name_index,
            total_names,
            "[TRIP] name_index out of range; clamping (parity risk)"
        );
        if debug {
            // Extra verbose output in debug mode
            tracing::debug!(
                child_frs,
                name_index,
                total_names,
                "[TRIP] compute_name_info_checked: clamping name_index to total_names-1"
            );
        }
        return 0; // Clamped to total_names-1, result = total_names - 1 - (total_names-1) = 0
    }
    total_names - 1 - name_index
}

/// Aggregated tree metrics returned by recursive traversal.
#[derive(Clone, Copy, Default)]
struct Agg {
    /// Total logical size in bytes.
    length: u64,
    /// Total allocated size in bytes.
    allocated: u64,
    /// Channel-A stream count in subtree (used to derive printed directory
    /// descendants).
    treesize: u32,
}

/// Traversal state for computing tree metrics in C++ parity mode.
struct CppTreeTraversal<'a> {
    /// Mutable reference to the `MftIndex` being processed.
    index: &'a mut MftIndex,
    /// Marks whether we've visited a record at least once (used for the orphan
    /// sweep).
    seen: Vec<bool>,
    /// Whether to emit debug tracing.
    debug: bool,
    /// Fix 5: Skip orphan sweep for parity mode.
    /// When true, only records reachable from ROOT via Win32-visible
    /// `FILE_NAME` edges are included in tree aggregation. Orphans without
    /// Win32 paths are excluded to match C++ Win32 enumeration behavior.
    skip_orphans: bool,
}

impl CppTreeTraversal<'_> {
    /// Runs the tree traversal starting from ROOT, then sweeps orphans.
    fn run(&mut self) {
        // Canonical NTFS root directory record number.
        const ROOT_FRS: u64 = 5;

        tracing::debug!("[TRIP] CppTreeTraversal::run ENTER");

        // Primary traversal from ROOT (if present).
        if let Some(root_idx) = self.index.frs_to_idx_opt(ROOT_FRS) {
            tracing::debug!(
                root_idx,
                "[TRIP] CppTreeTraversal::run -> starting from ROOT (FRS=5)"
            );
            // Root has a single visible path entry in output context.
            let _: Agg = self.preprocess(root_idx, 0, 1);
            tracing::debug!("[TRIP] CppTreeTraversal::run -> ROOT traversal done");
        } else if self.debug {
            tracing::warn!(
                "[cpp_tree] WARNING: ROOT_FRS=5 not present in frs_to_idx; running orphan sweep only"
            );
        }

        // Fix 5: Orphan sweep is skipped in parity mode (skip_orphans=true).
        // For parity with C++ Win32 enumeration, only records reachable from ROOT
        // via Win32-visible FILE_NAME edges should be included in tree aggregation.
        // Orphans without Win32 paths would inflate root Size/Descendants without
        // producing printable rows.
        if self.skip_orphans {
            let orphan_count = self.seen.iter().filter(|&&seen| !seen).count();
            tracing::debug!(
                orphan_count,
                "[TRIP] CppTreeTraversal::run EXIT (orphan sweep SKIPPED for parity mode)"
            );
            return;
        }

        // Orphan sweep: ensure every record has its printed tree metrics initialized.
        // This prevents LIVE scans from leaving some directories with Size/Desc = 0
        // due to transient linkage gaps.
        tracing::debug!("[TRIP] CppTreeTraversal::run -> starting orphan sweep");
        let mut orphan_count = 0_usize;
        for idx in 0..self.index.records.len() {
            if !self.seen[idx] {
                orphan_count += 1;
                let _: Agg = self.preprocess(idx, 0, 1);
            }
        }
        tracing::debug!(
            orphan_count,
            "[TRIP] CppTreeTraversal::run EXIT (orphan sweep done)"
        );
    }

    /// Recursively computes tree metrics for a record and its children.
    ///
    /// Returns the Channel-A aggregate (length, allocated, treesize) for this
    /// subtree, which the caller uses to accumulate into its own metrics.
    fn preprocess(&mut self, record_idx: usize, name_info: u32, total_names_raw: u32) -> Agg {
        if record_idx >= self.index.records.len() {
            return Agg::default();
        }

        self.seen[record_idx] = true;

        let total_names = total_names_raw.max(1);

        // Snapshot only the fields we need (avoid cloning the whole `FileRecord`).
        let (
            is_directory,
            first_child,
            first_stream_next,
            first_internal_stream,
            total_stream_count,
            first_len,
            first_alloc,
        ) = {
            let rec = &self.index.records[record_idx];
            (
                rec.stdinfo.is_directory(),
                rec.first_child,
                rec.first_stream.next_entry,
                rec.first_internal_stream,
                rec.total_stream_count,
                rec.first_stream.size.length,
                rec.first_stream.size.allocated,
            )
        };

        // 1) Aggregate children (Channel A outputs from children).
        let mut children = Agg::default();
        if is_directory {
            let mut child_entry_idx = first_child;
            while child_entry_idx != NO_ENTRY {
                // Extract fields from child_entry before calling preprocess (borrow checker).
                let (child_frs, child_name_idx, next_child_entry) = {
                    let ce = &self.index.children[child_entry_idx as usize];
                    (ce.child_frs, ce.name_index, ce.next_entry)
                };

                // Resolve child record index from child FRS.
                if let Some(child_idx) = self.index.frs_to_idx_opt(child_frs) {
                    // Determine which hardlink name of the child this directory entry refers to.
                    // Use the checked helper function to compute name_info from name_index.
                    // This logs when clamping occurs (parity risk indicator).
                    let child_total_names = u32::from(self.index.records[child_idx].name_count);
                    let child_name_info = compute_name_info_checked(
                        u32::from(child_name_idx),
                        child_total_names,
                        child_frs,
                        self.debug,
                    );

                    let child_agg =
                        self.preprocess(child_idx, child_name_info, child_total_names.max(1));

                    children.length = children.length.saturating_add(child_agg.length);
                    children.allocated = children.allocated.saturating_add(child_agg.allocated);
                    children.treesize = children.treesize.saturating_add(child_agg.treesize);
                }

                child_entry_idx = next_child_entry;
            }
        }

        // 2) Own streams (Channel A): default stream + ADS + internal streams,
        //    delta-distributed.
        let mut own_len = delta(first_len, name_info, total_names);
        let mut own_alloc = delta(first_alloc, name_info, total_names);
        // Internal streams must be delta-distributed per-stream (rounding correctness).
        let mut internal_idx = first_internal_stream;
        while internal_idx != NO_ENTRY {
            let ist: &InternalStreamInfo = &self.index.internal_streams[internal_idx as usize];
            own_len = own_len.saturating_add(delta(ist.size.length, name_info, total_names));
            own_alloc = own_alloc.saturating_add(delta(ist.size.allocated, name_info, total_names));
            internal_idx = ist.next_entry;
        }
        // Overflow user-visible streams (delta per stream).
        // Note: first_stream is embedded in the record; additional streams are in the
        // streams vec.
        let mut stream_idx = first_stream_next;
        while stream_idx != NO_ENTRY {
            let stream = &self.index.streams[stream_idx as usize];
            own_len = own_len.saturating_add(delta(stream.size.length, name_info, total_names));
            own_alloc =
                own_alloc.saturating_add(delta(stream.size.allocated, name_info, total_names));
            stream_idx = stream.next_entry;
        }

        // Stream count contribution (Channel A): counts ALL streams on the record (incl
        // internal).
        let own_stream_count: u32 = u32::from(total_stream_count.max(1));

        // Channel-A aggregate returned to parent.
        let result = Agg {
            length: children.length.saturating_add(own_len),
            allocated: children.allocated.saturating_add(own_alloc),
            treesize: children.treesize.saturating_add(own_stream_count),
        };

        // 3) Store printed metrics (Channel B) into the record fields.
        // These values are what show up in the scan output / parity checks.
        {
            let rec = &mut self.index.records[record_idx];

            if is_directory {
                // Printed descendants:
                //   - Use children Channel-A stream-count + 1 (directory itself).
                rec.descendants = children.treesize.saturating_add(1);

                // C++ parity: The printed treesize for a directory is:
                //   default_stream.length + children.length
                // In C++, only the default stream (type_name_id==0) gets children sizes added
                // (ntfs_index_load.hpp line 677: k->length += children_size.length).
                // Non-default streams (ADS) keep their original length.
                // So the directory's printed size = first_len + children.length,
                // NOT including ADS stream sizes.
                rec.treesize = children.length.saturating_add(first_len);
                rec.tree_allocated = children.allocated.saturating_add(first_alloc);
            } else {
                // Files print 0 descendants, and size == default stream only.
                rec.descendants = 0;
                rec.treesize = first_len;
                rec.tree_allocated = first_alloc;
            }
        }

        result
    }
}

/// Computes tree metrics using the C++-port algorithm.
///
/// Populates `treesize`, `tree_allocated`, and `descendants` for directory
/// records. If `debug` is true, emits warnings for unexpected index conditions.
///
/// # Arguments
/// * `index` - The MFT index to compute tree metrics for
/// * `debug` - Whether to emit debug tracing
/// * `skip_orphans` - Fix 5: If true, skip orphan sweep for parity mode. Only
///   records reachable from ROOT via Win32-visible `FILE_NAME` edges are
///   included in tree aggregation.
pub fn compute_tree_metrics_cpp_port(index: &mut MftIndex, debug: bool, skip_orphans: bool) {
    tracing::debug!(
        records = index.records.len(),
        skip_orphans,
        "[TRIP] cpp_tree::compute_tree_metrics_cpp_port ENTER (FIXED v0.2.187+)"
    );
    let seen = vec![false; index.records.len()];
    let mut traversal = CppTreeTraversal {
        index,
        seen,
        debug,
        skip_orphans,
    };
    traversal.run();
    tracing::debug!("[TRIP] cpp_tree::compute_tree_metrics_cpp_port -> traversal.run() done");

    // Diagnostic: every directory should have descendants >= 1 after tree
    // metrics computation. If we find a directory with descendants == 0, it
    // means the record was never stamped (traversal bug).
    // NOTE: This runs in RELEASE mode too for diagnosing LIVE scan issues.
    // TODO: Remove or gate behind a feature flag once parity is confirmed.
    for (idx, rec) in index.records.iter().enumerate() {
        if rec.stdinfo.is_directory() && rec.descendants == 0 {
            // Log warning instead of panicking - this helps diagnose issues
            // without crashing production builds.
            tracing::warn!(
                frs = rec.frs,
                idx = idx,
                first_child = rec.first_child,
                name_count = rec.name_count,
                is_reparse = rec.stdinfo.is_reparse(),
                reparse_tag = rec.reparse_tag,
                "[cpp_tree] WARNING: Directory has descendants=0 after tree metrics"
            );
        }
    }
    tracing::debug!("[TRIP] cpp_tree::compute_tree_metrics_cpp_port EXIT");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the delta function correctly distributes values across
    /// hardlinks.
    ///
    /// The C++ formula is: `delta(v, i, n) = floor(v*(i+1)/n) - floor(v*i/n)`
    /// This ensures the sum of all deltas equals the original value exactly.
    #[test]
    fn test_delta_sum_equals_original() {
        // Test various combinations of value and total_names
        let test_cases: &[(u64, u32)] = &[
            (0, 1),
            (1, 1),
            (5, 1),
            (5, 2),
            (5, 3),
            (100, 3),
            (1001, 2),
            (1000, 7),
            (999, 10),
            (1_000_000, 100),
        ];

        for &(value, total_names) in test_cases {
            let mut sum = 0_u64;
            for name_info in 0..total_names {
                sum += delta(value, name_info, total_names);
            }
            assert_eq!(
                sum, value,
                "Sum of deltas should equal original value for v={value}, n={total_names}"
            );
        }
    }

    /// Tests specific delta values to ensure C++ parity.
    ///
    /// For n=2, the C++ formula gives the extra byte to the SECOND link (i=1),
    /// not the first. This is critical for parity.
    #[test]
    fn test_delta_specific_values() {
        // For value=5, n=2:
        // i=0: 5*1/2 - 5*0/2 = 2 - 0 = 2
        // i=1: 5*2/2 - 5*1/2 = 5 - 2 = 3
        assert_eq!(delta(5, 0, 2), 2, "First link of 5/2 should get 2");
        assert_eq!(delta(5, 1, 2), 3, "Second link of 5/2 should get 3");

        // For value=1001, n=2:
        // i=0: 1001*1/2 - 1001*0/2 = 500 - 0 = 500
        // i=1: 1001*2/2 - 1001*1/2 = 1001 - 500 = 501
        assert_eq!(
            delta(1001, 0, 2),
            500,
            "First link of 1001/2 should get 500"
        );
        assert_eq!(
            delta(1001, 1, 2),
            501,
            "Second link of 1001/2 should get 501"
        );

        // For value=10, n=3:
        // i=0: 10*1/3 - 10*0/3 = 3 - 0 = 3
        // i=1: 10*2/3 - 10*1/3 = 6 - 3 = 3
        // i=2: 10*3/3 - 10*2/3 = 10 - 6 = 4
        assert_eq!(delta(10, 0, 3), 3, "First link of 10/3 should get 3");
        assert_eq!(delta(10, 1, 3), 3, "Second link of 10/3 should get 3");
        assert_eq!(delta(10, 2, 3), 4, "Third link of 10/3 should get 4");
    }

    /// Tests the `compute_name_info` helper function.
    ///
    /// C++ uses: `name_info = name_count - 1 - name_index`
    /// This reverses the order so that the delta distribution matches C++
    /// exactly.
    #[test]
    fn test_compute_name_info_helper() {
        // For name_count=2:
        // name_index=0 -> name_info = 2-1-0 = 1
        // name_index=1 -> name_info = 2-1-1 = 0
        assert_eq!(
            compute_name_info(0, 2),
            1,
            "name_index=0 should map to name_info=1 for name_count=2"
        );
        assert_eq!(
            compute_name_info(1, 2),
            0,
            "name_index=1 should map to name_info=0 for name_count=2"
        );

        // For name_count=3:
        // name_index=0 -> name_info = 3-1-0 = 2
        // name_index=1 -> name_info = 3-1-1 = 1
        // name_index=2 -> name_info = 3-1-2 = 0
        assert_eq!(
            compute_name_info(0, 3),
            2,
            "name_index=0 should map to name_info=2 for name_count=3"
        );
        assert_eq!(
            compute_name_info(1, 3),
            1,
            "name_index=1 should map to name_info=1 for name_count=3"
        );
        assert_eq!(
            compute_name_info(2, 3),
            0,
            "name_index=2 should map to name_info=0 for name_count=3"
        );

        // Edge cases for compute_name_info
        // Single hardlink: always returns 0
        assert_eq!(compute_name_info(0, 1), 0);
        assert_eq!(compute_name_info(0, 0), 0); // total_names=0 treated as 1

        // Out-of-bounds name_index should be clamped
        assert_eq!(compute_name_info(5, 2), 0); // 5 >= 2, clamped to 1, result = 2-1-1 = 0
        assert_eq!(compute_name_info(100, 3), 0); // 100 >= 3, clamped to 2, result = 3-1-2 = 0
    }

    /// Tests that the combined transformation + delta gives correct
    /// distribution.
    ///
    /// This is the critical test: when iterating children in order
    /// (`name_index` 0, 1, ...), the transformed `name_info` should produce
    /// the same delta values as C++.
    #[test]
    fn test_transformed_delta_distribution() {
        // For a file with 2 hardlinks and size 5:
        // C++ iterates name_index 0, 1 and transforms to name_info 1, 0
        // So the first visited link (name_index=0) gets delta(5, 1, 2) = 3
        // And the second visited link (name_index=1) gets delta(5, 0, 2) = 2
        let value: u64 = 5;
        let name_count: u32 = 2;

        let name_info_0 = compute_name_info(0, name_count);
        assert_eq!(delta(value, name_info_0, name_count), 3);

        let name_info_1 = compute_name_info(1, name_count);
        assert_eq!(delta(value, name_info_1, name_count), 2);

        // Total should still be 5
        assert_eq!(
            delta(value, name_info_0, name_count) + delta(value, name_info_1, name_count),
            value
        );
    }

    /// Tests edge cases for the delta function.
    #[test]
    fn test_delta_edge_cases() {
        // Single hardlink: should return the full value
        assert_eq!(delta(100, 0, 1), 100);

        // Zero value: should return 0 for all links
        assert_eq!(delta(0, 0, 2), 0);
        assert_eq!(delta(0, 1, 2), 0);

        // total_names = 0 is treated as 1 (early return)
        assert_eq!(delta(100, 0, 0), 100);
    }

    /// Tests that the helper function matches the exact C++ test case from the
    /// user's analysis document.
    #[test]
    fn delta_matches_cpp_and_name_info_mapping() {
        // Example that distinguishes the "shortcut" from exact C++,
        // and also distinguishes raw name_index vs reversed name_info.
        let value = 5_u64;
        let total_names = 2_u32;

        // C++ delta by i:
        assert_eq!(delta(value, 0, total_names), 2);
        assert_eq!(delta(value, 1, total_names), 3);

        // C++ mapping: name_info = (n-1-name_index)
        // name_index=0 => name_info=1 => gets 3
        // name_index=1 => name_info=0 => gets 2
        let name_info0 = compute_name_info(0, total_names);
        let name_info1 = compute_name_info(1, total_names);

        assert_eq!(name_info0, 1);
        assert_eq!(name_info1, 0);

        assert_eq!(delta(value, name_info0, total_names), 3);
        assert_eq!(delta(value, name_info1, total_names), 2);
    }
}
