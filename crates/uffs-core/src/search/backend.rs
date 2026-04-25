// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Search backend types: display rows, sort columns, filter modes, and
//! multi-drive search orchestration.
//!
//! Exception: `file_size_policy` allows this file to exceed 800 LOC.
//! Rationale: cross-cutting facade — `DisplayRow`, `PhaseTimings`,
//! `SearchResult`, `FilterMode`, and `MultiDriveBackend` form a
//! cohesive contract surface referenced by every dispatch path, the
//! daemon wire layer, and the test harness.  Splitting would
//! scatter the type definitions across files and break the
//! single-import convention downstream crates rely on.

use alloc::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use super::dispatch::{
    apply_dispatch_safety_nets, dispatch_match_all, dispatch_regex, dispatch_trigram_or_tree,
    pick_mode_label,
};
use crate::compact::DriveCompactIndex;
use crate::search::field::FieldId;

/// Sentinel: no truncation — return every matching record.
const UNLIMITED: usize = usize::MAX;

/// A single displayable search result row.
///
/// The filename is **not** stored separately — it is derived from the `path`
/// field using `name_start` (byte offset where the filename begins within
/// `path`).  This avoids one heap allocation per result row.
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::partial_pub_fields,
    reason = "name_start is private by design — accessed via name() method"
)]
pub struct DisplayRow {
    /// Record index within the compact/cache file.
    pub record_index: u32,
    /// Drive letter this result belongs to.
    pub drive: char,
    /// Full resolved path (e.g., `C:\Users\file.txt`).
    pub path: String,
    /// Byte offset within `path` where the filename begins.
    ///
    /// `self.name()` returns `&self.path[name_start..]`.
    /// Computed once at construction from the last `\` separator.
    name_start: u32,
    /// File size in bytes.
    pub size: u64,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Last modified time (Unix microseconds).
    pub modified: i64,
    /// Creation time (Unix microseconds).
    pub created: i64,
    /// Last access time (Unix microseconds).
    pub accessed: i64,
    /// Raw NTFS `FILE_ATTRIBUTE_*` flags.
    pub flags: u32,
    /// Allocated size on disk in bytes.
    pub allocated: u64,
    /// Descendant count (directories only).
    pub descendants: u32,
    /// Sum of logical file sizes in entire subtree (directories only).
    pub treesize: u64,
    /// Sum of allocated sizes in entire subtree (directories only).
    pub tree_allocated: u64,
}

impl DisplayRow {
    /// Construct a `DisplayRow`, computing `name_start` from the path.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "flat struct — all fields are required, no logical grouping"
    )]
    pub fn new(
        record_index: u32,
        drive: char,
        path: String,
        size: u64,
        is_directory: bool,
        modified: i64,
        created: i64,
        accessed: i64,
        flags: u32,
        allocated: u64,
        descendants: u32,
        treesize: u64,
        tree_allocated: u64,
    ) -> Self {
        let name_start = uffs_mft::len_to_u32(path.rfind('\\').map_or(0, |pos| pos + 1));
        Self {
            record_index,
            drive,
            path,
            name_start,
            size,
            is_directory,
            modified,
            created,
            accessed,
            flags,
            allocated,
            descendants,
            treesize,
            tree_allocated,
        }
    }

    /// Filename portion of the path (e.g., `file.txt`).
    ///
    /// Zero-cost: returns a `&str` slice into the owned `path`.
    ///
    /// The `uffs_format::FormatRow::name` trait method forwards to
    /// this inherent method — keeping the inherent impl named `name`
    /// (rather than e.g. `file_name`) preserves the accessor's
    /// ergonomics across the many `uffs-core` call sites that
    /// predate the trait.  The intentional collision with the trait
    /// method silences `clippy::same_name_method` here.
    #[must_use]
    #[inline]
    #[expect(
        clippy::same_name_method,
        reason = "shared name with the FormatRow trait impl is intentional — see method-level doc"
    )]
    pub fn name(&self) -> &str {
        self.path.get(self.name_start as usize..).unwrap_or("")
    }

    /// Directory portion of path (up to and including the last `\`).
    ///
    /// Uses `name_start` for zero-cost slicing (no `rfind` needed).
    #[must_use]
    #[inline]
    pub fn path_dir(&self) -> &str {
        self.path
            .get(..self.name_start as usize)
            .unwrap_or(&self.path)
    }
}

