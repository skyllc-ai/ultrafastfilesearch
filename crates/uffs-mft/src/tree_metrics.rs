// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tree metrics computation.
//!
//! This module implements the tree-metrics algorithm used for parity with the
//! reference implementation. This is the sole tree-metrics code path.
//!
//! Key properties:
//! - **Two-channel model**: Channel A is used for propagation up the tree
//!   (includes internal streams), while Channel B is what gets printed for
//!   directory rows (excludes internal streams).
//! - **Per-stream hardlink delta**: sizes are distributed per stream using the
//!   exact floor-division delta formula.
//! - **Orphan sweep**: after traversing from the NTFS root (FRS 5), we sweep
//!   any unvisited records to ensure LIVE scans don't leave some directories
//!   with zeroed metrics.
//!
//! Notes:
//! - This code intentionally uses indexed access into internal vectors; indices
//!   are produced by the index builder and are expected to be valid.
#![expect(
    clippy::indexing_slicing,
    reason = "tree metrics rely on validated index-builder offsets"
)]

// AUTO-GENERATED DROP-IN REPLACEMENT
// This file fixes LIVE/ONLINE parity gaps by ensuring the tree metrics:
// - Use the two-channel model (propagation vs printed metrics)
// - Distribute internal stream deltas *per stream* (not as a single aggregate)
// - Always initialize every record (orphan sweep) so LIVE scans never leave
//   Size/Desc = 0
//
// Intended for: crates/uffs-mft/src/tree_metrics.rs
//
// Notes:
// - We intentionally do NOT memoize `preprocess` results because the delta
//   distribution depends on (name_info, total_names). Caching by record would
//   break hardlink accounting.
// - The "treesize" field in the returned aggregate is the Channel-A
//   stream-count (counts streams in the subtree, including internal streams and
//   ADS, and can exceed row-count).

use crate::index::{MftIndex, NO_ENTRY};

/// Snapshot of the per-record fields needed for stream accumulation.
///
/// Avoids borrowing `self.index.records` while calling helper methods that
/// also need `&self`.
struct RecordSnapshot {
    /// Default stream length.
    first_len: u64,
    /// Default stream allocated (may include merged `WoF` allocated).
    first_alloc: u64,
    /// Hardlink share index.
    name_info: u32,
    /// Total hardlink count (≥ 1).
    total_names: u32,
    /// Head of internal stream chain, or `NO_ENTRY`.
    first_internal_stream: u32,
    /// Aggregate internal stream size (fallback when chain unavailable).
    internal_streams_size: u64,
    /// Aggregate internal stream allocated (fallback).
    internal_streams_allocated: u64,
    /// Head of overflow user-visible stream chain, or `NO_ENTRY`.
    first_stream_next: u32,
    /// Index of `WoF` stream to suppress, or `NO_ENTRY`.
    wof_stream_idx: u32,
}

/// Computes the delta share for a hardlink using the exact floor-division
/// formula.
///
/// Distributes `value` across `total_names` hardlinks, returning the share for
/// hardlink index `name_info`. This matches the reference implementation
/// exactly.
///
/// **Important**: `name_info` must be the transformed index, NOT the raw
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

/// Computes the transformed `name_info` from a raw `name_index`.
///
/// Reference parity uses: `name_info = name_count - 1 - name_index`
///
/// This reverses the order so that the delta distribution matches exactly.
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
#[expect(
    clippy::single_call_fn,
    reason = "extracted for parity diagnostic logging"
)]
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
        return 0;
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

/// Immutable snapshot of a record's fields needed by `preprocess`.
/// Avoids holding a `&self.index.records` borrow across recursive calls.
struct PreprocessSnapshot {
    /// Whether this record is a directory.
    is_directory: bool,
    /// First child entry index (or `NO_ENTRY`).
    first_child: u32,
    /// Next entry in the user-visible stream chain.
    first_stream_next: u32,
    /// First internal stream entry.
    first_internal_stream: u32,
    /// Total stream count (user + internal).
    total_stream_count: u16,
    /// Default stream logical size.
    first_len: u64,
    /// Default stream allocated size.
    first_alloc: u64,
    /// Reparse tag (for `WoF` detection).
    reparse_tag: u32,
    /// Aggregate internal streams logical size (cache-loaded fallback).
    internal_streams_size: u64,
    /// Aggregate internal streams allocated size (cache-loaded fallback).
    internal_streams_allocated: u64,
}

