// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Name iteration and Windows path materialization for index records.
//!
//! Exception: `PathResolver` is a self-contained ~490 LOC struct. Scheduled for
//! extraction to `index/path_resolver.rs` in Wave 5 — see
//! `docs/architecture/FILE_SIZE_REFACTOR_WAVES.md`.

use super::{FileRecord, IndexStreamInfo, LinkInfo, MftIndex, NO_ENTRY};
use crate::frs::{Frs, ParentFrs};

// ============================================================================
// Name/Stream Iteration (for hard link and ADS expansion)
// ============================================================================

/// Iterator over all names (hard links) for a record.
pub struct NameIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first name (inline in the record), consumed on first iteration.
    first: Option<&'a LinkInfo>,
    /// Index into `index.links` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for NameIter<'a> {
    type Item = (u16, &'a LinkInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_name
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let link = self.index.links.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = link.next_entry;
        Some((idx, link))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // We don't know the exact count without traversing
        (1, None)
    }
}

/// Iterator over all streams for a record.
pub struct StreamIter<'a> {
    /// Reference to the index for linked list traversal.
    index: &'a MftIndex,
    /// The first stream (inline in the record), consumed on first iteration.
    first: Option<&'a IndexStreamInfo>,
    /// Index into `index.streams` for the next entry, or `NO_ENTRY` if done.
    next_entry: u32,
    /// Current iteration index (0-based).
    idx: u16,
}

impl<'a> Iterator for StreamIter<'a> {
    type Item = (u16, &'a IndexStreamInfo);

    fn next(&mut self) -> Option<Self::Item> {
        // First iteration: return the inline first_stream
        if let Some(first) = self.first.take() {
            let idx = self.idx;
            self.idx += 1;
            self.next_entry = first.next_entry;
            return Some((idx, first));
        }

        // Subsequent iterations: follow the linked list
        if self.next_entry == NO_ENTRY {
            return None;
        }

        let stream = self.index.streams.get(self.next_entry as usize)?;
        let idx = self.idx;
        self.idx += 1;
        self.next_entry = stream.next_entry;
        Some((idx, stream))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (1, None)
    }
}

impl MftIndex {
    /// Iterate over all names (hard links) for a record.
    ///
    /// Most files have only one name (the primary), but files with hard links
    /// will have multiple entries. Each name has its own parent directory.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "iterator construction is not const-compatible"
    )]
    pub fn iter_names<'a>(&'a self, record: &'a FileRecord) -> NameIter<'a> {
        NameIter {
            index: self,
            first: Some(&record.first_name),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Iterate over all streams for a record.
    ///
    /// Most files have only the default `$DATA` stream, but files with
    /// Alternate Data Streams (ADS) will have multiple entries.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "iterator construction is not const-compatible"
    )]
    pub fn iter_streams<'a>(&'a self, record: &'a FileRecord) -> StreamIter<'a> {
        StreamIter {
            index: self,
            first: Some(&record.first_stream),
            next_entry: NO_ENTRY,
            idx: 0,
        }
    }

    /// Get the stream name (empty for default `$DATA` stream).
    #[must_use]
    pub fn stream_name(&self, stream: &IndexStreamInfo) -> &str {
        self.get_name(stream.name)
    }

    /// Get the Nth name (hard link) for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_name_at<'a>(&'a self, record: &'a FileRecord, idx: u16) -> Option<&'a LinkInfo> {
        if idx == 0 {
            return Some(&record.first_name);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_name.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.links.get(current as usize);
            }
            if let Some(link) = self.links.get(current as usize) {
                current = link.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Get the Nth stream for a record.
    ///
    /// Returns `None` if the index is out of bounds.
    #[must_use]
    pub fn get_stream_at<'a>(
        &'a self,
        record: &'a FileRecord,
        idx: u16,
    ) -> Option<&'a IndexStreamInfo> {
        if idx == 0 {
            return Some(&record.first_stream);
        }

        // Walk the linked list to find the Nth entry
        let mut current = record.first_stream.next_entry;
        let mut current_idx = 1_u16;

        while current != NO_ENTRY {
            if current_idx == idx {
                return self.streams.get(current as usize);
            }
            if let Some(stream) = self.streams.get(current as usize) {
                current = stream.next_entry;
                current_idx += 1;
            } else {
                break;
            }
        }
        None
    }

    /// Build the full path for a specific name (hard link) of a record.
    ///
    /// This handles the case where a file has multiple hard links, each
    /// with a different parent directory and thus a different path.
    #[must_use]
    pub fn build_path_for_name(&self, record: &FileRecord, name_idx: u16) -> String {
        let Some(name_info) = self.get_name_at(record, name_idx) else {
            return self.build_path(record.frs); // Fallback to primary
        };

        let mut components = Vec::new();

        // Add the file's own name
        let name = self.get_name(name_info.name);
        if !name.is_empty() && name != "." {
            components.push(name.to_owned());
        }

        // Walk up the parent chain from this name's parent.
        // `name_info.parent_frs` is typed `ParentFrs`; demote with
        // `.as_frs()` only at the `find()` call site.
        let no_entry_parent = ParentFrs::new(u64::from(NO_ENTRY));
        let mut current_parent = name_info.parent_frs;

        while current_parent != no_entry_parent && !current_parent.is_root() {
            if let Some(parent_record) = self.find(current_parent.as_frs()) {
                let parent_name = self.record_name(parent_record);
                if !parent_name.is_empty() && parent_name != "." {
                    components.push(parent_name.to_owned());
                }

                let next_parent = parent_record.first_name.parent_frs;
                if next_parent == no_entry_parent || next_parent == current_parent {
                    break;
                }
                current_parent = next_parent;
            } else {
                break;
            }
        }

        // Reverse and join with a standard drive-qualified backslash path.
        components.reverse();
        format!("{}:\\{}", self.volume.as_char(), components.join("\\"))
    }

    /// Build the full path including stream name for ADS.
    ///
    /// Format: `C:/path/to/file:stream_name` for ADS
    /// Format: `C:/path/to/file` for default stream
    #[must_use]
    pub fn build_path_with_stream(
        &self,
        record: &FileRecord,
        name_idx: u16,
        stream: &IndexStreamInfo,
    ) -> String {
        let base_path = self.build_path_for_name(record, name_idx);
        let stream_name = self.stream_name(stream);

        if stream_name.is_empty() {
            base_path
        } else {
            format!("{base_path}:{stream_name}")
        }
    }
}

// ============================================================================
// Path resolution (on-demand parent-chain traversal)
// ============================================================================

impl MftIndex {
    /// Build the full path for a record by traversing parent chain.
    ///
    /// This is done on-demand (not stored) to save memory.
    #[must_use]
    pub fn build_path(&self, frs: Frs) -> String {
        let mut components = Vec::new();
        let mut current_frs = frs;
        let no_entry_parent = ParentFrs::new(u64::from(NO_ENTRY));

        // Walk up the parent chain.
        while let Some(record) = self.find(current_frs) {
            let name = self.record_name(record);
            if !name.is_empty() && name != "." {
                components.push(name.to_owned());
            }

            // Move to parent.
            let parent_frs = record.first_name.parent_frs;
            if parent_frs == no_entry_parent || parent_frs.as_frs() == current_frs {
                break; // Root or self-reference
            }
            if parent_frs.is_root() {
                break; // Reached root
            }
            current_frs = parent_frs.as_frs();
        }

        // Reverse and join with a standard drive-qualified backslash path.
        components.reverse();
        format!("{}:\\{}", self.volume.as_char(), components.join("\\"))
    }
}
