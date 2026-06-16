// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Storage header and format version metadata.

use crate::index::MftIndex;
use crate::platform::DriveLetter;

/// Magic bytes for index file format.
const INDEX_MAGIC: &[u8; 8] = b"UFFSIDX\0";

/// Current index file format version.
///
/// - v2: `IndexNameRef` bit-packed `meta` field
/// - v3: tree metrics (`descendants`, `treesize`, `tree_allocated`)
/// - v4: `sequence_number`, `namespace`, `$FILE_NAME` timestamps
/// - v5: NTFS 3.0+ forensic fields (`lsn`, `usn`, `security_id`, `owner_id`)
/// - v6: P2 forensic fields (`reparse_tag`, `is_resident` in stream flags)
/// - v7: P3 forensic fields (`forensic_flags`, `base_frs` for extensions)
/// - v8: `total_stream_count` for full tree-metrics accounting
/// - v9: `StandardInfo.flags` stores raw NTFS `FILE_ATTRIBUTE_*` bits
/// - v10: `ExtensionIndex` CSR appended — zero rebuild on load
/// - v11: `ChildInfo` is Pod (24 bytes with explicit padding)
/// - v12: `build_epoch` (Unix µs) in header for cache staleness detection
/// - v13: timestamps stored as raw FILETIME (100-ns ticks since 1601-01-01)
///   instead of Unix microseconds — matches C++ baseline semantics
const INDEX_VERSION: u32 = 14;

/// Persistent index header stored at the beginning of the index file.
#[derive(Debug, Clone)]
pub struct IndexHeader {
    /// Magic bytes for format identification
    pub magic: [u8; 8],
    /// Format version for compatibility
    pub version: u32,
    /// Volume letter (e.g., [`DriveLetter::C`]).
    pub volume: DriveLetter,
    /// Volume serial number for validation
    pub volume_serial: u64,
    /// USN Journal ID at time of index creation
    pub usn_journal_id: u64,
    /// Next USN to read from (checkpoint)
    pub next_usn: crate::usn::Usn,
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
    /// Build epoch (Unix microseconds) — bumped on every build or mutation.
    /// v12+; 0 for older versions.
    pub build_epoch: u64,
}

impl IndexHeader {
    /// Creates a new header for the given index.
    #[must_use]
    pub fn new(
        index: &MftIndex,
        volume_serial: u64,
        usn_journal_id: u64,
        next_usn: crate::usn::Usn,
    ) -> Self {
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
            build_epoch: index.build_epoch,
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
        // v14 is a names-format break (WI-4.4): names are now stored as raw
        // WTF-8 bytes rather than guaranteed-UTF-8, so a pre-v14 cache must be
        // rebuilt from the MFT rather than reinterpreted. Reject anything
        // below v14; the caller's rebuild path writes a fresh v14 cache.
        if self.version < 14 || self.version > INDEX_VERSION {
            return Err("Unsupported index version (pre-v14 caches rebuild for WI-4.4 names)");
        }
        Ok(())
    }
}
