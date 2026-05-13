// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Extension-record merge helpers for base parsed records and columns.

use super::{ExtensionAttributes, ParseResult, ParsedColumns, ParsedRecord};
use crate::index::frs_to_usize;
use crate::ntfs::StreamInfo;

/// Merges extension record attributes into base records.
///
/// **LEGACY MULTI-PASS PIPELINE:** This type is part of the old
/// `parse_record_full → MftRecordMerger → from_parsed_records` pipeline.
/// The production hot paths now use direct-to-index parsers
/// (`SlidingIocpInline` for live, `load_raw_to_index_direct` for files)
/// that build the index incrementally without this intermediate allocation.
/// This merger is still used by:
/// - Legacy read modes (`Parallel`, `Pipelined`, `PipelinedParallel`,
///   `SlidingIocp`)
/// - `DataFrame` export (`load_raw_to_dataframe_with_options`)
/// - Tests and diagnostic tools
///
/// This implements extension record merging where attributes from extension
/// records are merged into their base records per NTFS specification.
///
/// # Performance Optimization (2026-01-23)
///
/// Uses `Vec<Option<ParsedRecord>>` indexed directly by FRS instead of
/// `HashMap`. This eliminates all hash computations (11.7M `SipHash` calls on
/// large MFTs). FRS numbers are sequential 0..N, making direct indexing O(1)
/// with no overhead.
///
/// Expected improvement: 20-30% overall (was 13% of CPU time in `HashMap` ops).
///
/// # Cross-Platform
///
/// This struct is cross-platform and can be used on all platforms.
/// It only depends on `ParsedRecord`, `ParseResult`, `ExtensionAttributes`,
/// and `ParsedColumns` which are all cross-platform.
#[derive(Debug)]
pub struct MftRecordMerger {
    /// Base records indexed directly by FRS number.
    /// `base_records[frs]` = Some(record) if present, None otherwise.
    base_records: Vec<Option<ParsedRecord>>,
    /// Pending extension attributes.
    extensions: Vec<ExtensionAttributes>,
    /// Count of base records (for efficient `len()`)
    base_count: usize,
}

