//! C++-port tree metrics computation.
//!
//! This module implements the tree-metrics algorithm used for parity with the
//! C++ reference implementation when `--tree-algo=cpp` is selected.
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
#[inline]
const fn delta(value: u64, name_info: u32, total_names: u32) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let n64 = total_names as u64;
    let i64 = name_info as u64;
    value * (i64 + 1) / n64 - value * i64 / n64
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
}

impl CppTreeTraversal<'_> {
    /// Runs the tree traversal starting from ROOT, then sweeps orphans.
    fn run(&mut self) {
        // Canonical NTFS root directory record number.
        const ROOT_FRS: u64 = 5;

        // Primary traversal from ROOT (if present).
        if let Some(root_idx) = self.index.frs_to_idx_opt(ROOT_FRS) {
            // Root has a single visible path entry in output context.
            let _: Agg = self.preprocess(root_idx, 0, 1);
        } else if self.debug {
            tracing::warn!(
                "[cpp_tree] WARNING: ROOT_FRS=5 not present in frs_to_idx; running orphan sweep only"
            );
        }

        // Orphan sweep: ensure every record has its printed tree metrics initialized.
        // This prevents LIVE scans from leaving some directories with Size/Desc = 0
        // due to transient linkage gaps.
        for idx in 0..self.index.records.len() {
            if !self.seen[idx] {
                let _: Agg = self.preprocess(idx, 0, 1);
            }
        }
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
                    let child_total_names = u32::from(self.index.records[child_idx].name_count);
                    let child_name_info = u32::from(child_name_idx);

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

                // Printed directory size/allocated:
                //   - Children Channel-A size + ONLY the directory's first/default stream size.
                //   - Excludes the directory's internal streams and overflow streams from its
                //     *own* row, but they still flow to the parent through `result`.
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
pub fn compute_tree_metrics_cpp_port(index: &mut MftIndex, debug: bool) {
    let seen = vec![false; index.records.len()];
    let mut traversal = CppTreeTraversal { index, seen, debug };
    traversal.run();
}
