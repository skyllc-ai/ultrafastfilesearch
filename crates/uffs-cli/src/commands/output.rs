//! Output helpers for CLI search commands.

use core::time::Duration;
use std::fs::File;
use std::io::{BufWriter, Write};

use anyhow::{Context, Result};
use tracing::info;
use uffs_core::output::{CPP_COLUMN_ORDER, OutputColumn, OutputConfig};
use uffs_core::{export_json, export_table};

#[path = "streaming.rs"]
mod streaming;
#[expect(
    unused_imports,
    reason = "cross-platform type, currently used from Windows multi-drive paths only"
)]
pub use streaming::StreamingWriter;
#[cfg(test)]
pub(super) use streaming::format_json_value;

/// Context for C++ baseline-compatible footer formatting.
pub(super) struct CppFooterContext<'a> {
    /// Drive letters to include in the footer (e.g., `['C', 'D']`).
    pub(super) output_targets: &'a [char],
    /// Original search pattern string.
    pub(super) pattern: &'a str,
    /// Total result row count for fast-scan heuristic.
    pub(super) row_count: usize,
}

/// Return whether the offline native results can be written directly without a
/// compatibility `DataFrame`.
#[must_use]
pub(super) fn can_write_native_results(format: &str, output_config: &OutputConfig) -> bool {
    matches!(format.to_ascii_lowercase().as_str(), "csv" | "custom")
        && !selected_output_columns(output_config).contains(&OutputColumn::Bulkiness)
}

/// A single attribute requirement: attribute must be set (Include) or not set
/// (Exclude).
#[derive(Debug, Clone, Copy)]
pub(super) enum AttrRequirement {
    /// Attribute must be set (e.g., `hidden`).
    Include(AttrKind),
    /// Attribute must NOT be set (e.g., `!hidden`).
    Exclude(AttrKind),
}

/// Known NTFS file attributes for filtering.
#[derive(Debug, Clone, Copy)]
pub(super) enum AttrKind {
    /// Hidden file attribute.
    Hidden,
    /// System file attribute.
    System,
    /// Archive attribute.
    Archive,
    /// Read-only attribute.
    ReadOnly,
    /// Compressed attribute.
    Compressed,
    /// Encrypted attribute.
    Encrypted,
    /// Sparse file attribute.
    Sparse,
    /// Reparse point attribute.
    Reparse,
    /// Offline attribute.
    Offline,
    /// Not content indexed attribute.
    NotIndexed,
    /// Temporary file attribute.
    Temporary,
    /// Virtual file attribute.
    Virtual,
    /// Pinned attribute.
    Pinned,
    /// Unpinned attribute.
    Unpinned,
    /// Integrity stream attribute.
    Integrity,
    /// No scrub data attribute.
    NoScrub,
    /// Directory flag.
    Directory,
}

impl AttrKind {
    /// Parse an attribute name (case-insensitive).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "hidden" | "h" => Some(Self::Hidden),
            "system" | "s" => Some(Self::System),
            "archive" | "a" => Some(Self::Archive),
            "readonly" | "r" | "read-only" => Some(Self::ReadOnly),
            "compressed" => Some(Self::Compressed),
            "encrypted" => Some(Self::Encrypted),
            "sparse" => Some(Self::Sparse),
            "reparse" => Some(Self::Reparse),
            "offline" | "o" => Some(Self::Offline),
            "notindexed" | "notcontent" => Some(Self::NotIndexed),
            "temporary" | "temp" => Some(Self::Temporary),
            "virtual" => Some(Self::Virtual),
            "pinned" => Some(Self::Pinned),
            "unpinned" => Some(Self::Unpinned),
            "integrity" => Some(Self::Integrity),
            "noscrub" => Some(Self::NoScrub),
            "directory" | "dir" => Some(Self::Directory),
            _ => None,
        }
    }

    /// Check if this attribute is set on the given record.
    #[inline]
    #[must_use]
    pub const fn is_set(self, record: &uffs_mft::index::FileRecord) -> bool {
        match self {
            Self::Hidden => record.stdinfo.is_hidden(),
            Self::System => record.stdinfo.is_system(),
            Self::Archive => record.stdinfo.is_archive(),
            Self::ReadOnly => record.stdinfo.is_readonly(),
            Self::Compressed => record.stdinfo.is_compressed(),
            Self::Encrypted => record.stdinfo.is_encrypted(),
            Self::Sparse => record.stdinfo.is_sparse(),
            Self::Reparse => record.stdinfo.is_reparse(),
            Self::Offline => record.stdinfo.is_offline(),
            Self::NotIndexed => record.stdinfo.is_not_indexed(),
            Self::Temporary => record.stdinfo.is_temporary(),
            Self::Virtual => record.stdinfo.is_virtual(),
            Self::Pinned => record.stdinfo.is_pinned(),
            Self::Unpinned => record.stdinfo.is_unpinned(),
            Self::Integrity => record.stdinfo.is_integrity_stream(),
            Self::NoScrub => record.stdinfo.is_no_scrub_data(),
            Self::Directory => record.is_directory(),
        }
    }
}

/// Record-level filters for the streaming writer.
///
/// ALL filters are combined with AND logic and applied inline during the
/// streaming scan — no separate filter pass, zero memory overhead.
///
/// # Example CLI usage
/// ```text
/// uffs *.txt --files-only --min-size 1024 --attr hidden --newer 7d --case
/// ```
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool maps to an independent CLI flag — not a state machine"
)]
pub(super) struct StreamingRecordFilter {
    /// Only output files (skip directories).
    pub files_only: bool,
    /// Only output directories (skip files).
    pub dirs_only: bool,
    /// Hide system/hidden files.
    pub hide_system: bool,
    /// Minimum file size filter.
    pub min_size: Option<u64>,
    /// Maximum file size filter.
    pub max_size: Option<u64>,
    /// Attribute requirements (all must be satisfied — AND logic).
    pub attr_filters: Vec<AttrRequirement>,
    /// Only records modified after this timestamp (microseconds since epoch).
    pub newer_modified: Option<i64>,
    /// Only records modified before this timestamp (microseconds since epoch).
    pub older_modified: Option<i64>,
    /// Only records created after this timestamp.
    pub newer_created: Option<i64>,
    /// Only records created before this timestamp.
    pub older_created: Option<i64>,
    /// Only records accessed after this timestamp.
    pub newer_accessed: Option<i64>,
    /// Only records accessed before this timestamp.
    pub older_accessed: Option<i64>,
    /// Exclude pattern — records matching this are rejected.
    pub exclude_pattern: Option<uffs_core::IndexPattern>,
    /// Maximum number of output rows (0 = unlimited).
    pub limit: usize,
    /// Sort specification (empty = no sort, output in FRS order).
    pub sort_spec: Vec<SortColumn>,
    /// Reverse sort order (descending).
    pub sort_desc: bool,
}

