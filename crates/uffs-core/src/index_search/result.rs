//! Search result modeling for direct `MftIndex` search.

use uffs_mft::index::{FileRecord, MftIndex};

/// Result of a search on `MftIndex`.
///
/// Each result represents a unique (record, name, stream) combination.
/// Files with hard links produce multiple results (different paths, same FRS).
/// Files with ADS produce multiple results (same path, different stream names).
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The file/directory name (includes `:stream_name` for ADS, legacy-output
    /// parity).
    pub name: String,
    /// The full path (if resolved), including `:stream_name` for ADS.
    pub path: Option<String>,
    /// File size in bytes (for this specific stream).
    pub size: u64,
    /// Allocated size on disk (0 for resident files, cluster-aligned for
    /// non-resident).
    pub allocated_size: u64,
    /// File Reference Segment number.
    pub frs: u64,
    /// Parent FRS (for this specific hard link).
    pub parent_frs: u64,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Stream name (empty for default `$DATA` stream).
    pub stream_name: String,
    /// Which hard link (0 = primary name).
    pub name_index: u16,
    /// Which stream (0 = default `$DATA`).
    pub stream_index: u16,

    // Tree metrics (pre-computed in MftIndex)
    /// Count of all descendants (files + subdirectories) in subtree (0 for
    /// files).
    pub descendants: u32,
    /// Sum of logical file sizes in subtree (includes this file/directory).
    pub treesize: u64,
    /// Sum of allocated disk sizes in subtree (includes this file/directory).
    pub tree_allocated: u64,
}

impl SearchResult {
    /// Create a new search result from a file record (primary name, default
    /// stream).
    #[must_use]
    pub fn from_record(record: &FileRecord, index: &MftIndex) -> Self {
        let is_directory = record.is_directory();
        // legacy-output parity: directories have empty name, files have actual name
        let name = if is_directory {
            String::new()
        } else {
            index.record_name(record).to_owned()
        };

        Self {
            name,
            path: None, // Path resolution is expensive, done on demand
            size: record.first_stream.size.length,
            allocated_size: record.first_stream.size.allocated,
            frs: record.frs,
            parent_frs: record.first_name.parent_frs,
            is_directory,
            stream_name: String::new(),
            name_index: 0,
            stream_index: 0,
            descendants: record.descendants,
            treesize: record.treesize,
            tree_allocated: record.tree_allocated,
        }
    }

    /// Create a search result for a specific (name, stream) combination.
    #[must_use]
    pub fn from_expanded(
        record: &FileRecord,
        index: &MftIndex,
        name_idx: u16,
        stream_idx: u16,
    ) -> Self {
        let name_info = index
            .get_name_at(record, name_idx)
            .unwrap_or(&record.first_name);
        let stream_info = index
            .get_stream_at(record, stream_idx)
            .unwrap_or(&record.first_stream);
        let is_directory = record.is_directory();

        // Get base filename and stream name
        let stream_name = index.stream_name(stream_info);
        let has_ads = !stream_name.is_empty();

        // legacy-output parity: directories have empty Name for default stream,
        // but ADS entries get "dirname:streamname" format (same as files)
        let name = if is_directory && !has_ads {
            // Default directory stream: empty Name
            String::new()
        } else if has_ads {
            // ADS entry (file or directory): "filename:streamname"
            let base_name = index.get_name(&name_info.name).to_owned();
            format!("{base_name}:{stream_name}")
        } else {
            // Default file stream: just the filename
            index.get_name(&name_info.name).to_owned()
        };

        // legacy-output parity: Only the default stream (stream_idx == 0) gets tree
        // metrics. ADS streams (stream_idx > 0) have
        // descendants/treesize/tree_allocated = 0. In C++, each stream has its
        // own treesize field, and only the default stream accumulates
        // children's treesize (line 4794 in UltraFastFileSearch.cpp).
        let (descendants, treesize, tree_allocated) = if stream_idx == 0 {
            (record.descendants, record.treesize, record.tree_allocated)
        } else {
            (0, 0, 0)
        };

        Self {
            name,
            path: None,
            size: stream_info.size.length,
            allocated_size: stream_info.size.allocated,
            frs: record.frs,
            parent_frs: name_info.parent_frs,
            is_directory,
            stream_name: stream_name.to_owned(),
            name_index: name_idx,
            stream_index: stream_idx,
            descendants,
            treesize,
            tree_allocated,
        }
    }

    /// Create with resolved path.
    #[must_use]
    pub fn with_path(mut self, path: String) -> Self {
        self.path = Some(path);
        self
    }

    /// Check if this is an Alternate Data Stream (ADS).
    #[must_use]
    pub fn is_ads(&self) -> bool {
        !self.stream_name.is_empty()
    }

    /// Check if this is a hard link (not the primary name).
    #[must_use]
    pub const fn is_hard_link(&self) -> bool {
        self.name_index > 0
    }
}
