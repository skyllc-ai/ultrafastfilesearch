// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Column-oriented accumulation helpers for parsed MFT records.
//!
//! # FRS wire-boundary policy (Phase 4 sub-phase 5d.4)
//!
//! The `frs: Vec<u64>` / `parent_frs: Vec<u64>` fields are the
//! columnar staging buffers that feed
//! [`crate::reader::dataframe_build`] and ultimately become
//! `polars::Series::new("frs", _)` columns.  They are deliberately
//! raw `u64` because the polars column type is the FRS wire boundary
//! by Phase-4 doctrine — every typed [`crate::Frs`] / [`crate::ParentFrs`]
//! value in the workspace demotes to raw `u64` at the polars / CSV /
//! JSON edge.
//!
//! Callers with typed FRS values demote via `frs.raw()` /
//! `u64::from(frs)` once per record before pushing into the column
//! vectors; see [`ParsedColumns::push_record`] for the canonical
//! lift site (`self.frs.push(record.frs.raw())`).

use tracing::{debug, info, warn};

use super::{ParsedRecord, create_placeholder_record};
use crate::ntfs::{NameInfo, StreamInfo};

/// Column-oriented storage for parsed MFT records (Struct-of-Arrays layout).
///
/// This struct stores MFT record data in column vectors rather than as an
/// array of structs. This layout is optimal for:
/// - Direct conversion to Polars `DataFrame` (no transpose needed)
/// - Cache-friendly parallel accumulation
/// - Efficient memory access patterns
///
/// # Performance
///
/// Using `SoA` layout eliminates the `AoS`→`SoA` transpose that was previously
/// done in `build_dataframe_from_records`, reducing `df_build` time by ~20%.
///
/// # Cross-Platform
///
/// This struct is cross-platform and can be used on all platforms.
#[derive(Debug, Clone, Default)]
pub struct ParsedColumns {
    // Core identifiers
    /// File Record Segment numbers.
    pub frs: Vec<u64>,
    /// Parent directory FRS values.
    pub parent_frs: Vec<u64>,
    /// File/directory names.
    pub name: Vec<String>,

    // Size information
    /// Logical file sizes in bytes.
    pub size: Vec<u64>,
    /// Allocated sizes on disk.
    pub allocated_size: Vec<u64>,

    // Timestamps (Unix microseconds)
    /// Creation timestamps.
    pub created: Vec<i64>,
    /// Modification timestamps.
    pub modified: Vec<i64>,
    /// Access timestamps.
    pub accessed: Vec<i64>,
    /// MFT change timestamps.
    pub mft_changed: Vec<i64>,

    // Record metadata
    /// Whether each record is a directory.
    pub is_directory: Vec<bool>,
    /// Number of hard links (names) per record.
    pub name_count: Vec<u16>,
    /// Number of data streams per record.
    pub stream_count: Vec<u16>,
    /// Stream name (empty for default stream, non-empty for ADS).
    pub stream_name: Vec<String>,

    // Attribute flags (all boolean columns for legacy-output parity)
    /// Read-only flag.
    pub is_readonly: Vec<bool>,
    /// Hidden flag.
    pub is_hidden: Vec<bool>,
    /// System flag.
    pub is_system: Vec<bool>,
    /// Archive flag.
    pub is_archive: Vec<bool>,
    /// Compressed flag.
    pub is_compressed: Vec<bool>,
    /// Encrypted flag.
    pub is_encrypted: Vec<bool>,
    /// Sparse flag.
    pub is_sparse: Vec<bool>,
    /// Reparse point flag.
    pub is_reparse: Vec<bool>,
    /// Offline flag.
    pub is_offline: Vec<bool>,
    /// Not content indexed flag.
    pub is_not_indexed: Vec<bool>,
    /// Temporary flag.
    pub is_temporary: Vec<bool>,
    /// Integrity stream flag (`ReFS`).
    pub is_integrity_stream: Vec<bool>,
    /// No scrub data flag.
    pub is_no_scrub_data: Vec<bool>,
    /// Pinned flag (`OneDrive`).
    pub is_pinned: Vec<bool>,
    /// Unpinned flag (`OneDrive`).
    pub is_unpinned: Vec<bool>,
    /// Virtual flag.
    pub is_virtual: Vec<bool>,
    /// Raw attribute flags (combined value for legacy-output parity).
    pub flags: Vec<u32>,
}