/// Feed `DisplayRow` straight into the shared `uffs-format` writer.
///
/// The daemon holds `DisplayRow` directly on the search hot path, so
/// this impl lets `uffs_format::write_rows::<DisplayRow, _>` run
/// without an intermediate copy.  Every accessor is O(1) and just
/// hands back a struct field (or the pre-computed filename slice),
/// matching the trait's inlineability requirement.
///
/// The trait method `name()` collides with `DisplayRow::name()` (the
/// inherent accessor that pre-dates the trait); the trait impl
/// delegates to the inherent impl so the behaviour is identical.
/// The `clippy::same_name_method` lint is silenced on the inherent
/// method above — see its `#[expect]` attribute.
impl uffs_format::FormatRow for DisplayRow {
    #[inline]
    fn drive(&self) -> char {
        self.drive
    }
    #[inline]
    fn path(&self) -> &str {
        &self.path
    }
    #[inline]
    fn name(&self) -> &str {
        Self::name(self)
    }
    #[inline]
    fn size(&self) -> u64 {
        self.size
    }
    #[inline]
    fn is_directory(&self) -> bool {
        self.is_directory
    }
    #[inline]
    fn modified(&self) -> i64 {
        self.modified
    }
    #[inline]
    fn created(&self) -> i64 {
        self.created
    }
    #[inline]
    fn accessed(&self) -> i64 {
        self.accessed
    }
    #[inline]
    fn flags(&self) -> u32 {
        self.flags
    }
    #[inline]
    fn allocated(&self) -> u64 {
        self.allocated
    }
    #[inline]
    fn descendants(&self) -> u32 {
        self.descendants
    }
    #[inline]
    fn treesize(&self) -> u64 {
        self.treesize
    }
    #[inline]
    fn tree_allocated(&self) -> u64 {
        self.tree_allocated
    }
}

/// Sub-phase wall-clock breakdown inside the `pattern == "*"` pipeline.
///
/// Populated only when the `match_all` dispatch path is taken (via
/// `collect_global_top_n_numeric`); the regex / trigram paths leave
/// this `None` on the parent [`SearchResult`].
///
/// Units are whole milliseconds (`u64`) to match the rest of the
/// `SearchProfile` wire type; sub-millisecond phases clamp to 0.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhaseTimings {
    /// Ext-index candidate iteration + inline predicate filter.
    pub scan_ms: u64,
    /// `sort_unstable_by_key` on the candidate `(u16, u32, i64)`
    /// tuples (or heap drain, depending on `use_heap`).
    pub sort_ms: u64,
    /// `tree::resolve_path_cached` over every sorted candidate.
    /// This is the dominant cost at high row counts: reordering
    /// candidates by a numeric key (e.g. `Modified`) scrambles
    /// MFT locality and collapses the `DirCache` hit rate.
    pub path_resolve_ms: u64,
    /// Deep-profile counter: number of candidates that reached
    /// the path-resolve loop (i.e. survived the scan + sort +
    /// truncate).  Divide `path_resolve_ms` by this to get
    /// per-record cost; anything over ~500 ns/record points at
    /// allocation pressure or a pathological parent-walk depth.
    pub path_candidates: u64,
    /// Deep-profile counter: total entries across all per-drive
    /// `DirCache` instances at the end of the path-resolve loop.
    /// Because `DirCache` is keyed by `parent_frs` and only grows
    /// on misses, this value is the exact miss count.  Hits =
    /// `path_candidates - path_cache_entries`.  A very small
    /// value (< 1 % of candidates) means locality is fine and the
    /// per-candidate cost is dominated by something else (string
    /// alloc, row building, etc.).
    pub path_cache_entries: u64,
    /// Deep-profile counter: cumulative nanoseconds spent inside
    /// `tree::resolve_path_cached` across all candidates.  This
    /// isolates the path-walk + cache-lookup + string-concat
    /// cost from the surrounding row-building work.  Compare
    /// against `path_build_row_ns` to see which half dominates.
    pub path_resolve_fn_ns: u64,
    /// Deep-profile counter: cumulative nanoseconds spent inside
    /// `make_display_row` + the subsequent `Vec::push`.  This
    /// measures the `DisplayRow` struct construction (name slice,
    /// flags decode, size/time copies) separately from the path
    /// resolution.
    pub path_build_row_ns: u64,
}

