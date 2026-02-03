//! C++ tree metrics port for LIVE/ONLINE parity.
//!
//! This module fixes LIVE/ONLINE parity gaps by ensuring the C++-port tree
//! metrics:
//! - Use the two-channel model (propagation vs printed metrics)
//! - Distribute internal stream deltas *per stream* (not as a single aggregate)
//! - Always initializes every record (orphan sweep) so LIVE scans never leave
//!   Size/Desc = 0
//!
//! Notes:
//! - We intentionally do NOT memoize `preprocess` results because the delta
//!   distribution depends on (`name_info`, `total_names`). Caching by record
//!   would break hardlink accounting.
//! - The "treesize" field in the returned aggregate is the C++ Channel-A
//!   stream-count (counts streams in the subtree, including internal streams
//!   and ADS, and can exceed row-count).

#![allow(clippy::indexing_slicing)] // Bounds checked via record_idx < len and linked list validity

use crate::index::{InternalStreamInfo, MftIndex, NO_ENTRY};

/// Computes the delta share of `value` for hardlink `name_info` out of
/// `total_names`.
///
/// Uses floor-based distribution: `delta(v, i, n) = floor(v*(i+1)/n) -
/// floor(v*i/n)`.
#[inline]
const fn delta(value: u64, name_info: u32, total_names: u32) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let total = total_names as u64;
    let base = value / total;
    let rem = value % total;
    // Note: const fn cannot use u64::from() so we keep `as u64` here.
    base + if (name_info as u64) < rem { 1 } else { 0 }
}

/// Aggregated tree metrics returned by `preprocess` (Channel A values).
#[derive(Clone, Copy, Default)]
struct Agg {
    /// Total logical size of subtree.
    length: u64,
    /// Total allocated size of subtree.
    allocated: u64,
    /// Channel-A stream count in subtree (used to derive printed directory
    /// descendants).
    treesize: u32,
}

/// Internal traversal state for the C++ tree algorithm.
struct CppTreeTraversal<'a> {
    /// Mutable reference to the MFT index being processed.
    index: &'a mut MftIndex,
    /// Marks whether we've visited a record at least once (used for the orphan
    /// sweep).
    seen: Vec<bool>,
    /// Whether to emit debug warnings.
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
                "[cpp_tree] ROOT_FRS=5 not present in frs_to_idx; running orphan sweep only"
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
    /// Returns Channel-A aggregate (propagated to parent).
    fn preprocess(&mut self, record_idx: usize, name_info: u32, total_names_raw: u32) -> Agg {
        if record_idx >= self.index.records.len() {
            return Agg::default();
        }

        self.seen[record_idx] = true;

        let total_names = total_names_raw.max(1);

        // Snapshot only the fields we need (avoid cloning the whole FileRecord).
        let (
            is_directory,
            first_child,
            first_stream_next_entry,
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
                // Extract all needed values from child_entry before calling preprocess
                // to avoid borrow checker issues (preprocess borrows &mut self).
                let (child_frs, child_name_index, next_entry) = {
                    let child_entry = &self.index.children[child_entry_idx as usize];
                    (
                        child_entry.child_frs,
                        child_entry.name_index,
                        child_entry.next_entry,
                    )
                };

                // Resolve child record index from child FRS.
                if let Some(resolved_child_idx) = self.index.frs_to_idx_opt(child_frs) {
                    // Determine which hardlink name of the child this directory entry refers to.
                    let child_total_names =
                        u32::from(self.index.records[resolved_child_idx].name_count);
                    let child_name_info = u32::from(child_name_index);

                    let child_agg = self.preprocess(
                        resolved_child_idx,
                        child_name_info,
                        child_total_names.max(1),
                    );

                    children.length = children.length.saturating_add(child_agg.length);
                    children.allocated = children.allocated.saturating_add(child_agg.allocated);
                    children.treesize = children.treesize.saturating_add(child_agg.treesize);
                }

                child_entry_idx = next_entry;
            }
        }

        // 2) Own streams (Channel A): default stream + ADS + internal streams,
        //    delta-distributed.
        let mut own_len = delta(first_len, name_info, total_names);
        let mut own_alloc = delta(first_alloc, name_info, total_names);

        // Internal streams must be delta-distributed per-stream (rounding correctness).
        let mut internal_idx = first_internal_stream;
        while internal_idx != NO_ENTRY {
            let st: &InternalStreamInfo = &self.index.internal_streams[internal_idx as usize];
            own_len = own_len.saturating_add(delta(st.size.length, name_info, total_names));
            own_alloc = own_alloc.saturating_add(delta(st.size.allocated, name_info, total_names));
            internal_idx = st.next_entry;
        }

        // Overflow user-visible streams (ADS etc), also delta-distributed per stream.
        let mut stream_idx = first_stream_next_entry;
        while stream_idx != NO_ENTRY {
            let st = &self.index.streams[stream_idx as usize];
            own_len = own_len.saturating_add(delta(st.size.length, name_info, total_names));
            own_alloc = own_alloc.saturating_add(delta(st.size.allocated, name_info, total_names));
            stream_idx = st.next_entry;
        }

        // Stream count contribution (Channel A): counts ALL streams on the record (incl
        // internal).
        let own_stream_count = u32::from(total_stream_count.max(1));

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

/// Computes tree metrics using the C++ two-channel algorithm.
///
/// This function traverses the MFT index tree and computes:
/// - `descendants`: count of all items in subtree (for directories)
/// - `treesize`: sum of logical file sizes in subtree
/// - `tree_allocated`: sum of allocated disk sizes in subtree
///
/// The algorithm uses two channels:
/// - Channel A (propagation): values returned by recursion, counts ALL streams
/// - Channel B (printed): values stored for output, only counts directory
///   stream
pub fn compute_tree_metrics_cpp_port(index: &mut MftIndex, debug: bool) {
    let seen = vec![false; index.records.len()];
    let mut traversal = CppTreeTraversal { index, seen, debug };
    traversal.run();
}