/// A single sort tier: column + direction.
#[derive(Debug, Clone, Copy)]
pub(super) struct SortColumn {
    /// The column to sort by.
    pub kind: SortKind,
    /// Whether this tier sorts descending.
    pub descending: bool,
}

/// Sort column kind.
#[derive(Debug, Clone, Copy)]
pub(super) enum SortKind {
    /// File size.
    Size,
    /// Allocated size on disk.
    SizeOnDisk,
    /// Last modification timestamp.
    Modified,
    /// Creation timestamp.
    Created,
    /// Last access timestamp.
    Accessed,
    /// Filename.
    Name,
    /// Full path.
    Path,
    /// File extension.
    Extension,
    /// Descendant count.
    Descendants,
    /// Hidden attribute.
    Hidden,
    /// System attribute.
    System,
    /// Archive attribute.
    Archive,
    /// Read-only attribute.
    ReadOnly,
    /// Compressed attribute.
    Compressed,
    /// Encrypted attribute.
    Encrypted,
    /// Directory flag.
    Directory,
}

impl SortKind {
    /// Smart default sort direction for this column.
    ///
    /// Dates and sizes default to descending (newest/largest first).
    /// Names and extensions default to ascending (A→Z).
    /// Booleans default to descending (true first).
    #[must_use]
    pub const fn default_descending(self) -> bool {
        matches!(
            self,
            Self::Size
                | Self::SizeOnDisk
                | Self::Modified
                | Self::Created
                | Self::Accessed
                | Self::Descendants
                | Self::Hidden
                | Self::System
                | Self::Archive
                | Self::ReadOnly
                | Self::Compressed
                | Self::Encrypted
                | Self::Directory
        )
    }

    /// Parse a sort column name (case-insensitive).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "size" => Some(Self::Size),
            "sizeondisk" | "allocated" => Some(Self::SizeOnDisk),
            "modified" | "written" | "date" => Some(Self::Modified),
            "created" => Some(Self::Created),
            "accessed" => Some(Self::Accessed),
            "name" => Some(Self::Name),
            "path" => Some(Self::Path),
            "ext" | "extension" | "type" => Some(Self::Extension),
            "descendants" => Some(Self::Descendants),
            "hidden" | "h" => Some(Self::Hidden),
            "system" | "s" => Some(Self::System),
            "archive" | "a" => Some(Self::Archive),
            "readonly" | "r" => Some(Self::ReadOnly),
            "compressed" => Some(Self::Compressed),
            "encrypted" => Some(Self::Encrypted),
            "directory" | "dir" => Some(Self::Directory),
            _ => None,
        }
    }
}

/// Parse a comma-separated `--sort` string into sort tiers.
///
/// Each tier is `column` or `column:asc` or `column:desc`.
/// Default direction: ascending.  `--sort-desc` reverses ALL tiers.
///
/// # Examples
/// - `"size"` → `[Size(asc)]`
/// - `"size:desc,name"` → `[Size(desc), Name(asc)]`
/// - `"modified:desc,size:asc,name"` → `[Modified(desc), Size(asc), Name(asc)]`
pub(super) fn parse_sort_spec(input: &str) -> Vec<SortColumn> {
    input
        .split(',')
        .filter_map(|raw_token| {
            let trimmed = raw_token.trim();
            let (name, dir) = if let Some((col_name, dir_str)) = trimmed.split_once(':') {
                (col_name.trim(), Some(dir_str.trim()))
            } else {
                (trimmed, None) // no explicit direction → use smart default
            };
            let kind = SortKind::parse(name)?;
            let descending = dir.map_or_else(
                || kind.default_descending(),
                |dir_str| {
                    matches!(
                        dir_str.to_ascii_lowercase().as_str(),
                        "desc" | "d" | "descending"
                    )
                },
            );
            Some(SortColumn { kind, descending })
        })
        .collect()
}

/// Compare two records by multi-tier sort specification.
///
/// Extracts sort keys on-demand from the index — no pre-materialized keys.
/// For name/path sorts, uses the names buffer directly (zero allocation).
pub(super) fn compare_records(
    a_idx: usize,
    b_idx: usize,
    index: &uffs_mft::MftIndex,
    sort_spec: &[SortColumn],
    global_desc: bool,
) -> core::cmp::Ordering {
    use core::cmp::Ordering;

    let Some(rec_a) = index.records.get(a_idx) else {
        return Ordering::Equal;
    };
    let Some(rec_b) = index.records.get(b_idx) else {
        return Ordering::Equal;
    };

    for col in sort_spec {
        let ord = match col.kind {
            SortKind::Size => rec_a
                .first_stream
                .size
                .length
                .cmp(&rec_b.first_stream.size.length),
            SortKind::SizeOnDisk => rec_a
                .first_stream
                .size
                .allocated
                .cmp(&rec_b.first_stream.size.allocated),
            SortKind::Modified => rec_a.stdinfo.modified.cmp(&rec_b.stdinfo.modified),
            SortKind::Created => rec_a.stdinfo.created.cmp(&rec_b.stdinfo.created),
            SortKind::Accessed => rec_a.stdinfo.accessed.cmp(&rec_b.stdinfo.accessed),
            SortKind::Name => {
                let na = index.record_name(rec_a);
                let nb = index.record_name(rec_b);
                na.to_ascii_lowercase().cmp(&nb.to_ascii_lowercase())
            }
            SortKind::Path => Ordering::Equal,
            SortKind::Extension => rec_a
                .first_name
                .name
                .extension_id()
                .cmp(&rec_b.first_name.name.extension_id()),
            SortKind::Descendants => rec_a.descendants.cmp(&rec_b.descendants),
            SortKind::Hidden => rec_a.stdinfo.is_hidden().cmp(&rec_b.stdinfo.is_hidden()),
            SortKind::System => rec_a.stdinfo.is_system().cmp(&rec_b.stdinfo.is_system()),
            SortKind::Archive => rec_a.stdinfo.is_archive().cmp(&rec_b.stdinfo.is_archive()),
            SortKind::ReadOnly => rec_a
                .stdinfo
                .is_readonly()
                .cmp(&rec_b.stdinfo.is_readonly()),
            SortKind::Compressed => rec_a
                .stdinfo
                .is_compressed()
                .cmp(&rec_b.stdinfo.is_compressed()),
            SortKind::Encrypted => rec_a
                .stdinfo
                .is_encrypted()
                .cmp(&rec_b.stdinfo.is_encrypted()),
            SortKind::Directory => rec_a.is_directory().cmp(&rec_b.is_directory()),
        };

        if ord != Ordering::Equal {
            // Per-tier direction: col.descending XOR global_desc.
            let effective_desc = col.descending ^ global_desc;
            return if effective_desc { ord.reverse() } else { ord };
        }
    }

    Ordering::Equal
}

