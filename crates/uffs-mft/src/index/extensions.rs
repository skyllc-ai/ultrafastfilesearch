// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Extension interning and posting lists used by index queries.

use alloc::sync::Arc;

use super::{MftIndex, NO_ENTRY, len_to_u16, len_to_u32};

/// Extension interning table for O(1) lookups and statistics.
#[derive(Debug, Clone, Default)]
pub struct ExtensionTable {
    /// Extension strings (`extension_id` → `Arc<str>`).
    pub names: Vec<Arc<str>>,
    /// File counts per extension (`extension_id` → count).
    pub counts: Vec<u32>,
    /// Total bytes per extension (`extension_id` → bytes).
    pub bytes: Vec<u64>,
    /// Reverse lookup: extension → `extension_id`.
    pub map: std::collections::HashMap<Arc<str>, u16>,
}

impl ExtensionTable {
    /// Create a new extension table with the reserved "no extension" entry.
    #[must_use]
    pub fn new() -> Self {
        let mut table = Self {
            names: Vec::new(),
            counts: Vec::new(),
            bytes: Vec::new(),
            map: std::collections::HashMap::new(),
        };

        let no_ext: Arc<str> = Arc::from("");
        table.names.push(Arc::clone(&no_ext));
        table.counts.push(0);
        table.bytes.push(0);
        table.map.insert(no_ext, 0);
        table
    }

    /// Intern an extension and return its ID.
    pub fn intern(&mut self, extension: &str) -> u16 {
        if extension.is_empty() {
            return 0;
        }

        let normalized = extension.trim_start_matches('.').to_lowercase();
        if normalized.is_empty() {
            return 0;
        }

        let ext_arc: Arc<str> = Arc::from(normalized.as_str());
        if let Some(&id) = self.map.get(&ext_arc) {
            return id;
        }

        let id = len_to_u16(self.names.len());
        if id == u16::MAX {
            return 0;
        }

        self.names.push(Arc::clone(&ext_arc));
        self.counts.push(0);
        self.bytes.push(0);
        self.map.insert(ext_arc, id);
        id
    }

    /// Record a file with the given extension and size.
    pub fn record_file(&mut self, extension_id: u16, file_size: u64) {
        let idx = extension_id as usize;
        if let (Some(count), Some(bytes)) = (self.counts.get_mut(idx), self.bytes.get_mut(idx)) {
            *count += 1;
            *bytes += file_size;
        }
    }

    /// Get the extension string for a given ID.
    #[must_use]
    pub(crate) fn get_extension(&self, extension_id: u16) -> Option<&str> {
        self.names
            .get(extension_id as usize)
            .map(|ext_arc: &Arc<str>| ext_arc.as_ref())
    }

    /// Get the file count for a given extension ID.
    #[must_use]
    pub(crate) fn get_count(&self, extension_id: u16) -> u32 {
        self.counts.get(extension_id as usize).copied().unwrap_or(0)
    }

    /// Get the total bytes for a given extension ID.
    #[must_use]
    pub(crate) fn get_bytes(&self, extension_id: u16) -> u64 {
        self.bytes.get(extension_id as usize).copied().unwrap_or(0)
    }

    /// Get the total number of unique extensions (including "no extension").
    #[must_use]
    pub const fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns true if the table is empty (only has the "no extension" entry).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.names.len() <= 1
    }

    /// Get top N extensions by total bytes.
    #[must_use]
    pub fn top_by_bytes(&self, limit: usize) -> Vec<(u16, &str, u64, u32)> {
        let mut entries: Vec<(u16, &str, u64, u32)> = (0..self.names.len())
            .filter_map(|idx| {
                let ext_id = len_to_u16(idx);
                let ext_str = self.names.get(idx)?.as_ref();
                let bytes = self.bytes.get(idx).copied().unwrap_or(0);
                let count = self.counts.get(idx).copied().unwrap_or(0);
                Some((ext_id, ext_str, bytes, count))
            })
            .collect();

        entries.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.2));
        entries.truncate(limit);
        entries
    }

    /// Get top N extensions by file count.
    #[must_use]
    pub fn top_by_count(&self, limit: usize) -> Vec<(u16, &str, u32, u64)> {
        let mut entries: Vec<(u16, &str, u32, u64)> = (0..self.names.len())
            .filter_map(|idx| {
                let ext_id = len_to_u16(idx);
                let ext_str = self.names.get(idx)?.as_ref();
                let count = self.counts.get(idx).copied().unwrap_or(0);
                let bytes = self.bytes.get(idx).copied().unwrap_or(0);
                Some((ext_id, ext_str, count, bytes))
            })
            .collect();

        entries.sort_unstable_by_key(|entry| core::cmp::Reverse(entry.2));
        entries.truncate(limit);
        entries
    }
}

