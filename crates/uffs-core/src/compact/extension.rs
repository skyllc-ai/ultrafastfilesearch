// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! [`ExtensionIndex`] — CSR `extension_id` → records inverted index for O(K)
//! `--ext` queries.

use crate::compact::CompactRecord;

/// Extension inverted index: `extension_id → &[u32]` (record indices).
///
/// CSR layout identical to `ChildrenIndex`.  Built once at load time in a
/// single O(N) pass so `--ext rs` queries can iterate only matching records
/// instead of scanning all 25M entries.
#[derive(Clone)]
pub struct ExtensionIndex {
    /// CSR offsets — length = `max_ext_id` + 2 (one per `ext_id` + sentinel).
    offsets: Vec<u32>,
    /// Flat array of record indices, grouped by `extension_id`.
    values: Vec<u32>,
}

impl ExtensionIndex {
    /// Total heap capacity (offsets + values) in bytes.
    #[must_use]
    pub const fn heap_size_bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.values.capacity() * size_of::<u32>()
    }

    /// Build from compact records in two passes (count + scatter).
    #[must_use]
    pub fn build(records: &[CompactRecord]) -> Self {
        // Find the maximum extension_id to size the offsets array.
        let max_id = records
            .iter()
            .map(|rec| rec.extension_id)
            .max()
            .unwrap_or(0) as usize;

        // Pass 1: count records per extension_id.
        let mut counts = vec![0_u32; max_id + 1];
        for rec in records {
            if rec.name_len == 0 {
                continue;
            }
            if let Some(cnt) = counts.get_mut(rec.extension_id as usize) {
                *cnt += 1;
            }
        }

        // Prefix-sum → offsets.
        let mut offsets = Vec::with_capacity(max_id + 2);
        let mut running = 0_u32;
        for &cnt in &counts {
            offsets.push(running);
            running = running.saturating_add(cnt);
        }
        offsets.push(running);

        // Pass 2: scatter record indices into values.
        let mut values = vec![0_u32; running as usize];
        let mut write_pos = offsets.clone();
        for (idx, rec) in records.iter().enumerate() {
            if rec.name_len == 0 {
                continue;
            }
            let eid = rec.extension_id as usize;
            if let Some(pos) = write_pos.get_mut(eid)
                && let Some(slot) = values.get_mut(*pos as usize)
            {
                let idx_u32 = uffs_mft::len_to_u32(idx);
                *slot = idx_u32;
                *pos += 1;
            }
        }

        Self { offsets, values }
    }

    /// Return record indices for the given `extension_id`.
    #[must_use]
    pub fn get(&self, ext_id: u16) -> &[u32] {
        let eid = ext_id as usize;
        let start = self.offsets.get(eid).copied().unwrap_or(0) as usize;
        let end = self.offsets.get(eid + 1).copied().unwrap_or(0) as usize;
        self.values.get(start..end).unwrap_or(&[])
    }

    /// Create an empty extension index.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            offsets: vec![0],
            values: Vec::new(),
        }
    }

    /// Total number of indexed record entries.
    #[must_use]
    pub const fn total_entries(&self) -> usize {
        self.values.len()
    }
}