/// Traversal state for computing tree metrics.
struct TreeTraversal<'a> {
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
    /// Win32 paths are excluded to match Win32 enumeration behavior.
    skip_orphans: bool,
    /// Current recursion depth (0 = root directory).
    /// Adds `reserved_clusters * cluster_size` to the root's children
    /// allocated at depth 0.
    depth: u32,
    /// Bytes to add to the root's children allocated at depth 0.
    /// NTFS formula: `(TotalReserved + MftZoneEnd - MftZoneStart) *
    /// BytesPerCluster`.
    reserved_allocated_bytes: u64,
}

impl TreeTraversal<'_> {
    /// Runs the tree traversal starting from ROOT, then sweeps orphans.
    fn run(&mut self) {
        tracing::debug!("[TRIP] TreeTraversal::run ENTER");

        self.traverse_from_root();

        if self.skip_orphans {
            let orphan_count = self.seen.iter().filter(|&&seen| !seen).count();
            tracing::debug!(
                orphan_count,
                "[TRIP] TreeTraversal::run EXIT (orphan sweep SKIPPED for parity mode)"
            );
            return;
        }

        self.sweep_orphans();
    }

    /// Start DFS from the NTFS root directory (FRS 5).
    fn traverse_from_root(&mut self) {
        const ROOT_FRS: u64 = 5;

        if let Some(root_idx) = self.index.frs_to_idx_opt(ROOT_FRS) {
            tracing::debug!(root_idx, "[TRIP] starting from ROOT (FRS=5)");
            let _: Agg = self.preprocess(root_idx, 0, 1);
            tracing::debug!("[TRIP] ROOT traversal done");
        } else if self.debug {
            tracing::warn!(
                "[tree_metrics] ROOT_FRS=5 not present in frs_to_idx; running orphan sweep only"
            );
        }
    }

    /// Visit every unseen record to initialize its printed tree metrics.
    fn sweep_orphans(&mut self) {
        tracing::debug!("[TRIP] starting orphan sweep");
        let mut orphan_count = 0_usize;
        for idx in 0..self.index.records.len() {
            if !self.seen[idx] {
                orphan_count += 1;
                let _: Agg = self.preprocess(idx, 0, 1);
            }
        }
        tracing::debug!(orphan_count, "[TRIP] orphan sweep done");
    }

    /// Finds the `WofCompressedData` stream in the named-stream chain.
    ///
    /// Returns the stream index if found, or `NO_ENTRY` if not present.
    /// This is only called for files with `reparse_tag == 0x8000_0017`.
    fn find_wof_stream(&self, first_stream_next: u32) -> u32 {
        const WOF_NAME: &str = "WofCompressedData";
        let mut idx = first_stream_next;
        while idx != NO_ENTRY {
            let stream = &self.index.streams[idx as usize];
            if self.index.get_name(stream.name) == WOF_NAME {
                return idx;
            }
            idx = stream.next_entry;
        }
        NO_ENTRY
    }

    /// Merges `WoF` stream's `allocated_size` into the default stream for files
    /// with `reparse_tag == 0x8000_0017`. Returns the `WoF` stream index (for
    /// exclusion from Channel-A propagation) and the updated `first_alloc`.
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: wof_stream_idx < streams.len() (from find_wof_stream)"
    )]
    fn merge_wof_alloc(
        &mut self,
        record_idx: usize,
        reparse_tag: u32,
        first_stream_next: u32,
        mut first_alloc: u64,
    ) -> (u32, u64) {
        let wof_idx = if reparse_tag == 0x8000_0017 {
            self.find_wof_stream(first_stream_next)
        } else {
            NO_ENTRY
        };
        if wof_idx != NO_ENTRY {
            let wof_alloc = self.index.streams[wof_idx as usize].size.allocated;
            first_alloc = first_alloc.saturating_add(wof_alloc);
            // Persist the merged value so output shows the correct SizeOnDisk
            self.index.records[record_idx].first_stream.size.allocated = first_alloc;
        }
        (wof_idx, first_alloc)
    }

    /// Accumulates own-stream sizes (default + internal + overflow ADS),
    /// delta-distributed across hardlinks.
    ///
    /// When `internal_streams` is not populated (cache-loaded index), falls
    /// back to the aggregate `internal_streams_size` /
    /// `internal_streams_allocated` stored on the record.
    fn accumulate_own_streams(&self, snap: &RecordSnapshot) -> (u64, u64) {
        let mut own_len = delta(snap.first_len, snap.name_info, snap.total_names);
        let mut own_alloc = delta(snap.first_alloc, snap.name_info, snap.total_names);

        // Internal streams: per-stream delta when chain is available,
        // aggregate fallback otherwise (cache-loaded path).
        if snap.first_internal_stream != NO_ENTRY && !self.index.internal_streams.is_empty() {
            let mut idx = snap.first_internal_stream;
            while idx != NO_ENTRY {
                let Some(ist) = self.index.internal_streams.get(idx as usize) else {
                    break;
                };
                own_len = own_len.saturating_add(delta(
                    ist.size.length,
                    snap.name_info,
                    snap.total_names,
                ));
                own_alloc = own_alloc.saturating_add(delta(
                    ist.size.allocated,
                    snap.name_info,
                    snap.total_names,
                ));
                idx = ist.next_entry;
            }
        } else if snap.internal_streams_size > 0 || snap.internal_streams_allocated > 0 {
            own_len = own_len.saturating_add(delta(
                snap.internal_streams_size,
                snap.name_info,
                snap.total_names,
            ));
            own_alloc = own_alloc.saturating_add(delta(
                snap.internal_streams_allocated,
                snap.name_info,
                snap.total_names,
            ));
        }

        // Overflow user-visible streams (ADS).
        let mut stream_idx = snap.first_stream_next;
        while stream_idx != NO_ENTRY {
            let Some(stream) = self.index.streams.get(stream_idx as usize) else {
                break;
            };
            // WoF stream: use 0 for both length and allocated in the
            // Channel-A propagation (allocated already merged into first_alloc).
            let is_wof = stream_idx == snap.wof_stream_idx;
            let slen = if is_wof { 0 } else { stream.size.length };
            let salloc = if is_wof { 0 } else { stream.size.allocated };
            own_len = own_len.saturating_add(delta(slen, snap.name_info, snap.total_names));
            own_alloc = own_alloc.saturating_add(delta(salloc, snap.name_info, snap.total_names));
            stream_idx = stream.next_entry;
        }

        (own_len, own_alloc)
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
        let snap = self.snapshot_record(record_idx);

        // WoF (Windows Overlay Filter) compression merging.
        let mut first_alloc = snap.first_alloc;
        let wof_stream_idx;
        (wof_stream_idx, first_alloc) = self.merge_wof_alloc(
            record_idx,
            snap.reparse_tag,
            snap.first_stream_next,
            first_alloc,
        );

        // 1) Aggregate children (Channel A).
        let children = if snap.is_directory {
            self.aggregate_children(snap.first_child)
        } else {
            Agg::default()
        };

        // 2) Own streams (Channel A): default + ADS + internal, delta-distributed.
        let own_snap = RecordSnapshot {
            first_len: snap.first_len,
            first_alloc,
            name_info,
            total_names,
            first_internal_stream: snap.first_internal_stream,
            internal_streams_size: snap.internal_streams_size,
            internal_streams_allocated: snap.internal_streams_allocated,
            first_stream_next: snap.first_stream_next,
            wof_stream_idx,
        };
        let (own_len, own_alloc) = self.accumulate_own_streams(&own_snap);

        let own_stream_count: u32 = u32::from(snap.total_stream_count.max(1));

        let result = Agg {
            length: children.length.saturating_add(own_len),
            allocated: children.allocated.saturating_add(own_alloc),
            treesize: children.treesize.saturating_add(own_stream_count),
        };

        // 3) Store printed metrics (Channel B).
        self.store_printed_metrics(
            record_idx,
            snap.is_directory,
            &children,
            snap.first_len,
            first_alloc,
        );

        result
    }

    /// Snapshot immutable fields from a record to avoid holding a borrow across
    /// recursive calls.
    fn snapshot_record(&self, idx: usize) -> PreprocessSnapshot {
        let rec = &self.index.records[idx];
        PreprocessSnapshot {
            is_directory: rec.stdinfo.is_directory(),
            first_child: rec.first_child,
            first_stream_next: rec.first_stream.next_entry,
            first_internal_stream: rec.first_internal_stream,
            total_stream_count: rec.total_stream_count,
            first_len: rec.first_stream.size.length,
            first_alloc: rec.first_stream.size.allocated,
            reparse_tag: rec.reparse_tag,
            internal_streams_size: rec.internal_streams_size,
            internal_streams_allocated: rec.internal_streams_allocated,
        }
    }

    /// Walk the child chain and recursively preprocess each child, returning
    /// the aggregated Channel-A metrics.
    fn aggregate_children(&mut self, first_child: u32) -> Agg {
        self.depth = self.depth.saturating_add(1);

        let mut agg = Agg::default();
        let mut child_entry_idx = first_child;

        while child_entry_idx != NO_ENTRY {
            let (child_frs, child_name_idx, next_entry) = {
                let ce = &self.index.children[child_entry_idx as usize];
                (ce.child_frs, ce.name_index, ce.next_entry)
            };

            if let Some(child_idx) = self.index.frs_to_idx_opt(child_frs) {
                let child_total_names = u32::from(self.index.records[child_idx].name_count);
                let child_name_info = compute_name_info_checked(
                    u32::from(child_name_idx),
                    child_total_names,
                    child_frs,
                    self.debug,
                );

                let child_agg =
                    self.preprocess(child_idx, child_name_info, child_total_names.max(1));
                agg.length = agg.length.saturating_add(child_agg.length);
                agg.allocated = agg.allocated.saturating_add(child_agg.allocated);
                agg.treesize = agg.treesize.saturating_add(child_agg.treesize);
            }

            child_entry_idx = next_entry;
        }

        self.depth = self.depth.saturating_sub(1);

        // At depth 0 (root) add reserved NTFS cluster allocation.
        if self.depth == 0 && self.reserved_allocated_bytes > 0 {
            agg.allocated = agg.allocated.saturating_add(self.reserved_allocated_bytes);
        }

        agg
    }

    /// Store the final "printed" metrics into the record (Channel B output).
    fn store_printed_metrics(
        &mut self,
        record_idx: usize,
        is_directory: bool,
        children: &Agg,
        first_len: u64,
        first_alloc: u64,
    ) {
        let rec = &mut self.index.records[record_idx];
        if is_directory {
            rec.descendants = children.treesize.saturating_add(1);
            rec.treesize = children.length.saturating_add(first_len);
            rec.tree_allocated = children.allocated.saturating_add(first_alloc);
        } else {
            rec.descendants = 0;
            rec.treesize = first_len;
            rec.tree_allocated = first_alloc;
        }
    }
}