/// Result of a search operation.
pub struct SearchResult {
    /// Matching rows.
    pub rows: Vec<DisplayRow>,
    /// How long the search took.
    pub duration: core::time::Duration,
    /// Total records scanned across all drives.
    pub records_scanned: usize,
    /// Sub-phase breakdown for the `pattern == "*"` fast path
    /// (scan / sort / `path_resolve`).  `None` for other dispatch
    /// paths (regex, trigram) and for match-all paths that took
    /// the `PathOnly` sort branch.
    pub phase_timings: Option<PhaseTimings>,
}

/// Legacy type alias — all sort columns are now `FieldId`.
pub type SortColumn = FieldId;

/// Filter mode for file/directory results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// Show all results.
    #[default]
    All,
    /// Show only files.
    FilesOnly,
    /// Show only directories.
    DirsOnly,
}

/// Parsed sort specification: column + direction.
#[derive(Debug, Clone, Copy)]
pub struct SortSpec {
    /// Which field to sort by.
    pub column: FieldId,
    /// `true` = descending (biggest / newest first).
    pub descending: bool,
}

/// Parameters for a search operation on [`MultiDriveBackend`].
///
/// Bundles all search-time knobs into a single struct so callers (daemon,
/// CLI, tests) use one consistent API and `search` stays under the
/// `clippy::too_many_arguments` threshold.
#[derive(Debug)]
pub struct SearchRequest<'a> {
    /// The search pattern (glob, substring, regex with `>` prefix, or `*`).
    pub pattern: &'a str,
    /// Whether matching is case-sensitive.
    pub case_sensitive: bool,
    /// Whether to match whole words only.
    pub whole_word: bool,
    /// Whether to match against the full path (not just filename).
    pub match_path: bool,
    /// Maximum number of results to return (`None` = unlimited).
    pub result_limit: Option<u32>,
    /// File / directory filter mode.
    pub filter_mode: FilterMode,
    /// Mutable search filters (extensions, dates, size, etc.).
    pub search_filters: &'a mut super::filters::SearchFilters,
    /// Drive-letter filter: only search drives whose letter is in this
    /// slice.  An empty slice means "search all loaded drives".
    pub drives_filter: &'a [char],
}

impl<'a> SearchRequest<'a> {
    /// Create a minimal request with only the required fields.
    ///
    /// All optional flags default to `false` / `None` / `FilterMode::All`.
    #[must_use]
    pub const fn new(
        pattern: &'a str,
        search_filters: &'a mut super::filters::SearchFilters,
    ) -> Self {
        Self {
            pattern,
            case_sensitive: false,
            whole_word: false,
            match_path: false,
            result_limit: None,
            filter_mode: FilterMode::All,
            search_filters,
            drives_filter: &[],
        }
    }
}

/// Shared, immutable index snapshot for concurrent query access.
///
/// Holds all loaded drives wrapped in per-drive `Arc`s.  Wrapped in an
/// outer `Arc` so concurrent queries can hold cheap references while
/// mutations (load, refresh, remove) atomically swap the pointer.
///
/// Created by the daemon's `IndexManager` — the TUI uses
/// [`MultiDriveBackend`] directly.
pub struct DriveIndex {
    /// Per-drive compact indices, each individually `Arc`-wrapped so
    /// adding/removing a single drive copies only `Arc` pointers (~8
    /// bytes each), not the underlying record data (~250 MB/drive).
    pub drives: Vec<Arc<DriveCompactIndex>>,
}