impl StreamingRecordFilter {
    /// Check if a record passes ALL filters (AND logic).
    #[inline]
    #[must_use]
    pub fn matches(&self, record: &uffs_mft::index::FileRecord) -> bool {
        // Type filter.
        let is_dir = record.is_directory();
        if self.files_only && is_dir {
            return false;
        }
        if self.dirs_only && !is_dir {
            return false;
        }

        // Legacy hide-system (combines hidden + system).
        if self.hide_system && (record.stdinfo.is_system() || record.stdinfo.is_hidden()) {
            return false;
        }

        // Size filter.
        let size = record.first_stream.size.length;
        if let Some(min) = self.min_size {
            if size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if size > max {
                return false;
            }
        }

        // Attribute requirements (AND — all must pass).
        for req in &self.attr_filters {
            match req {
                AttrRequirement::Include(kind) => {
                    if !kind.is_set(record) {
                        return false;
                    }
                }
                AttrRequirement::Exclude(kind) => {
                    if kind.is_set(record) {
                        return false;
                    }
                }
            }
        }

        // Date range filters (all three NTFS timestamps).
        if let Some(ts) = self.newer_modified {
            if record.stdinfo.modified < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_modified {
            if record.stdinfo.modified > ts {
                return false;
            }
        }
        if let Some(ts) = self.newer_created {
            if record.stdinfo.created < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_created {
            if record.stdinfo.created > ts {
                return false;
            }
        }
        if let Some(ts) = self.newer_accessed {
            if record.stdinfo.accessed < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_accessed {
            if record.stdinfo.accessed > ts {
                return false;
            }
        }

        true
    }
}

/// Parse a comma-separated `--attr` string into attribute requirements.
///
/// # Examples
/// - `"hidden"` → `[Include(Hidden)]`
/// - `"!hidden"` → `[Exclude(Hidden)]`
/// - `"hidden,compressed"` → `[Include(Hidden), Include(Compressed)]`
/// - `"!system,!hidden"` → `[Exclude(System), Exclude(Hidden)]`
pub(super) fn parse_attr_filter(input: &str) -> Vec<AttrRequirement> {
    input
        .split(',')
        .filter_map(|raw_token| {
            let trimmed = raw_token.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.strip_prefix('!').map_or_else(
                || AttrKind::parse(trimmed).map(AttrRequirement::Include),
                |name| AttrKind::parse(name).map(AttrRequirement::Exclude),
            )
        })
        .collect()
}

/// Parse a `--newer` / `--older` duration or date string into a timestamp.
///
/// Supports:
/// - `7d` → 7 days ago
/// - `24h` → 24 hours ago
/// - `30m` → 30 minutes ago
/// - `2026-01-15` → specific date (midnight UTC)
/// - `2026-01-15T10:30:00` → specific datetime
pub(super) fn parse_age_filter(raw_input: &str) -> Option<i64> {
    /// Helper: compute microseconds-since-epoch for "now minus N seconds".
    fn now_minus_secs(secs: i64) -> Option<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        let now_us = i64::try_from(now.as_micros()).ok()?;
        Some(now_us - secs * 1_000_000)
    }

    let input = raw_input.trim();

    // Duration format: Nd, Nh, Nm
    if let Some(days) = input
        .strip_suffix('d')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(days * 86_400);
    }
    if let Some(hours) = input
        .strip_suffix('h')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(hours * 3_600);
    }
    if let Some(mins) = input
        .strip_suffix('m')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(mins * 60);
    }

    // ISO date/datetime format via chrono
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp_micros());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        return Some(dt.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros());
    }

    None
}

