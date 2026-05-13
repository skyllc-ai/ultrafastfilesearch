// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Directory child-link maintenance and stable child ordering helpers.

use super::{ChildInfo, FileRecord, MftIndex, NO_ENTRY, frs_to_usize, len_to_u16, len_to_u32};

impl MftIndex {
    /// Add a child entry to a parent directory.
    ///
    /// This creates a `ChildInfo` entry linking the child to the parent.
    /// Used for building parent-child relationships for tree metrics.
    ///
    /// # Arguments
    /// * `parent_frs` - FRS of the parent directory
    /// * `child_frs` - FRS of the child file/directory
    /// * `name_index` - Which hardlink this is (0 for primary, 1+ for
    ///   additional)
    ///
    /// If the parent record does not exist yet, this creates a placeholder
    /// record so child entries are preserved even when chunks are processed
    /// out of order.
    pub(crate) fn add_child_entry(&mut self, parent_frs: u64, child_frs: u64, name_index: u16) {
        // Create a parent placeholder if it does not exist yet so
        // child edges are not dropped during out-of-order processing.
        let parent_frs_usize = frs_to_usize(parent_frs);

        // Expand lookup table if needed
        if parent_frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(parent_frs_usize + 1, NO_ENTRY);
        }

        // Get or create parent record index
        let Some(frs_slot) = self.frs_to_idx.get_mut(parent_frs_usize) else {
            return;
        };
        let parent_idx = if *frs_slot == NO_ENTRY {
            let new_idx = len_to_u32(self.records.len());
            *frs_slot = new_idx;
            self.records.push(FileRecord::new(parent_frs));
            new_idx as usize
        } else {
            *frs_slot as usize
        };

        // Create child entry
        let child_idx = len_to_u32(self.children.len());
        let Some(parent_rec) = self.records.get_mut(parent_idx) else {
            return;
        };
        let old_first_child = parent_rec.first_child;
        parent_rec.first_child = child_idx;