impl ParsedColumns {
    /// Creates a new empty `ParsedColumns`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `ParsedColumns` with pre-allocated capacity.
    ///
    /// Use this when you know the approximate number of records to avoid
    /// reallocations during accumulation.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            frs: Vec::with_capacity(capacity),
            parent_frs: Vec::with_capacity(capacity),
            name: Vec::with_capacity(capacity),
            size: Vec::with_capacity(capacity),
            allocated_size: Vec::with_capacity(capacity),
            created: Vec::with_capacity(capacity),
            modified: Vec::with_capacity(capacity),
            accessed: Vec::with_capacity(capacity),
            mft_changed: Vec::with_capacity(capacity),
            is_directory: Vec::with_capacity(capacity),
            name_count: Vec::with_capacity(capacity),
            stream_count: Vec::with_capacity(capacity),
            stream_name: Vec::with_capacity(capacity),
            is_readonly: Vec::with_capacity(capacity),
            is_hidden: Vec::with_capacity(capacity),
            is_system: Vec::with_capacity(capacity),
            is_archive: Vec::with_capacity(capacity),
            is_compressed: Vec::with_capacity(capacity),
            is_encrypted: Vec::with_capacity(capacity),
            is_sparse: Vec::with_capacity(capacity),
            is_reparse: Vec::with_capacity(capacity),
            is_offline: Vec::with_capacity(capacity),
            is_not_indexed: Vec::with_capacity(capacity),
            is_temporary: Vec::with_capacity(capacity),
            is_integrity_stream: Vec::with_capacity(capacity),
            is_no_scrub_data: Vec::with_capacity(capacity),
            is_pinned: Vec::with_capacity(capacity),
            is_unpinned: Vec::with_capacity(capacity),
            is_virtual: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of records stored.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.frs.len()
    }

    /// Returns true if no records are stored.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frs.is_empty()
    }

    /// Pushes a single parsed record into the columns.
    ///
    /// This is the hot path for accumulation - keep it fast!
    #[inline]
    pub fn push_record(&mut self, record: &ParsedRecord) {
        // Columnar storage is `Vec<u64>` — it backs the Polars dataframe
        // surface whose schema is u64.  Downcast at the push boundary.
        self.frs.push(record.frs.raw());
        self.parent_frs.push(record.parent_frs.raw());
        // legacy-output parity: directories have empty name
        if record.is_directory {
            self.name.push(String::new());
        } else {
            self.name.push(record.name.clone());
        }
        self.size.push(record.size);
        self.allocated_size.push(record.allocated_size);
        self.created.push(record.std_info.created);
        self.modified.push(record.std_info.modified);
        self.accessed.push(record.std_info.accessed);
        self.mft_changed.push(record.std_info.mft_changed);
        self.is_directory.push(record.is_directory);
        self.name_count.push(record.name_count());
        self.stream_count.push(record.stream_count());
        self.stream_name.push(String::new()); // Default stream (no ADS)
        self.is_readonly.push(record.std_info.is_readonly);
        self.is_hidden.push(record.std_info.is_hidden);
        self.is_system.push(record.std_info.is_system);
        self.is_archive.push(record.std_info.is_archive);
        self.is_compressed.push(record.std_info.is_compressed);
        self.is_encrypted.push(record.std_info.is_encrypted);
        self.is_sparse.push(record.std_info.is_sparse);
        self.is_reparse.push(record.std_info.is_reparse);
        self.is_offline.push(record.std_info.is_offline);
        self.is_not_indexed
            .push(record.std_info.is_not_content_indexed);
        self.is_temporary.push(record.std_info.is_temporary);
        self.is_integrity_stream
            .push(record.std_info.is_integrity_stream);
        self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
        self.is_pinned.push(record.std_info.is_pinned);
        self.is_unpinned.push(record.std_info.is_unpinned);
        self.is_virtual.push(record.std_info.is_virtual);
        self.flags.push(record.std_info.to_raw_flags());
    }

    /// Pushes a record with full expansion (names × streams).
    ///
    /// This matches established behavior: one row per (hard link × stream)
    /// combination. If a file has 2 hard links and 3 streams, this creates
    /// 6 rows.
    ///
    /// This is the default behavior for user-facing output, as users
    /// expect to see each hard link and ADS as separate entries.
    #[inline]
    pub(crate) fn push_record_expanded(&mut self, record: &ParsedRecord) {
        // Get names to iterate over (use primary name if names is empty)
        let names: Vec<_> = if record.names.is_empty() {
            vec![NameInfo {
                name: record.name.clone(),
                parent_frs: record.parent_frs,
                namespace: 3, // Win32+DOS
                fn_created: record.fn_created,
                fn_modified: record.fn_modified,
                fn_accessed: record.fn_accessed,
                fn_mft_changed: record.fn_mft_changed,
                source_frs: record.frs,
            }]
        } else {
            record.names.clone()
        };

        // Get streams to iterate over (use empty stream if streams is empty)
        let streams: Vec<_> = if record.streams.is_empty() {
            vec![StreamInfo {
                name: String::new(),
                size: record.size,
                allocated_size: record.allocated_size,
                is_sparse: false,
                is_compressed: false,
                is_resident: false,
            }]
        } else {
            record.streams.clone()
        };

        // Create one row per (name × stream) combination
        // Filter out internal Windows streams ($OBJECT_ID, $EA_INFORMATION, etc.)
        // to produce correct directory size output
        for name_info in &names {
            for stream_info in &streams {
                // Skip internal Windows streams (matches the legacy baseline
                // match_attributes=false)
                if crate::ntfs::is_internal_windows_stream(&stream_info.name) {
                    continue;
                }
                // Polars-bound `Vec<u64>` columns — downcast at the push boundary.
                self.frs.push(record.frs.raw());
                self.parent_frs.push(name_info.parent_frs.raw());
                // legacy-output parity: directories have empty name
                if record.is_directory {
                    self.name.push(String::new());
                } else {
                    self.name.push(name_info.name.clone());
                }
                // Use stream-specific size for ADS, file size for default stream
                let (size, alloc) = if stream_info.name.is_empty() {
                    (record.size, record.allocated_size)
                } else {
                    (stream_info.size, stream_info.allocated_size)
                };
                self.size.push(size);
                self.allocated_size.push(alloc);
                self.created.push(record.std_info.created);
                self.modified.push(record.std_info.modified);
                self.accessed.push(record.std_info.accessed);
                self.mft_changed.push(record.std_info.mft_changed);
                self.is_directory.push(record.is_directory);
                // For expanded records, counts are 1 (this row = one link + one stream)
                self.name_count.push(1);
                self.stream_count.push(1);
                self.stream_name.push(stream_info.name.clone());
                self.is_readonly.push(record.std_info.is_readonly);
                self.is_hidden.push(record.std_info.is_hidden);
                self.is_system.push(record.std_info.is_system);
                self.is_archive.push(record.std_info.is_archive);
                self.is_compressed.push(record.std_info.is_compressed);
                self.is_encrypted.push(record.std_info.is_encrypted);
                self.is_sparse.push(record.std_info.is_sparse);
                self.is_reparse.push(record.std_info.is_reparse);
                self.is_offline.push(record.std_info.is_offline);
                self.is_not_indexed
                    .push(record.std_info.is_not_content_indexed);
                self.is_temporary.push(record.std_info.is_temporary);
                self.is_integrity_stream
                    .push(record.std_info.is_integrity_stream);
                self.is_no_scrub_data.push(record.std_info.is_no_scrub_data);
                self.is_pinned.push(record.std_info.is_pinned);
                self.is_unpinned.push(record.std_info.is_unpinned);
                self.is_virtual.push(record.std_info.is_virtual);
                self.flags.push(record.std_info.to_raw_flags());
            }
        }
    }

    /// Extends this `ParsedColumns` with all records from another.
    ///
    /// Used in Rayon reduce phase to merge per-thread results.
    pub fn extend(&mut self, other: Self) {
        self.frs.extend(other.frs);
        self.parent_frs.extend(other.parent_frs);
        self.name.extend(other.name);
        self.size.extend(other.size);
        self.allocated_size.extend(other.allocated_size);
        self.created.extend(other.created);
        self.modified.extend(other.modified);
        self.accessed.extend(other.accessed);
        self.mft_changed.extend(other.mft_changed);
        self.is_directory.extend(other.is_directory);
        self.name_count.extend(other.name_count);
        self.stream_count.extend(other.stream_count);
        self.stream_name.extend(other.stream_name);
        self.is_readonly.extend(other.is_readonly);
        self.is_hidden.extend(other.is_hidden);
        self.is_system.extend(other.is_system);
        self.is_archive.extend(other.is_archive);
        self.is_compressed.extend(other.is_compressed);
        self.is_encrypted.extend(other.is_encrypted);
        self.is_sparse.extend(other.is_sparse);
        self.is_reparse.extend(other.is_reparse);
        self.is_offline.extend(other.is_offline);
        self.is_not_indexed.extend(other.is_not_indexed);
        self.is_temporary.extend(other.is_temporary);
        self.is_integrity_stream.extend(other.is_integrity_stream);
        self.is_no_scrub_data.extend(other.is_no_scrub_data);
        self.is_pinned.extend(other.is_pinned);
        self.is_unpinned.extend(other.is_unpinned);
        self.is_virtual.extend(other.is_virtual);
        self.flags.extend(other.flags);
    }

    /// Reserves capacity for additional records.
    pub fn reserve(&mut self, additional: usize) {
        self.frs.reserve(additional);
        self.parent_frs.reserve(additional);
        self.name.reserve(additional);
        self.size.reserve(additional);
        self.allocated_size.reserve(additional);
        self.created.reserve(additional);
        self.modified.reserve(additional);
        self.accessed.reserve(additional);
        self.mft_changed.reserve(additional);
        self.is_directory.reserve(additional);
        self.name_count.reserve(additional);
        self.stream_count.reserve(additional);
        self.stream_name.reserve(additional);
        self.is_readonly.reserve(additional);
        self.is_hidden.reserve(additional);
        self.is_system.reserve(additional);
        self.is_archive.reserve(additional);
        self.is_compressed.reserve(additional);
        self.is_encrypted.reserve(additional);
        self.is_sparse.reserve(additional);
        self.is_reparse.reserve(additional);
        self.is_offline.reserve(additional);
        self.is_not_indexed.reserve(additional);
        self.is_temporary.reserve(additional);
        self.is_integrity_stream.reserve(additional);
        self.is_no_scrub_data.reserve(additional);
        self.is_pinned.reserve(additional);
        self.is_unpinned.reserve(additional);
        self.is_virtual.reserve(additional);
        self.flags.reserve(additional);
    }

    /// Creates `ParsedColumns` from a vector of `ParsedRecord`.
    ///
    /// # Arguments
    ///
    /// * `records` - The parsed records to convert
    /// * `expand_links` - If `true`, expand hard links to separate rows
    ///   (standard behavior). If `false`, one row per FRS.
    #[must_use]
    pub fn from_records(records: Vec<ParsedRecord>, expand_links: bool) -> Self {
        // Estimate capacity using integer arithmetic to avoid float precision issues
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file (len * 6 / 5)
            records.len().saturating_mul(6) / 5
        } else {
            records.len()
        };

        let mut columns = Self::with_capacity(estimated_capacity);
        for record in records {
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }

    /// Maximum iterations for placeholder creation to prevent infinite loops.
    const MAX_PLACEHOLDER_ITERATIONS: usize = 10;

    /// Adds placeholder records for parent directories that are referenced
    /// but not present in the parsed records.
    ///
    /// This matches established behavior where `at()` creates placeholder
    /// records for any referenced FRS that hasn't been seen yet. Without
    /// this, path resolution fails with `<unknown:XXXXXX>` for files whose
    /// parent directories weren't parsed (e.g., marked as not-in-use in
    /// bitmap).
    ///
    /// # Performance Optimization (2026-01-23)
    ///
    /// Uses `FxHashSet` instead of `std::collections::HashSet` for faster
    /// hashing. `FxHash` is 5-10x faster than `SipHash` for integer keys.
    ///
    /// # Returns
    ///
    /// The number of placeholder records added.
    pub fn add_missing_parent_placeholders(&mut self) -> usize {
        let mut total_added = 0_usize;
        let mut iterations = 0_usize;

        loop {
            iterations += 1;
            if iterations > Self::MAX_PLACEHOLDER_ITERATIONS {
                warn!(
                    iterations,
                    "Max iterations reached in placeholder creation - possible cycle"
                );
                break;
            }

            let added = self.insert_missing_parent_round();
            if added == 0 {
                break;
            }
            total_added += added;
        }

        if total_added > 0 {
            info!(
                total_added,
                iterations, "Added placeholder records for missing parent directories"
            );
        }

        total_added
    }

    /// Single pass: finds parents referenced by records but not yet present,
    /// inserts placeholder records for them, and returns how many were added.
    fn insert_missing_parent_round(&mut self) -> usize {
        use rustc_hash::FxHashSet;

        let known_frs: FxHashSet<u64> = self.frs.iter().copied().collect();
        let referenced: FxHashSet<u64> = self.parent_frs.iter().copied().collect();

        let missing: Vec<u64> = referenced
            .difference(&known_frs)
            .filter(|&&frs| frs != 0 && frs != 5)
            .copied()
            .collect();

        if missing.is_empty() {
            return 0;
        }

        debug!(
            missing_count = missing.len(),
            "Creating placeholder records for missing parent directories"
        );

        let count = missing.len();
        for frs in missing {
            let placeholder = create_placeholder_record(frs);
            self.push_record(&placeholder);
        }
        count
    }
}