/// Extension index using Compressed Sparse Row (CSR) format.
#[derive(Debug, Default, Clone)]
pub struct ExtensionIndex {
    /// CSR offsets: `offsets[extension_id]` gives the start index in
    /// `postings`, and `offsets[extension_id + 1]` gives the exclusive end.
    pub offsets: Vec<u32>,
    /// CSR postings: record indices for each extension.
    pub postings: Vec<u32>,
}

impl ExtensionIndex {
    /// Build the extension index from an `MftIndex`.
    #[must_use]
    pub fn build(index: &MftIndex) -> Self {
        let num_extensions = index.extensions.len();
        let mut counts = vec![0_u32; num_extensions];

        for record in &index.records {
            let ext_id = record.first_name.name.extension_id() as usize;
            if let Some(count) = counts.get_mut(ext_id) {
                *count += 1;
            }

            if record.name_count > 1 {
                let mut link_idx = record.first_name.next_entry;
                while link_idx != NO_ENTRY {
                    if let Some(link) = index.links.get(link_idx as usize) {
                        let link_ext_id = link.name.extension_id() as usize;
                        if let Some(count) = counts.get_mut(link_ext_id) {
                            *count += 1;
                        }
                        link_idx = link.next_entry;
                    } else {
                        break;
                    }
                }
            }
        }

        let mut offsets = Vec::with_capacity(num_extensions + 1);
        offsets.push(0);

        let mut sum = 0_u32;
        for count in &counts {
            sum += count;
            offsets.push(sum);
        }

        let total_postings = sum as usize;
        let mut postings = vec![0_u32; total_postings];
        let mut write_pos = offsets.clone();

        for (record_idx, record) in index.records.iter().enumerate() {
            let ext_id = record.first_name.name.extension_id() as usize;
            if let Some(&pos_u32) = write_pos.get(ext_id) {
                let pos = pos_u32 as usize;
                if let Some(posting_slot) = postings.get_mut(pos) {
                    *posting_slot = len_to_u32(record_idx);
                    if let Some(write_slot) = write_pos.get_mut(ext_id) {
                        *write_slot += 1;
                    }
                }
            }

            if record.name_count > 1 {
                let mut link_idx = record.first_name.next_entry;
                while link_idx != NO_ENTRY {
                    if let Some(link) = index.links.get(link_idx as usize) {
                        let link_ext_id = link.name.extension_id() as usize;
                        if let Some(&pos_u32) = write_pos.get(link_ext_id) {
                            let pos = pos_u32 as usize;
                            if let Some(posting_slot) = postings.get_mut(pos) {
                                *posting_slot = len_to_u32(record_idx);
                                if let Some(write_slot) = write_pos.get_mut(link_ext_id) {
                                    *write_slot += 1;
                                }
                            }
                        }
                        link_idx = link.next_entry;
                    } else {
                        break;
                    }
                }
            }
        }

        Self { offsets, postings }
    }

    /// Get record indices for a given `extension_id`.
    #[must_use]
    pub fn get_records(&self, extension_id: u16) -> &[u32] {
        let ext_id = extension_id as usize;
        if let (Some(&start_u32), Some(&end_u32)) =
            (self.offsets.get(ext_id), self.offsets.get(ext_id + 1))
        {
            let start = start_u32 as usize;
            let end = end_u32 as usize;
            self.postings.get(start..end).unwrap_or(&[])
        } else {
            &[]
        }
    }

    /// Get the number of files with a given extension.
    #[must_use]
    pub fn count(&self, extension_id: u16) -> usize {
        let ext_id = extension_id as usize;
        if let (Some(&start), Some(&end)) = (self.offsets.get(ext_id), self.offsets.get(ext_id + 1))
        {
            (end - start) as usize
        } else {
            0
        }
    }

    /// Returns true if the index is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.postings.is_empty()
    }

    /// Returns the total number of postings.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.postings.len()
    }
}