/// Stream output directly from `MftIndex` — zero `SearchResult` allocation.
///
/// This replaces the chain: `IndexQuery::collect()` → `Vec<SearchResult>` →
/// `write_native_results_to()` with a single pass that reads record fields
/// directly from the index and writes rows inline.
///
/// Eliminates:
/// - 8M+ `SearchResult` allocations (3 Strings each)
/// - The Rayon parallel collect overhead
/// - Redundant `index.find(result.frs)` lookups in the output loop
pub(super) fn write_index_streaming<W: Write + ?Sized>(
    index: &uffs_mft::MftIndex,
    writer: &mut W,
    format: &str,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<usize> {
    write_index_streaming_with_filter(
        index,
        None,
        None,
        false,
        false,
        &StreamingRecordFilter::default(),
        writer,
        format,
        output_config,
        footer_ctx,
    )
}

/// Core streaming writer with optional pattern filter and optional record
/// indices.
///
/// - `pattern = None`: write ALL records (full scan `*`)
/// - `pattern = Some(pat)`: write only records whose name matches `pat`
/// - `record_indices = Some(indices)`: only visit these records (extension
///   index)
/// - `record_indices = None`: scan all records sequentially
#[expect(
    clippy::too_many_lines,
    reason = "single-pass streaming writer needs inline path + row logic"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "unified streaming writer accepting all filter options"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "flat column-match dispatch + filter checks — structurally simple, just many branches"
)]
pub(super) fn write_index_streaming_with_filter<W: Write + ?Sized>(
    index: &uffs_mft::MftIndex,
    pattern: Option<&uffs_core::IndexPattern>,
    record_indices: Option<&[u32]>,
    case_sensitive: bool,
    is_path_pattern: bool,
    record_filter: &StreamingRecordFilter,
    writer: &mut W,
    format: &str,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<usize> {
    use uffs_mft::index::PathCache;

    let output_cols = selected_output_columns(output_config);
    let tz_offset_secs = output_config.timezone_offset_secs;

    let t_cache = std::time::Instant::now();
    let path_cache = PathCache::build(index, false);
    let resolver = path_cache.resolver();
    let dir_cache = path_cache.dir_cache();
    let cache_ms = t_cache.elapsed().as_millis();
    tracing::info!(cache_ms, "📊 streaming: PathCache + dir_cache built");

    write_native_header(writer, output_config, output_cols)?;

    let mut row_buffer = String::with_capacity(512);
    let mut path_buffer = String::with_capacity(256);
    let mut hardlink_buf = String::new();
    let mut itoa_buf = itoa::Buffer::new();
    let mut row_count: usize = 0;
    let t_rows = std::time::Instant::now();

    // If sorting is requested, collect matching record indices using Top-K
    // heap (when limit is set) or full collect+sort (when unlimited).
    // Default sort limit: 200 rows to avoid collecting millions of records.
    let sorted_indices: Option<Vec<u32>> = (!record_filter.sort_spec.is_empty()).then(|| {
        let effective_limit = if record_filter.limit > 0 {
            record_filter.limit
        } else {
            200 // default sort limit to avoid collecting millions of records
        };

        let sort_spec = &record_filter.sort_spec;
        let desc = record_filter.sort_desc;

        let base_iter: Box<dyn Iterator<Item = (usize, &uffs_mft::index::FileRecord)>> =
            if let Some(indices) = record_indices {
                Box::new(indices.iter().filter_map(|&idx_u32| {
                    let idx = idx_u32 as usize;
                    index.records.get(idx).map(|rec| (idx, rec))
                }))
            } else {
                Box::new(index.records.iter().enumerate())
            };

        // Collect matching record indices, then use select_nth_unstable_by
        // (introselect) for O(n) average Top-K selection instead of O(n log n) full
        // sort.
        let mut matching: Vec<u32> = Vec::new();
        for (record_idx, record) in base_iter {
            if !resolver.is_valid_idx(record_idx) {
                continue;
            }
            if !record_filter.matches(record) {
                continue;
            }
            if let Some(pat) = pattern {
                let matches = if is_path_pattern {
                    path_buffer.clear();
                    resolver.materialize_path_into(index, record_idx, dir_cache, &mut path_buffer);
                    pat.matches(&path_buffer, case_sensitive)
                } else {
                    pat.matches(index.record_name(record), case_sensitive)
                };
                if !matches {
                    continue;
                }
            }
            if let Some(excl) = &record_filter.exclude_pattern {
                if excl.matches(index.record_name(record), case_sensitive) {
                    continue;
                }
            }
            matching.push(u32::try_from(record_idx).unwrap_or(u32::MAX));
        }

        // Partial sort: if we have more matches than the limit, use
        // select_nth_unstable_by to find the top-K in O(n) average,
        // then sort only those K entries in O(k log k).
        if matching.len() > effective_limit {
            matching.select_nth_unstable_by(effective_limit, |&idx_a, &idx_b| {
                compare_records(idx_a as usize, idx_b as usize, index, sort_spec, desc)
            });
            matching.truncate(effective_limit);
        }

        // Final sort of the top-K entries.
        matching.sort_unstable_by(|&idx_a, &idx_b| {
            compare_records(idx_a as usize, idx_b as usize, index, sort_spec, desc)
        });

        matching
    });

    // Build the final iterator: sorted indices or original scan order.
    let record_iter: Box<dyn Iterator<Item = (usize, &uffs_mft::index::FileRecord)>> =
        if let Some(sorted) = &sorted_indices {
            // Sorted path: iterate pre-filtered, pre-sorted indices.
            Box::new(sorted.iter().filter_map(|&idx_u32| {
                let idx = idx_u32 as usize;
                index.records.get(idx).map(|rec| (idx, rec))
            }))
        } else if let Some(indices) = record_indices {
            Box::new(indices.iter().filter_map(|&idx_u32| {
                let idx = idx_u32 as usize;
                index.records.get(idx).map(|rec| (idx, rec))
            }))
        } else {
            Box::new(index.records.iter().enumerate())
        };

    for (record_idx, record) in record_iter {
        if !resolver.is_valid_idx(record_idx) {
            continue;
        }

        // Apply attribute filters (files_only, dirs_only, hide_system, size).
        if !record_filter.matches(record) {
            continue;
        }

        let is_directory = record.is_directory();

        // Resolve primary path into reusable buffer (zero per-record allocation).
        path_buffer.clear();
        resolver.materialize_path_into(index, record_idx, dir_cache, &mut path_buffer);

        // Apply pattern filter: match against full path or filename.
        if let Some(pat) = pattern {
            if is_path_pattern {
                if !pat.matches(&path_buffer, case_sensitive) {
                    continue;
                }
            } else {
                let name = index.record_name(record);
                if !pat.matches(name, case_sensitive) {
                    continue;
                }
            }
        }

        // Apply exclude pattern (reject matches).
        if let Some(excl) = &record_filter.exclude_pattern {
            let name = index.record_name(record);
            if excl.matches(name, case_sensitive) {
                continue;
            }
        }

        // Check limit — stop early if we've reached the max.
        if record_filter.limit > 0 && row_count >= record_filter.limit {
            break;
        }

        // Expand names × streams (same logic as RecordExpander).
        let name_count = record.name_count.max(1);
        let stream_count = record.stream_count.max(1);

        for name_idx in 0..name_count {
            for stream_idx in 0..stream_count {
                let Some(stream_info) = index.get_stream_at(record, stream_idx) else {
                    continue;
                };
                if !stream_info.is_output_stream() {
                    continue;
                }

                // Build the display name.
                let name_info = index
                    .get_name_at(record, name_idx)
                    .unwrap_or(&record.first_name);
                let stream_name = index.stream_name(stream_info);
                let has_ads = !stream_name.is_empty();
                let base_name = index.get_name(&name_info.name);

                // Path base: use path_buffer for primary name, resolve
                // alternate for hardlinks. NEVER mutate path_buffer in this
                // inner loop — it's shared across stream iterations.
                let base_path: &str = if name_idx == 0 {
                    &path_buffer
                } else {
                    // Hard link — resolve via alternate parent (rare, <1%).
                    hardlink_buf.clear();
                    let alt = resolver.materialize_path_for_name(index, record_idx, name_idx);
                    hardlink_buf.push_str(&alt);
                    &hardlink_buf
                };
                // Whether this directory path needs a trailing backslash.
                let dir_needs_sep = is_directory && !has_ads && !base_path.ends_with('\\');

                // Determine tree metrics and displayed sizes.
                let (descendants, treesize, tree_allocated) = if stream_idx == 0 {
                    record.tree_metrics()
                } else {
                    (0, 0, 0)
                };
                let displayed_size = if is_directory && !has_ads {
                    treesize
                } else {
                    stream_info.size.length
                };
                let displayed_alloc = if is_directory && !has_ads {
                    tree_allocated
                } else {
                    stream_info.size.allocated
                };

                // Display name: dirs get empty name for default stream.
                let display_name: &str = if is_directory && !has_ads {
                    ""
                } else if has_ads {
                    // Inline "base:stream" — avoid allocation by writing
                    // directly during column output below.
                    ""
                } else {
                    base_name
                };

                // Path-only (parent directory portion including trailing \).
                // For directories: PathOnly = full path with trailing \
                //   (e.g., "D:\...\images\" → "D:\...\images\")
                // For files: PathOnly = parent directory with trailing \
                //   (e.g., "D:\...\images\foo.jpg" → "D:\...\images\")
                // For ADS: PathOnly = parent directory of the base path
                let path_only: &str = if is_directory && !has_ads {
                    // Directory default stream: PathOnly = full dir path
                    // (base_path may or may not have trailing \, we add it
                    // in the column writer so just use base_path + \ here)
                    base_path
                } else {
                    base_path
                        .rfind('\\')
                        .and_then(|pos| base_path.get(..=pos))
                        .unwrap_or_default()
                };

                // Build row (clear any hardlink stash from above).
                row_buffer.clear();
                for (col_idx, col) in output_cols.iter().enumerate() {
                    if col_idx > 0 {
                        row_buffer.push_str(&output_config.separator);
                    }
                    match col {
                        OutputColumn::Path => {
                            row_buffer.push_str(&output_config.quote);
                            row_buffer.push_str(base_path);
                            if has_ads {
                                row_buffer.push(':');
                                row_buffer.push_str(stream_name);
                            } else if dir_needs_sep {
                                row_buffer.push('\\');
                            }
                            row_buffer.push_str(&output_config.quote);
                        }
                        OutputColumn::Name => {
                            if has_ads {
                                row_buffer.push_str(&output_config.quote);
                                row_buffer.push_str(base_name);
                                row_buffer.push(':');
                                row_buffer.push_str(stream_name);
                                row_buffer.push_str(&output_config.quote);
                            } else {
                                append_quoted(&mut row_buffer, &output_config.quote, display_name);
                            }
                        }
                        OutputColumn::PathOnly => {
                            row_buffer.push_str(&output_config.quote);
                            row_buffer.push_str(path_only);
                            if dir_needs_sep && is_directory && !has_ads {
                                row_buffer.push('\\');
                            }
                            row_buffer.push_str(&output_config.quote);
                        }
                        OutputColumn::Size => {
                            row_buffer.push_str(itoa_buf.format(displayed_size));
                        }
                        OutputColumn::SizeOnDisk => {
                            row_buffer.push_str(itoa_buf.format(displayed_alloc));
                        }
                        OutputColumn::Created => {
                            append_datetime(
                                &mut row_buffer,
                                record.stdinfo.created,
                                tz_offset_secs,
                            );
                        }
                        OutputColumn::Modified => {
                            append_datetime(
                                &mut row_buffer,
                                record.stdinfo.modified,
                                tz_offset_secs,
                            );
                        }
                        OutputColumn::Accessed => {
                            append_datetime(
                                &mut row_buffer,
                                record.stdinfo.accessed,
                                tz_offset_secs,
                            );
                        }
                        OutputColumn::Descendants => {
                            row_buffer.push_str(itoa_buf.format(descendants));
                        }
                        OutputColumn::TreeSize => {
                            row_buffer.push_str(itoa_buf.format(treesize));
                        }
                        OutputColumn::TreeAllocated => {
                            row_buffer.push_str(itoa_buf.format(tree_allocated));
                        }
                        OutputColumn::Type => {
                            let ext_id = record.first_name.name.extension_id();
                            let ext = index.extensions.get_extension(ext_id).unwrap_or("");
                            append_quoted(&mut row_buffer, &output_config.quote, ext);
                        }
                        OutputColumn::Attributes | OutputColumn::AttributeValue => {
                            row_buffer.push_str(itoa_buf.format(record.stdinfo.to_attributes()));
                        }
                        OutputColumn::Hidden => {
                            append_bool(&mut row_buffer, output_config, record.stdinfo.is_hidden());
                        }
                        OutputColumn::System => {
                            append_bool(&mut row_buffer, output_config, record.stdinfo.is_system());
                        }
                        OutputColumn::Archive => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_archive(),
                            );
                        }
                        OutputColumn::ReadOnly => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_readonly(),
                            );
                        }
                        OutputColumn::Compressed => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_compressed(),
                            );
                        }
                        OutputColumn::Encrypted => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_encrypted(),
                            );
                        }
                        OutputColumn::Sparse => {
                            append_bool(&mut row_buffer, output_config, record.stdinfo.is_sparse());
                        }
                        OutputColumn::Reparse => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_reparse(),
                            );
                        }
                        OutputColumn::Offline => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_offline(),
                            );
                        }
                        OutputColumn::NotIndexed => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_not_indexed(),
                            );
                        }
                        OutputColumn::Temporary => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_temporary(),
                            );
                        }
                        OutputColumn::Virtual => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_virtual(),
                            );
                        }
                        OutputColumn::Pinned => {
                            append_bool(&mut row_buffer, output_config, record.stdinfo.is_pinned());
                        }
                        OutputColumn::Unpinned => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_unpinned(),
                            );
                        }
                        OutputColumn::DirectoryFlag => {
                            append_bool(&mut row_buffer, output_config, is_directory);
                        }
                        OutputColumn::Integrity => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_integrity_stream(),
                            );
                        }
                        OutputColumn::NoScrub => {
                            append_bool(
                                &mut row_buffer,
                                output_config,
                                record.stdinfo.is_no_scrub_data(),
                            );
                        }
                        OutputColumn::Bulkiness => {
                            row_buffer.push_str(OutputColumn::Bulkiness.default_value());
                        }
                    }
                }

                row_buffer.push('\n');
                writer.write_all(row_buffer.as_bytes())?;
                row_count += 1;
            }
        }
    }

    let rows_ms = t_rows.elapsed().as_millis();
    tracing::debug!(cache_ms, rows_ms, row_count, "[TIMING] streaming output");
    tracing::info!(
        cache_ms,
        rows_ms,
        row_count,
        "📊 streaming: output phase breakdown"
    );

    if format == "custom" {
        let final_footer = CppFooterContext {
            output_targets: footer_ctx.output_targets,
            pattern: footer_ctx.pattern,
            row_count,
        };
        write_cpp_drive_footer(writer, &final_footer)?;
    }

    Ok(row_count)
}

