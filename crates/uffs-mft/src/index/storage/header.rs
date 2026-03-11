//! Storage header and format version metadata.

use crate::index::MftIndex;

/// Magic bytes for index file format.
const INDEX_MAGIC: &[u8; 8] = b"UFFSIDX\0";

/// Current index file format version.
/// Version 2: Changed `IndexNameRef` to use bit-packed `meta` field instead of
/// separate length/flags
/// Version 3: Added tree metrics (descendants, treesize, `tree_allocated`) to
/// `FileRecord` serialization
/// Version 4: Added `sequence_number`, `namespace`, and `$FILE_NAME` timestamps
/// (`fn_created`, `fn_modified`, `fn_accessed`, `fn_mft_changed`)
/// Version 5: Added NTFS 3.0+ forensic fields: `lsn`, `usn`, `security_id`,
/// `owner_id` Version 6: Added P2 forensic fields: `reparse_tag`, `is_resident`
/// (in stream flags) Version 7: Added P3 forensic fields: `forensic_flags`
/// (renamed from reserved), `base_frs` for extension records
/// Version 8: Added `total_stream_count` for full tree-metrics accounting
const INDEX_VERSION: u32 = 8;

/// Persistent index header stored at the beginning of the index file.
#[derive(Debug, Clone)]
pub struct IndexHeader {
    /// Magic bytes for format identification
    pub magic: [u8; 8],
    /// Format version for compatibility
    pub version: u32,
    /// Volume letter (e.g., 'C')
    pub volume: char,
    /// Volume serial number for validation
    pub volume_serial: u64,
    /// USN Journal ID at time of index creation
    pub usn_journal_id: u64,
    /// Next USN to read from (checkpoint)
    pub next_usn: i64,
    /// Timestamp when index was created (Unix epoch seconds)
    pub created_at: u64,
    /// Number of records in the index
    pub record_count: u64,
    /// Size of names buffer in bytes
    pub names_size: u64,
    /// Number of link entries
    pub links_count: u64,
    /// Number of stream entries
    pub streams_count: u64,
    /// Number of children entries
    pub children_count: u64,
}

impl IndexHeader {
    /// Creates a new header for the given index.
    #[must_use]
    pub fn new(index: &MftIndex, volume_serial: u64, usn_journal_id: u64, next_usn: i64) -> Self {
        Self {
            magic: *INDEX_MAGIC,
            version: INDEX_VERSION,
            volume: index.volume,
            volume_serial,
            usn_journal_id,
            next_usn,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |dur| dur.as_secs()),
            record_count: index.records.len() as u64,
            names_size: index.names.len() as u64,
            links_count: index.links.len() as u64,
            streams_count: index.streams.len() as u64,
            children_count: index.children.len() as u64,
        }
    }

    /// Validates the header magic and version.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic bytes are invalid or the version is
    /// unsupported.
    pub fn validate(&self) -> Result<(), &'static str> {
        if &self.magic != INDEX_MAGIC {
            return Err("Invalid index file magic");
        }
        // Accept version 3 through the latest supported format revision.
        if self.version < 3 || self.version > INDEX_VERSION {
            return Err("Unsupported index version");
        }
        Ok(())
    }
}