impl MftRecordMerger {
    /// Creates a new merger.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            base_records: Vec::new(),
            extensions: Vec::new(),
            base_count: 0,
        }
    }

    /// Creates a new merger with capacity for `max_frs` records.
    ///
    /// # Arguments
    ///
    /// * `max_frs` - The maximum FRS number expected (typically `total_records`
    ///   from MFT)
    #[must_use]
    pub fn with_capacity(max_frs: usize) -> Self {
        Self {
            // Pre-allocate for direct FRS indexing
            base_records: vec![None; max_frs + 1],
            extensions: Vec::with_capacity(max_frs / 100), // Extensions are rare
            base_count: 0,
        }
    }

    /// Adds a parse result to the merger.
    ///
    /// # Performance
    ///
    /// O(1) insertion - direct index assignment, no hashing.
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: resize ensures FRS < len"
    )]
    pub fn add_result(&mut self, result: ParseResult) {
        match result {
            ParseResult::Base(record) => {
                let frs = frs_to_usize(record.frs);
                // Expand if needed (rare - only if FRS exceeds initial capacity)
                if frs >= self.base_records.len() {
                    self.base_records.resize(frs + 1, None);
                }
                if self.base_records[frs].is_none() {
                    self.base_count += 1;
                }
                self.base_records[frs] = Some(record);
            }
            ParseResult::Extension(ext) => {
                self.extensions.push(ext);
            }
            ParseResult::Skip => {}
        }
    }

    /// Merges all extensions into their base records and returns the result.
    #[must_use]
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: base_frs < base_records.len()"
    )]
    pub fn merge(mut self) -> Vec<ParsedRecord> {
        // Merge all extensions into their base records
        for ext in self.extensions {
            let base_frs = frs_to_usize(ext.base_frs);
            if base_frs < self.base_records.len()
                && let Some(ref mut base) = self.base_records[base_frs]
            {
                // Merge names from extension records (no dedup — every $FILE_NAME
                // attribute is counted, including duplicates across base+extension records).
                for name in ext.names {
                    base.names.push(name);
                }
                // Merge streams from extension records
                //
                // Each primary attribute (LowestVCN == 0) is counted as a separate
                // stream. Same-name non-$DATA streams are NOT merged — each internal
                // attribute type instance is counted individually. However, $DATA streams
                // (named ADS or unnamed default) may appear as continuation extents
                // that should be merged by name.
                //
                // Internal streams ("$EA", "$OBJECT_ID", etc.) from extension records
                // are separate attribute instances, each counted individually.
                for stream in ext.streams {
                    // Check if this is an internal/system stream (name starts with
                    // "$" + uppercase letter). These are non-$DATA attribute types
                    // that are counted as separate streams.
                    let is_internal = stream
                        .name
                        .strip_prefix('$')
                        .and_then(|rest| rest.chars().next())
                        .is_some_and(|ch| ch.is_ascii_uppercase());

                    if is_internal {
                        // Internal stream: always push as new (each counted separately)
                        base.streams.push(stream);
                    } else if let Some(existing) =
                        base.streams.iter_mut().find(|s| s.name == stream.name)
                    {
                        // $DATA stream (unnamed or named ADS): merge sizes
                        // This handles continuation extents split across segments
                        existing.size += stream.size;
                        existing.allocated_size += stream.allocated_size;
                        existing.is_sparse |= stream.is_sparse;
                        existing.is_compressed |= stream.is_compressed;
                        existing.is_resident &= stream.is_resident;
                    } else {
                        // New $DATA stream (ADS not in base): add it
                        base.streams.push(stream);
                    }
                }
                // Merge directory index sizes from extension records
                // For directories, $I30 attributes may be split across extension records
                if ext.dir_index_size > 0 || ext.dir_index_allocated > 0 {
                    // Find or create the default stream (empty name) for directory
                    if let Some(default_stream) =
                        base.streams.iter_mut().find(|s| s.name.is_empty())
                    {
                        // Add extension's $I30 sizes to existing default stream
                        default_stream.size += ext.dir_index_size;
                        default_stream.allocated_size += ext.dir_index_allocated;
                    } else {
                        // Create default stream for directory index
                        base.streams.push(StreamInfo {
                            name: String::new(),
                            size: ext.dir_index_size,
                            allocated_size: ext.dir_index_allocated,
                            is_sparse: false,
                            is_compressed: false,
                            is_resident: ext.dir_index_allocated == 0,
                        });
                    }
                }
            }
        }

        // Recalculate sizes from merged streams and fix primary name if needed
        let mut result = Vec::with_capacity(self.base_count);
        for record in self.base_records.iter_mut().flatten() {
            // Sort names by source_frs (ascending) to match MFT scan order.
            // Records are processed in ascending FRS order, so when an extension
            // record has a lower FRS than the base, its names appear first.
            record.names.sort_by_key(|n| n.source_frs);

            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }

            // If base record had no $FILE_NAME but extensions added names,
            // update the primary name from the first available name.
            // This handles cases where all $FILE_NAME attributes are in extension records.
            if record.name.is_empty() && !record.names.is_empty() {
                // Find the best name (prefer Win32/Win32+DOS namespace)
                let best_name = record
                    .names
                    .iter()
                    .rfind(|name| matches!(name.namespace, 1 | 3))
                    .or_else(|| record.names.first());
                if let Some(name_info) = best_name {
                    record.name = name_info.name.clone();
                    record.parent_frs = name_info.parent_frs;
                    record.namespace = name_info.namespace;
                    record.fn_created = name_info.fn_created;
                    record.fn_modified = name_info.fn_modified;
                    record.fn_accessed = name_info.fn_accessed;
                    record.fn_mft_changed = name_info.fn_mft_changed;
                }
            }
        }

        // Collect non-None records that have a name
        // Records without a name after merging have no $FILE_NAME attributes
        // (not even in extension records) and should be skipped
        for record in self.base_records.into_iter().flatten() {
            if !record.name.is_empty() {
                result.push(record);
            }
        }

        result
    }

    /// Returns the number of base records.
    #[must_use]
    pub const fn base_count(&self) -> usize {
        self.base_count
    }

    /// Returns the number of pending extensions.
    #[must_use]
    pub const fn extension_count(&self) -> usize {
        self.extensions.len()
    }

    /// Merges all extensions and returns the result as `ParsedColumns` (`SoA`
    /// layout).
    ///
    /// This is more efficient than `merge()` followed by conversion because it
    /// avoids creating an intermediate `Vec<ParsedRecord>`.
    ///
    /// # Arguments
    ///
    /// * `expand_links` - If `true` (default), expand hard links to separate
    ///   rows (matching expected behavior). If `false`, output one row per
    ///   unique FRS (power user mode).
    #[must_use]
    pub(crate) fn merge_into_columns(self, expand_links: bool) -> ParsedColumns {
        self.merge_into_columns_internal(expand_links)
    }

    /// Internal implementation for `merge_into_columns`.
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds checked: base_frs < base_records.len()"
    )]
    fn merge_into_columns_internal(mut self, expand_links: bool) -> ParsedColumns {
        // Merge all extensions into their base records
        for ext in self.extensions {
            // FRS values are bounded by MFT size, always < 2^32 on real systems
            let base_frs = usize::try_from(ext.base_frs).unwrap_or(usize::MAX);
            if base_frs < self.base_records.len()
                && let Some(ref mut base) = self.base_records[base_frs]
            {
                // Merge names from extension records (no dedup — every $FILE_NAME
                // attribute is counted, including duplicates across base+extension records).
                for name in ext.names {
                    base.names.push(name);
                }
                // Merge streams from extension records
                //
                // Each primary attribute (LowestVCN == 0) is counted as a separate
                // stream. Same-name non-$DATA streams are NOT merged — each internal
                // attribute type instance is counted individually. However, $DATA streams
                // (named ADS or unnamed default) may appear as continuation extents
                // that should be merged by name.
                //
                // Internal streams ("$EA", "$OBJECT_ID", etc.) from extension records
                // are separate attribute instances, each counted individually.
                for stream in ext.streams {
                    // Check if this is an internal/system stream (name starts with
                    // "$" + uppercase letter). These are non-$DATA attribute types
                    // that are counted as separate streams.
                    let is_internal = stream
                        .name
                        .strip_prefix('$')
                        .and_then(|rest| rest.chars().next())
                        .is_some_and(|ch| ch.is_ascii_uppercase());

                    if is_internal {
                        // Internal stream: always push as new (each counted separately)
                        base.streams.push(stream);
                    } else if let Some(existing) =
                        base.streams.iter_mut().find(|s| s.name == stream.name)
                    {
                        // $DATA stream (unnamed or named ADS): merge sizes
                        // This handles continuation extents split across segments
                        existing.size += stream.size;
                        existing.allocated_size += stream.allocated_size;
                        existing.is_sparse |= stream.is_sparse;
                        existing.is_compressed |= stream.is_compressed;
                        existing.is_resident &= stream.is_resident;
                    } else {
                        // New $DATA stream (ADS not in base): add it
                        base.streams.push(stream);
                    }
                }
                // Merge directory index sizes from extension records
                // For directories, $I30 attributes may be split across extension records
                if ext.dir_index_size > 0 || ext.dir_index_allocated > 0 {
                    // Find or create the default stream (empty name) for directory
                    if let Some(default_stream) =
                        base.streams.iter_mut().find(|s| s.name.is_empty())
                    {
                        // Add extension's $I30 sizes to existing default stream
                        default_stream.size += ext.dir_index_size;
                        default_stream.allocated_size += ext.dir_index_allocated;
                    } else {
                        // Create default stream for directory index
                        base.streams.push(StreamInfo {
                            name: String::new(),
                            size: ext.dir_index_size,
                            allocated_size: ext.dir_index_allocated,
                            is_sparse: false,
                            is_compressed: false,
                            is_resident: ext.dir_index_allocated == 0,
                        });
                    }
                }
            }
        }

        // Recalculate sizes from merged streams and fix primary name if needed
        // For directories, size comes from the default stream (which now includes
        // merged $I30 sizes from extension records)
        for record in self.base_records.iter_mut().flatten() {
            // Sort names by source_frs (ascending) to match MFT scan order.
            // Records are processed in ascending FRS order, so when an extension
            // record has a lower FRS than the base, its names appear first.
            record.names.sort_by_key(|n| n.source_frs);

            if let Some(default_stream) = record.streams.iter().find(|s| s.name.is_empty()) {
                record.size = default_stream.size;
                record.allocated_size = default_stream.allocated_size;
            }

            // If base record had no $FILE_NAME but extensions added names,
            // update the primary name from the first available name.
            // This handles cases where all $FILE_NAME attributes are in extension records.
            if record.name.is_empty() && !record.names.is_empty() {
                // Find the best name (prefer Win32/Win32+DOS namespace)
                let best_name = record
                    .names
                    .iter()
                    .rfind(|name| matches!(name.namespace, 1 | 3))
                    .or_else(|| record.names.first());
                if let Some(name_info) = best_name {
                    record.name = name_info.name.clone();
                    record.parent_frs = name_info.parent_frs;
                    record.namespace = name_info.namespace;
                    record.fn_created = name_info.fn_created;
                    record.fn_modified = name_info.fn_modified;
                    record.fn_accessed = name_info.fn_accessed;
                    record.fn_mft_changed = name_info.fn_mft_changed;
                }
            }
        }

        // Estimate capacity: if expanding links, we need more space
        // Use integer arithmetic to avoid float precision issues
        let estimated_capacity = if expand_links {
            // Rough estimate: assume average of 1.2 links per file (base_count * 6 / 5)
            self.base_count.saturating_mul(6) / 5
        } else {
            self.base_count
        };

        // Convert directly to ParsedColumns (single pass, no intermediate Vec)
        // Skip records with empty names - they have no $FILE_NAME attributes
        let mut columns = ParsedColumns::with_capacity(estimated_capacity);
        for record in self.base_records.into_iter().flatten() {
            // Skip records without a name after merging
            if record.name.is_empty() {
                continue;
            }
            if expand_links {
                columns.push_record_expanded(&record);
            } else {
                columns.push_record(&record);
            }
        }
        columns
    }
}

impl Default for MftRecordMerger {
    fn default() -> Self {
        Self::new()
    }
}