        self.children.push(ChildInfo {
            next_entry: old_first_child,
            _pad0: [0; 4],
            child_frs,
            name_index,
            _pad1: [0; 6],
        });
    }

    /// Rebuilds `first_child` linked lists from `FILE_NAME` parent references.
    ///
    /// This is a self-heal for LIVE pipelines where child lists may be
    /// incomplete due to ordering / placeholder timing. It is deterministic
    /// and based only on already-parsed `parent_frs` links.
    ///
    /// # Algorithm
    ///
    /// 1. Collect `(parent_frs, child_frs, name_index)` edges from all records
    /// 2. Clear existing children and reset `first_child` pointers
    /// 3. Rebuild using `add_child_entry()`
    ///
    /// # When to Use
    ///
    /// Call this as a fallback if tree metrics computation leaves directories
    /// with `descendants == 0`, which indicates the child graph was incomplete.
    pub(crate) fn rebuild_children_from_names(&mut self) {
        tracing::debug!(
            records = self.records.len(),
            "[TRIP] MftIndex::rebuild_children_from_names ENTER"
        );

        // Phase 1: collect (parent_frs, child_frs, name_index) edges.
        let edges = self.collect_parent_child_edges();

        tracing::debug!(
            edges_collected = edges.len(),
            "[TRIP] MftIndex::rebuild_children_from_names -> Phase 1 done"
        );

        // Phase 2: reset child lists and rebuild.
        self.children.clear();
        for rec in &mut self.records {
            rec.first_child = NO_ENTRY;
        }

        for (parent_frs, child_frs, name_index) in edges {
            self.add_child_entry(parent_frs, child_frs, name_index);
        }
        tracing::debug!(
            children = self.children.len(),
            "[TRIP] MftIndex::rebuild_children_from_names EXIT"
        );
    }

    /// Walk every record's link chain and collect `(parent_frs, child_frs,
    /// name_index)` edges for child-list reconstruction.
    fn collect_parent_child_edges(&self) -> Vec<(u64, u64, u16)> {
        let no_entry_frs: u64 = u64::from(NO_ENTRY);
        let mut edges: Vec<(u64, u64, u16)> =
            Vec::with_capacity(self.records.len().saturating_mul(2));

        for rec in &self.records {
            let child_frs = rec.frs;
            let name_count = usize::from(rec.name_count);

            let mut current_link = rec.first_name;
            for name_index in 0..name_count {
                let parent_frs = current_link.parent_frs;

                // Skip missing/placeholder parents and self-references
                // (root has parent == self).
                if parent_frs != no_entry_frs && parent_frs != child_frs {
                    // Remap list index → parse index (link chain is stored
                    // in reverse encounter order).
                    let parse_index = len_to_u16(name_count - 1 - name_index);
                    edges.push((parent_frs, child_frs, parse_index));
                }

                if current_link.next_entry == NO_ENTRY {
                    break;
                }
                if let Some(next_link) = self.links.get(current_link.next_entry as usize) {
                    current_link = *next_link;
                } else {
                    break;
                }
            }
        }
        edges
    }
    /// Sort children within each directory by filename (case-insensitive).
    ///
    /// This method sorts the children of each directory in the index by their
    /// primary filename using case-insensitive comparison. The sorting uses an
    /// ASCII fast path for zero-allocation comparison when possible.
    ///
    /// This should be called once after the index is fully built (after parsing
    /// or after merging fragments) to provide natural sorted directory
    /// listings.
    ///
    /// # Performance
    ///
    /// - ASCII filenames: Zero allocations per comparison (~10-100x faster)
    /// - Non-ASCII filenames: Falls back to `to_lowercase()` (rare)
    /// - Overhead: ~10-50 ms per 1M files (negligible)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut index = MftIndex::new('C');
    /// // ... parse MFT records ...
    /// index.sort_directory_children(); // Sort all directory children
    /// ```
    pub(crate) fn sort_directory_children(&mut self) {
        // Temporary buffer for collecting children (reused across directories)
        let mut child_indices: Vec<u32> = Vec::new();

        // Iterate through all records to find directories
        for record_idx in 0..self.records.len() {
            let (is_dir, first_child) = if let Some(rec) = self.records.get(record_idx) {
                (rec.is_directory(), rec.first_child)
            } else {
                continue;
            };

            // Skip non-directories
            if !is_dir {
                continue;
            }

            // Skip directories with no children
            if first_child == NO_ENTRY {
                continue;
            }

            // Collect all children from the linked list
            child_indices.clear();
            let mut current_idx = first_child;
            while current_idx != NO_ENTRY {
                child_indices.push(current_idx);
                if let Some(child) = self.children.get(current_idx as usize) {
                    current_idx = child.next_entry;
                } else {
                    break;
                }
            }

            // Skip if only one child (already sorted)
            if child_indices.len() <= 1 {
                continue;
            }

            // Sort children by filename (case-insensitive, zero-allocation for ASCII)
            child_indices.sort_by(|&idx_a, &idx_b| {
                // Get child info entries
                let (Some(child_a), Some(child_b)) = (
                    self.children.get(idx_a as usize),
                    self.children.get(idx_b as usize),
                ) else {
                    return core::cmp::Ordering::Equal;
                };

                // Get child records
                let rec_a = self
                    .frs_to_idx_opt(child_a.child_frs)
                    .and_then(|idx| self.records.get(idx));
                let rec_b = self
                    .frs_to_idx_opt(child_b.child_frs)
                    .and_then(|idx| self.records.get(idx));

                // Get filenames (use appropriate name index for hard links)
                let name_a = rec_a
                    .and_then(|rec| self.get_link_at(rec, child_a.name_index))
                    .map_or("", |link| self.get_name(link.name));
                let name_b = rec_b
                    .and_then(|rec| self.get_link_at(rec, child_b.name_index))
                    .map_or("", |link| self.get_name(link.name));

                // Compare case-insensitively (zero allocations for ASCII)
                // Inlined from cmp_ascii_case_insensitive to satisfy clippy::single_call_fn
                if name_a.is_ascii() && name_b.is_ascii() {
                    // Fast path: both strings are ASCII
                    name_a
                        .bytes()
                        .map(|byte| byte.to_ascii_lowercase())
                        .cmp(name_b.bytes().map(|byte| byte.to_ascii_lowercase()))
                } else {
                    // Slow path: at least one string contains non-ASCII characters
                    name_a.to_lowercase().cmp(&name_b.to_lowercase())
                }
            });

            // Rebuild the linked list in sorted order
            for (idx, &current_child_idx) in child_indices.iter().enumerate() {
                let next_child_idx = child_indices.get(idx + 1).copied().unwrap_or(NO_ENTRY);

                // Update the next_entry pointer
                if let Some(child) = self.children.get_mut(current_child_idx as usize) {
                    child.next_entry = next_child_idx;
                }
            }

            // Update the directory's first_child to point to the new head
            if let (Some(first_idx), Some(dir_record)) = (
                child_indices.first().copied(),
                self.records.get_mut(record_idx),
            ) {
                dir_record.first_child = first_idx;
            }
        }
    }
}