/// Write the configured header for direct native output.
fn write_native_header<W: Write + ?Sized>(
    writer: &mut W,
    output_config: &OutputConfig,
    output_cols: &[OutputColumn],
) -> Result<()> {
    if !output_config.header {
        return Ok(());
    }

    let mut header = String::with_capacity(output_cols.len() * 24);
    for (idx, col) in output_cols.iter().enumerate() {
        if idx > 0 {
            header.push_str(&output_config.separator);
        }
        header.push_str(&output_config.quote);
        header.push_str(col.display_name());
        header.push_str(&output_config.quote);
    }
    header.push('\n');
    header.push('\n');
    writer.write_all(header.as_bytes())?;
    Ok(())
}

/// Return the effective output columns for the current configuration.
#[must_use]
pub(super) fn selected_output_columns(output_config: &OutputConfig) -> &[OutputColumn] {
    output_config.columns.as_deref().unwrap_or(CPP_COLUMN_ORDER)
}

/// Public wrapper for `write_native_header` (used by multi-drive streaming).
pub(super) fn write_native_header_pub<W: Write + ?Sized>(
    writer: &mut W,
    output_config: &OutputConfig,
    output_cols: &[OutputColumn],
) -> Result<()> {
    write_native_header(writer, output_config, output_cols)
}