impl DriveIndex {
    /// Create an empty index with no drives loaded.
    #[must_use]
    pub const fn new() -> Self {
        Self { drives: Vec::new() }
    }

    /// Total record count across all loaded drives.
    #[must_use]
    pub fn total_records(&self) -> usize {
        self.drives.iter().map(|dr| dr.records.len()).sum()
    }

    /// List loaded drives with record counts.
    #[must_use]
    pub fn drive_summary(&self) -> Vec<(char, usize)> {
        self.drives
            .iter()
            .map(|dr| (dr.letter, dr.records.len()))
            .collect()
    }
}

impl Default for DriveIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Multi-drive search backend backed by compact indices.
pub struct MultiDriveBackend {
    /// Loaded drives (compact index, ~72 bytes/record).
    pub drives: Vec<DriveCompactIndex>,
    /// Last search results (kept for re-sorting without re-searching).
    pub last_results: Vec<DisplayRow>,
    /// Current (primary) sort column.
    pub sort_column: FieldId,
    /// Primary sort direction (`true` = descending).
    pub sort_desc: bool,
    /// Additional sort tiers beyond the primary.
    pub extra_sort_tiers: Vec<SortSpec>,
}

impl Default for MultiDriveBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiDriveBackend {
    /// Create a new empty backend.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            drives: Vec::new(),
            last_results: Vec::new(),
            sort_column: FieldId::Modified,
            sort_desc: true,
            extra_sort_tiers: Vec::new(),
        }
    }

    /// Total record count across all loaded drives.
    #[must_use]
    pub fn total_records(&self) -> usize {
        self.drives.iter().map(|dr| dr.records.len()).sum()
    }

    /// List loaded drives with record counts.
    #[must_use]
    pub fn drive_summary(&self) -> Vec<(char, usize)> {
        self.drives
            .iter()
            .map(|dr| (dr.letter, dr.records.len()))
            .collect()
    }

    /// Search all loaded drives using the given request.
    ///
    /// This is the single search entry point.  Results are sorted by the
    /// backend's current `sort_column` / `sort_desc`, then truncated to
    /// `result_limit`.
    ///
    /// When `drives_filter` is non-empty, only drives whose letter is in
    /// the slice are searched.
    #[expect(
        clippy::too_many_lines,
        reason = "search dispatch with three modes and a drive filter"
    )]
    pub fn search(&mut self, req: SearchRequest<'_>) -> SearchResult {
        // `pattern` is bound `mut` so the ext-glob safety net below can
        // rewrite `*.<ext>` → `"*"` in-place without introducing a
        // shadow binding (see the `shadow_reuse` workspace lint).
        let SearchRequest {
            mut pattern,
            case_sensitive,
            whole_word,
            match_path,
            result_limit,
            filter_mode,
            search_filters,
            drives_filter,
        } = req;

        let start = Instant::now();
        let mut rows = Vec::new();
        let mut phase_timings: Option<PhaseTimings> = None;

        if pattern.is_empty() {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                phase_timings: None,
            };
        }

        // Apply both dispatch-time pattern rewrites (drive-prefix +
        // ext-glob) via the shared helper — see `apply_dispatch_safety_nets`
        // docs for the composition rules.  `drive_from_prefix` owns the
        // single-element vec so the borrow in `effective_drives_filter`
        // stays valid through the stash-and-partition block below.
        let mut drive_from_prefix: Vec<char> = Vec::new();
        apply_dispatch_safety_nets(
            &mut pattern,
            match_path,
            case_sensitive,
            drives_filter.is_empty(),
            search_filters,
            &mut drive_from_prefix,
        );
        let effective_drives_filter: &[char] = if drive_from_prefix.is_empty() {
            drives_filter
        } else {
            &drive_from_prefix
        };

        // When a drive filter is active, temporarily swap out non-matching
        // drives so the rest of the search logic (which uses `self.drives`)
        // only touches the requested subset. We restore afterwards.
        let stashed_drives = if effective_drives_filter.is_empty() {
            None
        } else {
            let all = core::mem::take(&mut self.drives);
            let (keep, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|dr| {
                effective_drives_filter
                    .iter()
                    .any(|fl| fl.eq_ignore_ascii_case(&dr.letter))
            });
            self.drives = keep;
            Some(rest)
        };

        let is_match_all = pattern == "*";
        let is_regex = pattern.starts_with('>') && pattern.len() > 1;
        let limit = result_limit.map_or(UNLIMITED, |val| val as usize);

        // Fold needle using $UpCase from the first drive (all drives share
        // the same default table; a live volume table would override).
        let fold = self
            .drives
            .first()
            .map_or_else(uffs_text::case_fold::CaseFold::default_table, |drive| {
                drive.fold
            });
        let needle = if case_sensitive {
            pattern.to_owned()
        } else {
            let mut buf = Vec::with_capacity(pattern.len());
            fold.fold_into(pattern, &mut buf).to_owned()
        };
        let is_path = !is_match_all && !is_regex && crate::search::tree::is_path_pattern(&needle);

        if is_match_all {
            let (match_all_rows, match_all_timings) = super::query::collect_global_top_n(
                &self.drives,
                limit,
                self.sort_column,
                self.sort_desc,
                filter_mode,
                search_filters,
            );
            rows = match_all_rows;
            phase_timings = match_all_timings;
            // Post-filters that require resolved paths (type, path_contains,
            // bulkiness, path_length) are not applied inside
            // `collect_global_top_n` — they operate on `DisplayRow`, not
            // `CompactRecord`.  Apply them here, matching the regex and
            // normal-search branches.
            if search_filters.needs_display_row_filter() {
                super::filters::apply_search_filters(&mut rows, search_filters);
            }
        } else if is_regex {
            let regex_pattern = needle.strip_prefix('>').unwrap_or(&needle);
            match regex::RegexBuilder::new(regex_pattern)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(compiled_re) => {
                    let drive_results: Vec<Vec<DisplayRow>> = self
                        .drives
                        .par_iter()
                        .map(|drive| {
                            super::query::search_compact_drive_regex(drive, &compiled_re, limit)
                        })
                        .collect();
                    for drive_rows in drive_results {
                        rows.extend(drive_rows);
                    }
                    super::filters::apply_filter(&mut rows, filter_mode);
                    super::filters::apply_search_filters(&mut rows, search_filters);
                    sort_rows(
                        &mut rows,
                        self.sort_column,
                        self.sort_desc,
                        &self.extra_sort_tiers,
                    );
                    rows.truncate(limit);
                }
                Err(_err) => {
                    // Restore stashed drives before returning.
                    if let Some(rest) = stashed_drives {
                        self.drives.extend(rest);
                    }
                    self.last_results.clear();
                    return SearchResult {
                        rows: Vec::new(),
                        duration: start.elapsed(),
                        records_scanned: 0,
                        phase_timings: None,
                    };
                }
            }
        } else {
            let drive_results: Vec<Vec<DisplayRow>> = self
                .drives
                .par_iter()
                .map(|drive| {
                    if is_path {
                        super::query::search_compact_drive_tree(drive, &needle, limit)
                    } else {
                        super::query::search_compact_drive(
                            drive,
                            &needle,
                            limit,
                            case_sensitive,
                            whole_word,
                            match_path,
                        )
                    }
                })
                .collect();
            for drive_rows in drive_results {
                rows.extend(drive_rows);
            }
            super::filters::apply_filter(&mut rows, filter_mode);
            super::filters::apply_search_filters(&mut rows, search_filters);
            sort_rows(
                &mut rows,
                self.sort_column,
                self.sort_desc,
                &self.extra_sort_tiers,
            );
            rows.truncate(limit);
        }

        let scanned = self.drives.iter().map(|dr| dr.records.len()).sum();

        // Restore stashed drives if we filtered them out.
        if let Some(rest) = stashed_drives {
            self.drives.extend(rest);
        }
        let wall_ms = start.elapsed().as_millis();

        let mode = if is_match_all {
            "match-all"
        } else if is_regex {
            "regex"
        } else if is_path {
            "tree"
        } else {
            "trigram"
        };
        tracing::debug!(
            target: "cache_profile",
            wall_ms = %wall_ms,
            rows = rows.len(),
            scanned,
            mode,
            "search_total"
        );

        // Store results in last_results for TUI re-sort; return the
        // same rows by swapping ownership then cloning back.  This is
        // identical cost to the old clone_from — but callers that never
        // re-sort (CLI / daemon) can ignore last_results entirely.
        // Future optimisation: make SearchResult borrow from last_results.
        self.last_results = rows;
        SearchResult {
            rows: self.last_results.clone(),
            duration: start.elapsed(),
            records_scanned: scanned,
            phase_timings,
        }
    }

    /// Re-sort the last results by a different column.
    pub fn sort(&mut self, column: FieldId, descending: bool) {
        self.sort_column = column;
        self.sort_desc = descending;
        self.extra_sort_tiers.clear();
        sort_rows(&mut self.last_results, column, descending, &[]);
    }

    /// Cycle to the next sort column with a sensible default direction.
    pub fn cycle_sort(&mut self) {
        let next = self.sort_column.cycle_next();
        let new_desc = matches!(
            next.default_sort_direction(),
            Some(crate::search::field::SortDirection::Descending)
        );
        self.sort_column = next;
        self.sort_desc = new_desc;
        self.extra_sort_tiers.clear();
        sort_rows(&mut self.last_results, self.sort_column, self.sort_desc, &[
        ]);
    }

    /// Toggle sort direction (ascending ↔ descending) and re-sort.
    pub fn toggle_sort_direction(&mut self) {
        self.sort_desc = !self.sort_desc;
        self.extra_sort_tiers.clear();
        sort_rows(&mut self.last_results, self.sort_column, self.sort_desc, &[
        ]);
    }
}

