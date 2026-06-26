// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! [`ChildrenIndex`] — CSR parent→children adjacency, the read-side of the
//! tree walk + the Phase-1 subtree path-length propagation.

use crate::compact::CompactRecord;

/// Children index in CSR (Compressed Sparse Row) layout.
///
/// `children(i)` returns the compact indices of record i's children as
/// a contiguous `&[u32]` slice.  The CSR layout avoids per-record `Vec`
/// allocations and enables bulk serialization/deserialization.
#[derive(Clone)]
pub struct ChildrenIndex {
    /// CSR offsets — one per record + sentinel.  Length = `record_count` + 1.
    /// Children of record `i` are `values[offsets[i]..offsets[i+1]]`.
    offsets: Vec<u32>,
    /// Flat array of all child indices.
    values: Vec<u32>,
}

impl ChildrenIndex {
    /// Total heap capacity (offsets + values) in bytes.
    #[must_use]
    pub const fn heap_size_bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.values.capacity() * size_of::<u32>()
    }

    /// Build from `CompactRecord::parent_idx` in two passes (count + scatter).
    #[must_use]
    pub fn build(records: &[CompactRecord]) -> Self {
        // Count children per parent
        let mut counts = vec![0_u32; records.len()];
        for rec in records {
            let parent = rec.parent_idx;
            if parent != u32::MAX
                && let Some(cnt) = counts.get_mut(parent as usize)
            {
                *cnt += 1;
            }
        }

        // Prefix-sum → offsets
        let mut offsets = Vec::with_capacity(records.len() + 1);
        let mut running = 0_u32;
        for &cnt in &counts {
            offsets.push(running);
            running = running.saturating_add(cnt);
        }
        offsets.push(running);

        // Scatter children into values
        let mut values = vec![0_u32; running as usize];
        let mut write_pos = offsets.clone();
        for (idx, rec) in records.iter().enumerate() {
            let parent = rec.parent_idx;
            if parent != u32::MAX
                && let Some(pos) = write_pos.get_mut(parent as usize)
                && let Some(slot) = values.get_mut(*pos as usize)
            {
                let child_idx = uffs_mft::len_to_u32(idx);
                *slot = child_idx;
                *pos += 1;
            }
        }

        Self { offsets, values }
    }

    /// Construct directly from pre-built CSR arrays (cache deserialization).
    #[must_use]
    pub const fn from_csr(offsets: Vec<u32>, values: Vec<u32>) -> Self {
        Self { offsets, values }
    }

    /// Borrow the CSR components for serialization.
    #[must_use]
    pub(crate) fn as_csr(&self) -> (&[u32], &[u32]) {
        (&self.offsets, &self.values)
    }

    /// Return the children of record `idx` as a contiguous slice.
    #[must_use]
    pub fn get(&self, idx: usize) -> &[u32] {
        let start = self.offsets.get(idx).copied().unwrap_or(0) as usize;
        let end = self.offsets.get(idx + 1).copied().unwrap_or(0) as usize;
        self.values.get(start..end).unwrap_or(&[])
    }

    /// Total number of child entries across all records.
    #[must_use]
    pub const fn total_children(&self) -> usize {
        self.values.len()
    }

    /// Number of records tracked (one slot per record).
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Create an empty children index.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            offsets: vec![0],
            values: Vec::new(),
        }
    }
}