/// Stream rows from an `MftIndex` WITHOUT writing header/footer.
///
/// Used by multi-drive streaming where the caller writes one header before
/// all drives and one footer after all drives.
#[expect(
    dead_code,
    reason = "cross-platform helper, currently called from Windows multi-drive paths only"
)]
pub(super) fn write_index_streaming_no_header<W: Write + ?Sized>(
    index: &uffs_mft::MftIndex,
    writer: &mut W,
    output_config: &OutputConfig,
) -> Result<usize> {
    // Use a no-header OutputConfig clone and pass format="" to skip footer.
    let mut no_header_config = output_config.clone();
    no_header_config.header = false;
    let footer_ctx = CppFooterContext {
        output_targets: &[],
        pattern: "",
        row_count: 0,
    };
    write_index_streaming(index, writer, "", &no_header_config, &footer_ctx)
}

/// Public wrapper for `write_cpp_drive_footer` (used by multi-drive streaming).
pub(super) fn write_cpp_footer_pub<W: Write + ?Sized>(
    writer: &mut W,
    ctx: &CppFooterContext<'_>,
) -> Result<()> {
    write_cpp_drive_footer(writer, ctx)
}

/// Append a quoted string field.
fn append_quoted(row_buffer: &mut String, quote: &str, value: &str) {
    row_buffer.push_str(quote);
    row_buffer.push_str(value);
    row_buffer.push_str(quote);
}

/// Append a boolean field using the configured positive/negative strings.
fn append_bool(row_buffer: &mut String, output_config: &OutputConfig, value: bool) {
    if value {
        row_buffer.push_str(&output_config.pos);
    } else {
        row_buffer.push_str(&output_config.neg);
    }
}

/// Append a datetime field using fast manual formatting.
///
/// Replaces `chrono::format("%Y-%m-%d %H:%M:%S")` which re-parses the format
/// string on every call (24.9M times for 8.3M records × 3 timestamp columns).
/// Manual formatting is ~10-20× faster for this fixed format.
#[expect(
    clippy::cast_sign_loss,
    reason = "rem_euclid always returns non-negative value"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "day_secs and doe are mathematically bounded within u32 range"
)]
fn append_datetime(row_buffer: &mut String, timestamp_micros: i64, tz_offset_secs: i32) {
    use core::fmt::Write;

    // Apply timezone offset directly to the Unix timestamp (avoids chrono
    // DateTime construction + with_timezone + format overhead entirely).
    let adjusted_secs = timestamp_micros.div_euclid(1_000_000) + i64::from(tz_offset_secs);

    // Civil time decomposition (no leap seconds — matches chrono behavior).
    // Algorithm: days since Unix epoch → year/month/day; remainder → H:M:S.
    let day_secs = adjusted_secs.rem_euclid(86_400) as u32;
    let days = adjusted_secs.div_euclid(86_400) + 719_468; // shift to 0000-03-01 epoch

    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year_offset = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_proxy = (5 * doy + 2) / 153;
    let day = doy - (153 * month_proxy + 2) / 5 + 1;
    let month = if month_proxy < 10 {
        month_proxy + 3
    } else {
        month_proxy - 9
    };
    let year = if month <= 2 {
        year_offset + 1
    } else {
        year_offset
    };

    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    // Write "YYYY-MM-DD HH:MM:SS" directly — no format string parsing.
    // String::write_fmt is infallible, so ignoring the result is safe.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "String::write_fmt never fails"
    )]
    let _ = write!(
        row_buffer,
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
    );
}