// ── Free-function search for concurrent access ───────────────────────

/// Execute a search against a shared [`DriveIndex`] snapshot.
///
/// All per-query state (sort, filters, limit) is passed as parameters —
/// this function **never mutates the index**, so it is safe to call from
/// multiple threads/tasks simultaneously.
///
/// This is the daemon-facing entry point.  The TUI continues to use
/// [`MultiDriveBackend::search()`] which wraps its own per-query state.
#[expect(
    clippy::too_many_lines,
    reason = "search dispatch with three modes and a drive filter — mirrors MultiDriveBackend::search"
)]
pub fn search_index(
    index: &DriveIndex,
    req: SearchRequest<'_>,
    sort_column: FieldId,
    sort_desc: bool,
    extra_sort_tiers: &[SortSpec],
) -> SearchResult {
    // `pattern` is bound `mut` so the ext-glob safety net below can
    // rewrite `*.<ext>` → `"*"` in-place without introducing a shadow
    // binding (see the `shadow_reuse` workspace lint).
    let SearchRequest {
        mut pattern,
        case_sensitive,
        whole_word,
        match_path,
        result_limit,
        filter_mode,
        search_filters,
        drives_filter,
    } = req;

    let start = Instant::now();

    if pattern.is_empty() {
        return SearchResult {
            rows: Vec::new(),
            duration: start.elapsed(),
            records_scanned: 0,
            phase_timings: None,
        };
    }

    // Apply both dispatch-time pattern rewrites (drive-prefix +
    // ext-glob) via the shared helper — see `apply_dispatch_safety_nets`
    // docs for the composition rules.  `drive_from_prefix` owns the
    // single-element vec so the borrow in `effective_drives_filter`
    // stays valid for the rest of the function.
    let mut drive_from_prefix: Vec<char> = Vec::new();
    apply_dispatch_safety_nets(
        &mut pattern,
        match_path,
        case_sensitive,
        drives_filter.is_empty(),
        search_filters,
        &mut drive_from_prefix,
    );
    let effective_drives_filter: &[char] = if drive_from_prefix.is_empty() {
        drives_filter
    } else {
        &drive_from_prefix
    };

    // Filter drives without mutation — just skip non-matching ones.
    let active_drives: Vec<&DriveCompactIndex> = index
        .drives
        .iter()
        .filter(|dr| {
            effective_drives_filter.is_empty()
                || effective_drives_filter
                    .iter()
                    .any(|fl| fl.eq_ignore_ascii_case(&dr.letter))
        })
        .map(Arc::as_ref)
        .collect();

    let is_match_all = pattern == "*";
    let is_regex = pattern.starts_with('>') && pattern.len() > 1;
    let limit = result_limit.map_or(UNLIMITED, |val| val as usize);

    // Fold needle using $UpCase from the first drive.
    let fold = active_drives
        .first()
        .map_or_else(uffs_text::case_fold::CaseFold::default_table, |drive| {
            drive.fold
        });
    let needle = if case_sensitive {
        pattern.to_owned()
    } else {
        let mut buf = Vec::with_capacity(pattern.len());
        fold.fold_into(pattern, &mut buf).to_owned()
    };
    let is_path = !is_match_all && !is_regex && crate::search::tree::is_path_pattern(&needle);

    tracing::debug!(
        pattern,
        sort_column = ?sort_column,
        sort_desc,
        limit,
        is_match_all,
        hide_system = search_filters.hide_system,
        filters_empty = search_filters.is_empty(),
        "[1] search_index entry"
    );

    let (rows, phase_timings): (Vec<DisplayRow>, Option<PhaseTimings>) = if is_match_all {
        dispatch_match_all(
            &active_drives,
            limit,
            sort_column,
            sort_desc,
            filter_mode,
            search_filters,
        )
    } else if is_regex {
        let Some(regex_rows) = dispatch_regex(
            &active_drives,
            &needle,
            case_sensitive,
            limit,
            filter_mode,
            search_filters,
            sort_column,
            sort_desc,
            extra_sort_tiers,
        ) else {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                phase_timings: None,
            };
        };
        (regex_rows, None)
    } else {
        (
            dispatch_trigram_or_tree(
                &active_drives,
                &needle,
                is_path,
                case_sensitive,
                whole_word,
                match_path,
                limit,
                filter_mode,
                search_filters,
                sort_column,
                sort_desc,
                extra_sort_tiers,
            ),
            None,
        )
    };

    let scanned = active_drives.iter().map(|dr| dr.records.len()).sum();
    let wall_ms = start.elapsed().as_millis();
    let mode = pick_mode_label(is_match_all, is_regex, is_path);
    tracing::debug!(
        target: "cache_profile",
        wall_ms = %wall_ms,
        rows = rows.len(),
        scanned,
        mode,
        "search_index_total"
    );

    SearchResult {
        rows,
        duration: start.elapsed(),
        records_scanned: scanned,
        phase_timings,
    }
}

// ── Dispatch helpers ───────────────────────────────────────────────────
// The pattern-rewrite safety nets (`apply_dispatch_safety_nets` and its
// internal helpers) plus the three per-branch dispatch functions
// (`dispatch_match_all`, `dispatch_regex`, `dispatch_trigram_or_tree`)
// and the `pick_mode_label` tracing helper live in `dispatch.rs`,
// extracted for the 800-LOC file-size policy.  Imported at the top of
// this file.

// ── Sorting & DataFrame conversion ─────────────────────────────────────
// Each concern lives in its own sibling module so callers can read
// either contract without scrolling past the other.  Re-exported here
// so existing `use uffs_core::search::backend::*;` call sites see no
// change.
pub use super::dataframe_convert::{dataframe_to_display_rows, display_rows_to_dataframe};
pub use super::sorting::{format_sort_spec, parse_sort_spec, sort_rows, sort_rows_with_fold};

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
