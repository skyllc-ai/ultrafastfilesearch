//! Per-worker index fragments used during parallel MFT parsing.

use super::{
    ChildInfo, ExtensionTable, FileRecord, IndexStreamInfo, InternalStreamInfo, LinkInfo, NO_ENTRY,
    frs_to_usize, len_to_u32,
};

// ============================================================================
// MftIndexFragment - Partial index for parallel parsing
// ============================================================================

/// A partial MFT index built by a worker thread during parallel parsing.
///
/// Each worker builds its own fragment, which is later merged into the
/// final `MftIndex`. This avoids contention on a shared index.
///
/// # Thread Safety
///
/// Fragments are built by a single thread and then moved to the merge
/// thread. No synchronization is needed during building.
#[derive(Debug, Default)]
pub struct MftIndexFragment {
    /// File records parsed by this worker
    pub records: Vec<FileRecord>,
    /// FRS → record index lookup (local to this fragment)
    pub frs_to_idx: Vec<u32>,
    /// Filenames concatenated (local buffer)
    pub names: String,
    /// Overflow hard link entries
    pub links: Vec<LinkInfo>,
    /// Overflow stream entries
    pub streams: Vec<IndexStreamInfo>,
    /// Internal stream entries filtered from user-visible output but retained
    /// for precise tree-metrics accounting.
    pub internal_streams: Vec<InternalStreamInfo>,
    /// Directory child entries
    pub children: Vec<ChildInfo>,
    /// Extension interning table (local to this fragment)
    pub extensions: ExtensionTable,
}

impl MftIndexFragment {
    /// Create a new empty fragment with estimated capacity.
    #[must_use]
    pub fn with_capacity(record_capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(record_capacity),
            frs_to_idx: Vec::with_capacity(record_capacity),
            names: String::with_capacity(record_capacity * 20), // ~20 chars avg
            links: Vec::new(),
            streams: Vec::new(),
            internal_streams: Vec::new(),
            children: Vec::with_capacity(record_capacity / 10), // ~10% are dirs
            extensions: ExtensionTable::new(),
        }
    }

    /// Extract extension from a filename and intern it.
    ///
    /// Returns the `extension_id` for the extension (0 if no extension).
    /// Extensions are normalized to lowercase without the leading dot.
    pub fn intern_extension(&mut self, filename: &str) -> u16 {
        // Find the last dot in the filename
        if let Some(dot_pos) = filename.rfind('.') {
            // Make sure it's not a hidden file (e.g., ".gitignore")
            // and not at the end (e.g., "file.")
            if dot_pos > 0 && dot_pos < filename.len() - 1 {
                if let Some(extension) = filename.get(dot_pos + 1..) {
                    return self.extensions.intern(extension);
                }
            }
        }

        // No extension found
        0
    }

    /// Get or create a record for the given FRS.
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: resize ensures frs_usize < len"
    )]
    pub fn get_or_create(&mut self, frs: u64) -> &mut FileRecord {
        let frs_usize = frs_to_usize(frs);

        // Expand lookup table if needed
        if frs_usize >= self.frs_to_idx.len() {
            self.frs_to_idx.resize(frs_usize + 1, NO_ENTRY);
        }

        let idx = self.frs_to_idx[frs_usize];
        if idx == NO_ENTRY {
            // Create new record
            let new_idx = len_to_u32(self.records.len());
            self.frs_to_idx[frs_usize] = new_idx;
            self.records.push(FileRecord::new(frs));
            let len = self.records.len();
            &mut self.records[len - 1]
        } else {
            &mut self.records[idx as usize]
        }
    }

    /// Add a filename to the names buffer, return the offset.
    pub fn add_name(&mut self, name: &str) -> u32 {
        let offset = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
        self.names.push_str(name);
        offset
    }

    /// Number of records in this fragment.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if fragment is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
