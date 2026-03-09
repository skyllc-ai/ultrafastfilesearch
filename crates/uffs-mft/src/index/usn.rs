//! Incremental USN-driven updates for persisted indexes.

use super::{
    FileRecord, IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex, NO_ENTRY, StandardInfo,
};

// USN Journal Incremental Update Support
// ============================================================================

/// Statistics from applying USN changes to an index.
#[derive(Debug, Clone, Default)]
pub struct UsnApplyStats {
    /// Number of records marked as deleted
    pub deleted: usize,
    /// Number of records created (placeholder)
    pub created: usize,
    /// Number of records modified (name/metadata)
    pub modified: usize,
    /// Number of changes skipped (FRS not in index)
    pub skipped: usize,
}

impl MftIndex {
    /// Applies USN Journal changes to update the index incrementally.
    ///
    /// This is much faster than a full MFT scan for typical workloads where
    /// only a small percentage of files change between runs.
    ///
    /// # Limitations
    ///
    /// - **Deletes**: Marks records as deleted (sets flags)
    /// - **Creates**: Creates placeholder records (limited info from USN)
    /// - **Renames**: Updates filename if FRS exists
    /// - **Metadata**: Marks as modified (actual values need MFT read)
    ///
    /// For full accuracy on creates/renames, a selective MFT read would be
    /// needed. This implementation provides a fast approximation that's
    /// sufficient for most search use cases.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "FRS fits in usize on 64-bit"
    )]
    pub fn apply_usn_changes(&mut self, changes: &[crate::usn::FileChange]) -> UsnApplyStats {
        // DELETED flag uses bit 31 of the u32 flags field
        const DELETED_FLAG: u32 = 0x8000_0000;

        let mut stats = UsnApplyStats::default();

        for change in changes {
            let frs = change.frs;
            let frs_usize = frs as usize;

            // Check if FRS is in our lookup table
            let idx = self.frs_to_idx.get(frs_usize).copied().unwrap_or(NO_ENTRY);

            if change.deleted {
                // Handle deletion
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else if let Some(record) = self.records.get_mut(idx as usize) {
                    // Mark as deleted using bit 31 of flags
                    record.stdinfo.flags |= DELETED_FLAG;
                    stats.deleted += 1;
                }
            } else if change.created {
                // Handle creation - create placeholder record
                // Note: We only have limited info from USN (FRS, parent, name)
                // For full info, we'd need to read the actual MFT record
                if idx == NO_ENTRY {
                    // Expand lookup table if needed
                    if frs_usize >= self.frs_to_idx.len() {
                        self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
                    }

                    // Create new record
                    let new_idx = self.records.len() as u32;
                    if let Some(slot) = self.frs_to_idx.get_mut(frs_usize) {
                        *slot = new_idx;
                    }

                    // Add filename to names buffer
                    let name_start = self.names.len() as u32;
                    self.names.push_str(&change.filename);
                    let name_len = change.filename.len() as u16;

                    // Create placeholder record
                    let record = FileRecord {
                        frs,
                        sequence_number: 0, // USN doesn't provide sequence number
                        namespace: 1,       // Assume Win32 namespace
                        forensic_flags: 0,  // Not deleted/corrupt/extension
                        base_frs: 0,        // Not an extension record
                        lsn: 0,             // USN doesn't provide LSN
                        reparse_tag: 0,     // USN doesn't provide reparse tag
                        stdinfo: StandardInfo::default(),
                        name_count: 1,
                        stream_count: 1,       // User-visible streams
                        total_stream_count: 1, // All streams (for tree metrics)
                        first_internal_stream: NO_ENTRY,
                        first_child: NO_ENTRY,
                        first_name: LinkInfo {
                            next_entry: NO_ENTRY,
                            name: IndexNameRef::new(
                                name_start,
                                name_len,
                                change.filename.is_ascii(),
                                IndexNameRef::NO_EXTENSION, // TODO: Extract extension (Phase 1)
                            ),
                            parent_frs: change.parent_frs,
                        },
                        first_stream: IndexStreamInfo::default(),
                        fn_created: 0,
                        fn_modified: 0,
                        fn_accessed: 0,
                        fn_mft_changed: 0,
                        descendants: 0,
                        treesize: 0,
                        tree_allocated: 0,
                        internal_streams_size: 0,
                        internal_streams_allocated: 0,
                    };
                    self.records.push(record);
                    stats.created += 1;
                } else {
                    // FRS already exists - might be a re-create after delete
                    // Clear the deleted flag if it was set
                    if let Some(record) = self.records.get_mut(idx as usize) {
                        record.stdinfo.flags &= !DELETED_FLAG;
                    }
                    stats.skipped += 1;
                }
            } else if change.renamed {
                // Handle rename - update filename
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else if let Some(record) = self.records.get_mut(idx as usize) {
                    // Update the primary name
                    // Note: This appends to names buffer (old name becomes orphaned)
                    // A compaction pass could reclaim this space if needed
                    let name_start = self.names.len() as u32;
                    self.names.push_str(&change.filename);
                    let name_len = change.filename.len() as u16;

                    record.first_name.name = IndexNameRef::new(
                        name_start,
                        name_len,
                        change.filename.is_ascii(),
                        IndexNameRef::NO_EXTENSION,
                    ); // TODO: Extract extension (Phase 1)
                    record.first_name.parent_frs = change.parent_frs;
                    stats.modified += 1;
                }
            } else if change.size_changed || change.metadata_changed {
                // Handle size/metadata change
                // We can't update the actual values without reading MFT
                // Just mark as modified for now
                if idx == NO_ENTRY {
                    stats.skipped += 1;
                } else {
                    stats.modified += 1;
                }
            } else {
                stats.skipped += 1;
            }
        }

        stats
    }
}