/// Computes tree metrics using the shared parity-preserving algorithm.
///
/// Populates `treesize`, `tree_allocated`, and `descendants` for directory
/// records. If `debug` is true, emits warnings for unexpected index conditions.
///
/// # Arguments
/// * `index` - The MFT index to compute tree metrics for
/// * `debug` - Whether to emit debug tracing
/// * `skip_orphans` - If true, skip orphan sweep for parity mode. Only records
///   reachable from ROOT via Win32-visible `FILE_NAME` edges are included in
///   tree aggregation.
pub fn compute_tree_metrics(index: &mut MftIndex, debug: bool, skip_orphans: bool) {
    tracing::debug!(
        records = index.records.len(),
        skip_orphans,
        "[TRIP] tree_metrics::compute_tree_metrics ENTER"
    );
    let reserved_bytes = index.reserved_allocated_bytes;
    let seen = vec![false; index.records.len()];
    let mut traversal = TreeTraversal {
        index,
        seen,
        debug,
        skip_orphans,
        depth: 0,
        reserved_allocated_bytes: reserved_bytes,
    };
    traversal.run();
    tracing::debug!("[TRIP] tree_metrics::compute_tree_metrics -> traversal.run() done");

    warn_unstamped_directories(index);
    tracing::debug!("[TRIP] tree_metrics::compute_tree_metrics EXIT");
}