/// Convert `IndexQuery` results to a `DataFrame` for output compatibility.
///
/// **TEMPORARY**: This function exists only for compatibility with the current
/// output pipeline which expects a `DataFrame`. The proper solution is to
/// output directly from `SearchResults` without `DataFrame` conversion.
///
/// TODO: Remove this function and output directly from `SearchResults` +
/// `MftIndex`.
#[expect(
    clippy::too_many_lines,
    reason = "builds the full output schema with 30+ columns"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short names (e.g. df) conventional in DataFrame-heavy code"
)]
pub(super) fn results_to_dataframe(
    index: &uffs_mft::MftIndex,
    results: Vec<uffs_core::SearchResult>,
    _resolve_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

    let height = results.len();

    let mut frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut parent_frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut names: Vec<String> = Vec::with_capacity(height);
    let mut file_types: Vec<String> = Vec::with_capacity(height);
    let mut paths: Vec<String> = Vec::with_capacity(height);
    let mut sizes: Vec<u64> = Vec::with_capacity(height);
    let mut allocated_sizes: Vec<u64> = Vec::with_capacity(height);
    let mut created_times: Vec<i64> = Vec::with_capacity(height);
    let mut modified_times: Vec<i64> = Vec::with_capacity(height);
    let mut accessed_times: Vec<i64> = Vec::with_capacity(height);
    let mut mft_changed_times: Vec<i64> = Vec::with_capacity(height);
    let mut is_dirs: Vec<bool> = Vec::with_capacity(height);
    let mut is_readonly: Vec<bool> = Vec::with_capacity(height);
    let mut is_hidden: Vec<bool> = Vec::with_capacity(height);
    let mut is_system: Vec<bool> = Vec::with_capacity(height);
    let mut is_archive: Vec<bool> = Vec::with_capacity(height);
    let mut is_compressed: Vec<bool> = Vec::with_capacity(height);
    let mut is_encrypted: Vec<bool> = Vec::with_capacity(height);
    let mut is_sparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_reparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_offline: Vec<bool> = Vec::with_capacity(height);
    let mut is_not_indexed: Vec<bool> = Vec::with_capacity(height);
    let mut is_temporary: Vec<bool> = Vec::with_capacity(height);
    let mut is_integrity: Vec<bool> = Vec::with_capacity(height);
    let mut is_no_scrub: Vec<bool> = Vec::with_capacity(height);
    let mut is_pinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_unpinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_virtual: Vec<bool> = Vec::with_capacity(height);
    let mut flags_values: Vec<u32> = Vec::with_capacity(height);

    let mut descendants_values: Vec<u32> = Vec::with_capacity(height);
    let mut treesize_values: Vec<u64> = Vec::with_capacity(height);
    let mut tree_allocated_values: Vec<u64> = Vec::with_capacity(height);
    let mut stream_names: Vec<String> = Vec::with_capacity(height);

    for result in results {
        let record = index.find(result.frs);
        let file_type = if let Some(rec) = record {
            let ext_id = rec.first_name.name.extension_id();
            index
                .extensions
                .get_extension(ext_id)
                .unwrap_or("")
                .to_owned()
        } else {
            result
                .name
                .rfind('.')
                .and_then(|pos| {
                    if pos > 0 && pos < result.name.len() - 1 {
                        result.name.get(pos + 1..)
                    } else {
                        None
                    }
                })
                .map(str::to_lowercase)
                .unwrap_or_default()
        };

        frs_values.push(result.frs);
        parent_frs_values.push(result.parent_frs);
        paths.push(result.path.unwrap_or_default());
        sizes.push(result.size);
        stream_names.push(result.stream_name);
        names.push(result.name);

        file_types.push(file_type);

        if let Some(rec) = record {
            allocated_sizes.push(result.allocated_size);
            created_times.push(rec.stdinfo.created);
            modified_times.push(rec.stdinfo.modified);
            accessed_times.push(rec.stdinfo.accessed);
            mft_changed_times.push(rec.stdinfo.mft_changed);
            is_dirs.push(rec.is_directory());
            is_readonly.push(rec.stdinfo.is_readonly());
            is_hidden.push(rec.stdinfo.is_hidden());
            is_system.push(rec.stdinfo.is_system());
            is_archive.push(rec.stdinfo.is_archive());
            is_compressed.push(rec.stdinfo.is_compressed());
            is_encrypted.push(rec.stdinfo.is_encrypted());
            is_sparse.push(rec.stdinfo.is_sparse());
            is_reparse.push(rec.stdinfo.is_reparse());
            is_offline.push(rec.stdinfo.is_offline());
            is_not_indexed.push(rec.stdinfo.is_not_indexed());
            is_temporary.push(rec.stdinfo.is_temporary());
            is_integrity.push(rec.stdinfo.is_integrity_stream());
            is_no_scrub.push(rec.stdinfo.is_no_scrub_data());
            is_pinned.push(rec.stdinfo.is_pinned());
            is_unpinned.push(rec.stdinfo.is_unpinned());
            is_virtual.push(rec.stdinfo.is_virtual());
            flags_values.push(rec.stdinfo.to_attributes());
        } else {
            allocated_sizes.push(0);
            created_times.push(0);
            modified_times.push(0);
            accessed_times.push(0);
            mft_changed_times.push(0);
            is_dirs.push(result.is_directory);
            is_readonly.push(false);
            is_hidden.push(false);
            is_system.push(false);
            is_archive.push(false);
            is_compressed.push(false);
            is_encrypted.push(false);
            is_sparse.push(false);
            is_reparse.push(false);
            is_offline.push(false);
            is_not_indexed.push(false);
            is_temporary.push(false);
            is_integrity.push(false);
            is_no_scrub.push(false);
            is_pinned.push(false);
            is_unpinned.push(false);
            is_virtual.push(false);
            flags_values.push(0);
        }

        let (desc, tsize, talloc) = if result.stream_index > 0 {
            (0_u32, 0_u64, 0_u64)
        } else if let Some(rec) = record {
            rec.tree_metrics()
        } else {
            (result.descendants, result.treesize, result.tree_allocated)
        };
        descendants_values.push(desc);
        treesize_values.push(tsize);
        tree_allocated_values.push(talloc);
    }

    let columns = vec![
        Series::new("frs".into(), frs_values).into_column(),
        Series::new("parent_frs".into(), parent_frs_values).into_column(),
        Series::new("name".into(), names).into_column(),
        Series::new("type".into(), file_types).into_column(),
        Series::new("path".into(), paths).into_column(),
        Series::new("size".into(), sizes).into_column(),
        Series::new("allocated_size".into(), allocated_sizes).into_column(),
        Series::new("created".into(), created_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast created column: {e}"))?
            .into_column(),
        Series::new("modified".into(), modified_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast modified column: {e}"))?
            .into_column(),
        Series::new("accessed".into(), accessed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast accessed column: {e}"))?
            .into_column(),
        Series::new("mft_changed".into(), mft_changed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast mft_changed column: {e}"))?
            .into_column(),
        Series::new("is_directory".into(), is_dirs).into_column(),
        Series::new("is_readonly".into(), is_readonly).into_column(),
        Series::new("is_hidden".into(), is_hidden).into_column(),
        Series::new("is_system".into(), is_system).into_column(),
        Series::new("is_archive".into(), is_archive).into_column(),
        Series::new("is_compressed".into(), is_compressed).into_column(),
        Series::new("is_encrypted".into(), is_encrypted).into_column(),
        Series::new("is_sparse".into(), is_sparse).into_column(),
        Series::new("is_reparse".into(), is_reparse).into_column(),
        Series::new("is_offline".into(), is_offline).into_column(),
        Series::new("is_not_indexed".into(), is_not_indexed).into_column(),
        Series::new("is_temporary".into(), is_temporary).into_column(),
        Series::new("is_integrity_stream".into(), is_integrity).into_column(),
        Series::new("is_no_scrub_data".into(), is_no_scrub).into_column(),
        Series::new("is_pinned".into(), is_pinned).into_column(),
        Series::new("is_unpinned".into(), is_unpinned).into_column(),
        Series::new("is_virtual".into(), is_virtual).into_column(),
        Series::new("flags".into(), flags_values).into_column(),
        Series::new("descendants".into(), descendants_values).into_column(),
        Series::new("treesize".into(), treesize_values).into_column(),
        Series::new("tree_allocated".into(), tree_allocated_values).into_column(),
        Series::new("stream_name".into(), stream_names).into_column(),
    ];

    let mut df = uffs_mft::DataFrame::new_infer_height(columns)
        .map_err(|err| anyhow::anyhow!("Failed to create DataFrame: {err}"))?;

    df = tokio::task::block_in_place(|| uffs_core::apply_directory_treesize(&df))
        .map_err(|err| anyhow::anyhow!("Failed to apply directory treesize: {err}"))?;

    df = uffs_core::add_path_only_column(&df)
        .map_err(|err| anyhow::anyhow!("Failed to add path_only column: {err}"))?;

    Ok(df)
}

/// Write search results to console or file.
pub(super) fn write_results(
    results: &uffs_mft::DataFrame,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    _elapsed: Duration,
    pattern: &str,
) -> Result<()> {
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    let row_count = results.height();

    let footer_ctx = CppFooterContext {
        output_targets,
        pattern,
        row_count,
    };

    if is_console {
        let stdout_handle = std::io::stdout();
        let mut stdout = stdout_handle.lock();
        match format {
            "json" => export_json(results, &mut stdout)?,
            "csv" => output_config.write(results, &mut stdout)?,
            "custom" => {
                output_config.write(results, &mut stdout)?;
                write_cpp_drive_footer(&mut stdout, &footer_ctx)?;
            }
            _ => export_table(results, &mut stdout)?,
        }
        stdout.flush()?;
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = BufWriter::new(file);

        match format {
            "json" => export_json(results, &mut writer)?,
            "custom" => {
                output_config.write(results, &mut writer)?;
                write_cpp_drive_footer(&mut writer, &footer_ctx)?;
            }
            _ => output_config.write(results, &mut writer)?,
        }
        writer.flush()?;

        info!(file = out, "Results written to file");
    }

    Ok(())
}

/// Append the legacy C++ drive footer for baseline-compatible custom output.
///
/// Uses CRLF line endings (`\r\n`) to match C++ baseline behavior.
/// When `row_count` is < 20,000, appends the fast-scan message.
fn write_cpp_drive_footer<W: Write + ?Sized>(
    writer: &mut W,
    ctx: &CppFooterContext<'_>,
) -> Result<()> {
    if ctx.output_targets.is_empty() {
        return Ok(());
    }

    write!(writer, "\r\n")?;
    write!(writer, "\r\n")?;
    write!(
        writer,
        "Drives? \t{}\t{}\r\n",
        ctx.output_targets.len(),
        format_cpp_drive_letters(ctx.output_targets)
    )?;
    write!(writer, "\r\n")?;

    if ctx.row_count < 20_000 {
        write!(
            writer,
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t{pattern}\r\n",
            pattern = ctx.pattern
        )?;
        write!(writer, "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n")?;
    }

    Ok(())
}

/// Format drive letters using the legacy C++ footer style (for example `D:` or
/// `C:|D:`).
#[must_use]
fn format_cpp_drive_letters(output_targets: &[char]) -> String {
    output_targets
        .iter()
        .map(|drive| format!("{}:", drive.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