/// Diagnostic: every directory should have `descendants >= 1` after tree
/// metrics. A zero value means traversal never stamped it.
/// Runs in release mode to aid live-scan diagnosis.
fn warn_unstamped_directories(index: &MftIndex) {
    for (idx, rec) in index.records.iter().enumerate() {
        if rec.stdinfo.is_directory() && rec.descendants == 0 {
            tracing::warn!(
                frs = rec.frs,
                idx = idx,
                first_child = rec.first_child,
                name_count = rec.name_count,
                is_reparse = rec.stdinfo.is_reparse(),
                reparse_tag = rec.reparse_tag,
                "[tree_metrics] WARNING: Directory has descendants=0 after tree metrics"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the delta function correctly distributes values across
    /// hardlinks.
    ///
    /// The formula is: `delta(v, i, n) = floor(v*(i+1)/n) - floor(v*i/n)`
    /// This ensures the sum of all deltas equals the original value exactly.
    #[test]
    fn delta_sum_equals_original() {
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

    /// Tests specific delta values to ensure reference parity.
    #[test]
    fn delta_specific_values() {
        assert_eq!(delta(5, 0, 2), 2, "First link of 5/2 should get 2");
        assert_eq!(delta(5, 1, 2), 3, "Second link of 5/2 should get 3");

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

        assert_eq!(delta(10, 0, 3), 3, "First link of 10/3 should get 3");
        assert_eq!(delta(10, 1, 3), 3, "Second link of 10/3 should get 3");
        assert_eq!(delta(10, 2, 3), 4, "Third link of 10/3 should get 4");
    }

    /// Tests the `compute_name_info` helper function.
    #[test]
    fn compute_name_info_helper() {
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

        assert_eq!(compute_name_info(0, 1), 0);
        assert_eq!(compute_name_info(0, 0), 0);
        assert_eq!(compute_name_info(5, 2), 0);
        assert_eq!(compute_name_info(100, 3), 0);
    }

    /// Tests that the combined transformation + delta gives correct
    /// distribution.
    #[test]
    fn transformed_delta_distribution() {
        let value: u64 = 5;
        let name_count: u32 = 2;

        let name_info_0 = compute_name_info(0, name_count);
        assert_eq!(delta(value, name_info_0, name_count), 3);

        let name_info_1 = compute_name_info(1, name_count);
        assert_eq!(delta(value, name_info_1, name_count), 2);

        assert_eq!(
            delta(value, name_info_0, name_count) + delta(value, name_info_1, name_count),
            value
        );
    }

    /// Tests edge cases for the delta function.
    #[test]
    fn delta_edge_cases() {
        assert_eq!(delta(100, 0, 1), 100);
        assert_eq!(delta(0, 0, 2), 0);
        assert_eq!(delta(0, 1, 2), 0);
        assert_eq!(delta(100, 0, 0), 100);
    }

    /// Tests that the helper function matches the exact reference mapping.
    #[test]
    fn delta_matches_reference_name_info_mapping() {
        let value = 5_u64;
        let total_names = 2_u32;

        assert_eq!(delta(value, 0, total_names), 2);
        assert_eq!(delta(value, 1, total_names), 3);

        let name_info0 = compute_name_info(0, total_names);
        let name_info1 = compute_name_info(1, total_names);

        assert_eq!(name_info0, 1);
        assert_eq!(name_info1, 0);

        assert_eq!(delta(value, name_info0, total_names), 3);
        assert_eq!(delta(value, name_info1, total_names), 2);
    }
}
