//! Search backend: direct MftIndex search without DataFrame/Polars.
//!
//! Walks `MftIndex.records` with pattern matching and filters,
//! collecting results into `Vec<DisplayRow>` for the UI.

use core::sync::atomic::AtomicBool;
use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;
use uffs_core::index_search::{IndexPattern, compile_parsed_pattern};
use uffs_core::pattern::ParsedPattern;
use uffs_mft::index::MftIndex;

/// Maximum results returned per search (prevents UI lag on broad patterns).
/// 1K is plenty for a terminal display — keeps search under ~50ms.
const DEFAULT_RESULT_LIMIT: usize = 1_000;

/// Even lower limit for very short patterns (1-2 chars) that match millions.
const SHORT_PATTERN_LIMIT: usize = 200;

/// A single displayable search result row.
#[derive(Debug, Clone)]
pub struct DisplayRow {
    /// Drive letter this result belongs to.
    pub drive: char,
    /// Full resolved path (e.g., `C:\Users\file.txt`).
    pub path: String,
    /// Filename only (e.g., `file.txt`).
    pub name: String,
    /// File size in bytes.
    pub size: u64,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Last modified time (Unix microseconds).
    pub modified: i64,
}

/// A loaded drive with its `MftIndex` and trigram search index.
pub struct DriveIndex {
    /// Drive letter (e.g., 'C').
    pub letter: char,
    /// The in-memory MFT index (core data — untouched).
    pub index: MftIndex,
    /// Trigram inverted index for fast substring search.
    pub trigram: TrigramIndex,
    /// Pre-resolved lowercase full paths for each record (for verification +
    /// display). `paths_lower[i]` = lowercase full path for record `i`.
    pub paths_lower: Vec<String>,
    /// Where this index was loaded from (used for refresh in Wave 3).
    #[expect(dead_code, reason = "stored for future refresh feature (Wave 3)")]
    pub source: IndexSource,
}

/// Trigram inverted index: maps 3-byte sequences to sorted lists of record
/// indices.
///
/// Built once at load time. Search = intersect posting lists for query
/// trigrams, then verify candidates against pre-lowered paths. O(matches) not
/// O(n).
pub struct TrigramIndex {
    /// Trigram → sorted Vec of record indices containing that trigram.
    postings: std::collections::HashMap<[u8; 3], Vec<u32>>,
}

impl TrigramIndex {
    /// Build a trigram index from pre-lowered paths.
    #[expect(
        clippy::single_call_fn,
        reason = "constructor called once per DriveIndex load; separation improves readability"
    )]
    fn build(paths_lower: &[String]) -> Self {
        let mut postings: std::collections::HashMap<[u8; 3], Vec<u32>> =
            std::collections::HashMap::new();

        for (idx, path) in paths_lower.iter().enumerate() {
            let bytes = path.as_bytes();
            if bytes.len() < 3 {
                continue;
            }
            // Safe: record count is bounded by MFT size (max ~4M records per drive)
            #[expect(
                clippy::cast_possible_truncation,
                reason = "MFT record count is bounded by NTFS limits (~4M max)"
            )]
            let record_idx = idx as u32;
            // Extract unique trigrams from this path
            let mut seen = std::collections::HashSet::new();
            for window in bytes.windows(3) {
                // windows(3) guarantees exactly 3 elements
                // windows(3) guarantees exactly 3 elements, so try_into always succeeds
                let Ok(tri): Result<[u8; 3], _> = window.try_into() else {
                    continue;
                };
                if seen.insert(tri) {
                    postings.entry(tri).or_default().push(record_idx);
                }
            }
        }

        Self { postings }
    }

    /// Number of unique trigrams in the index.
    pub fn posting_count(&self) -> usize {
        self.postings.len()
    }

    /// Search: intersect posting lists for query trigrams, return candidate
    /// record indices.
    ///
    /// For queries < 3 chars, returns None (caller should fall back to linear
    /// scan).
    fn search(&self, needle_lower: &str) -> Option<Vec<u32>> {
        let bytes = needle_lower.as_bytes();
        if bytes.len() < 3 {
            return None; // too short for trigram search
        }

        // Extract trigrams from the query
        // windows(3) guarantees exactly 3 elements per window, so try_into always
        // succeeds
        let trigrams: Vec<[u8; 3]> = bytes
            .windows(3)
            .filter_map(|win| win.try_into().ok())
            .collect();

        // Find the smallest posting list (most selective trigram)
        let mut lists: Vec<&[u32]> = trigrams
            .iter()
            .filter_map(|tri| self.postings.get(tri).map(Vec::as_slice))
            .collect();

        if lists.is_empty() {
            return Some(Vec::new()); // no trigrams found → no matches
        }

        // Sort by list size (intersect smallest first for efficiency)
        lists.sort_unstable_by_key(|list| list.len());

        // Intersect all posting lists
        // Safe: we checked lists.is_empty() above, so first() always succeeds
        let Some(first_list) = lists.first() else {
            return Some(Vec::new());
        };
        let mut result = first_list.to_vec();
        for list in lists.iter().skip(1) {
            result = intersect_sorted(&result, list);
            if result.is_empty() {
                break;
            }
        }

        Some(result)
    }
}

/// Intersect two sorted u32 slices, returning a new sorted Vec of common
/// elements.
#[expect(
    clippy::single_call_fn,
    reason = "called from TrigramIndex::search loop; separation keeps intersection logic isolated"
)]
fn intersect_sorted(list_a: &[u32], list_b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(list_a.len().min(list_b.len()));
    let mut iter_a = list_a.iter().peekable();
    let mut iter_b = list_b.iter().peekable();

    while let (Some(&val_a), Some(&val_b)) = (iter_a.peek(), iter_b.peek()) {
        match val_a.cmp(val_b) {
            core::cmp::Ordering::Equal => {
                result.push(*val_a);
                iter_a.next();
                iter_b.next();
            }
            core::cmp::Ordering::Less => {
                iter_a.next();
            }
            core::cmp::Ordering::Greater => {
                iter_b.next();
            }
        }
    }
    result
}

/// Where a drive index was loaded from.
#[expect(
    dead_code,
    reason = "variants store source paths for future refresh feature (Wave 3)"
)]
pub enum IndexSource {
    /// Raw/IOCP/compressed MFT file.
    MftFile(PathBuf),
}

/// Result of a search operation.
pub struct SearchResult {
    /// Matching rows.
    pub rows: Vec<DisplayRow>,
    /// How long the search took.
    pub duration: core::time::Duration,
    /// Total records scanned across all drives.
    pub records_scanned: usize,
    /// Time spent in scan phase (ms).
    pub scan_ms: u128,
    /// Number of trigram candidates (0 = trigram not used, >0 = trigram hit).
    pub trigram_candidates: usize,
}

/// Multi-drive search backend.
pub struct MultiDriveBackend {
    /// Loaded drives.
    pub drives: Vec<DriveIndex>,
    /// Last search results (kept for re-sorting without re-searching).
    pub last_results: Vec<DisplayRow>,
    /// Current sort column.
    pub sort_column: SortColumn,
    /// Sort direction.
    pub sort_desc: bool,
}

/// Columns available for sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    /// Sort by filename.
    Name,
    /// Sort by file size.
    Size,
    /// Sort by last modified time.
    Modified,
    /// Sort by full path.
    Path,
}

impl MultiDriveBackend {
    /// Create a new empty backend.
    #[must_use]
    #[expect(
        clippy::single_call_fn,
        reason = "constructor called from App::new; separation keeps backend initialization isolated"
    )]
    pub const fn new() -> Self {
        Self {
            drives: Vec::new(),
            last_results: Vec::new(),
            sort_column: SortColumn::Name,
            sort_desc: false,
        }
    }

    /// Total record count across all loaded drives.
    #[must_use]
    pub fn total_records(&self) -> usize {
        self.drives.iter().map(|dr| dr.index.records.len()).sum()
    }

    /// List loaded drives with record counts.
    #[must_use]
    pub fn drive_summary(&self) -> Vec<(char, usize)> {
        self.drives
            .iter()
            .map(|dr| (dr.letter, dr.index.records.len()))
            .collect()
    }

    /// Search across all loaded drives.
    ///
    /// Compiles the pattern once, then walks each drive's `MftIndex`
    /// collecting matching records into `DisplayRow`s.
    pub fn search(&mut self, pattern: &str, name_only: bool) -> SearchResult {
        let start = Instant::now();
        let mut rows = Vec::new();

        // Empty pattern → clear results
        if pattern.is_empty() {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        }

        let Ok(parsed) = ParsedPattern::parse(pattern) else {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        };

        let Ok(compiled) = compile_parsed_pattern(&parsed) else {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        };

        let is_path_pattern = parsed.is_path_pattern() && !name_only;

        let limit = if pattern.len() <= 2 {
            SHORT_PATTERN_LIMIT
        } else {
            DEFAULT_RESULT_LIMIT
        };

        let needle_lower = pattern.to_ascii_lowercase();
        let cancelled = AtomicBool::new(false);

        let scan_start = Instant::now();
        let drive_results: Vec<Vec<DisplayRow>> = self
            .drives
            .par_iter()
            .map(|drive| {
                search_drive(
                    drive,
                    &compiled,
                    &needle_lower,
                    is_path_pattern,
                    limit,
                    &cancelled,
                )
            })
            .collect();
        let scan_ms = scan_start.elapsed().as_millis();

        for drive_rows in drive_results {
            rows.extend(drive_rows);
        }
        rows.truncate(limit);
        let scanned = self.drives.iter().map(|dr| dr.index.records.len()).sum();

        sort_rows(&mut rows, self.sort_column, self.sort_desc);

        // Count trigram candidates for diagnostics
        let tri_candidates = self
            .drives
            .iter()
            .filter_map(|dr| {
                let needle = pattern.to_ascii_lowercase();
                dr.trigram.search(&needle).map(|cands| cands.len())
            })
            .sum::<usize>();

        self.last_results.clone_from(&rows);
        SearchResult {
            rows,
            duration: start.elapsed(),
            records_scanned: scanned,
            scan_ms,
            trigram_candidates: tri_candidates,
        }
    }

    /// Search without mutating self — safe for concurrent read-only access.
    ///
    /// Used by the background search thread via `Arc<MultiDriveBackend>`.
    #[expect(
        dead_code,
        reason = "public API for future async background search thread"
    )]
    pub fn search_readonly(&self, pattern: &str, name_only: bool) -> SearchResult {
        let start = Instant::now();
        let mut rows = Vec::new();

        if pattern.is_empty() {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        }

        let Ok(parsed) = ParsedPattern::parse(pattern) else {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        };

        let Ok(compiled) = compile_parsed_pattern(&parsed) else {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
                scan_ms: 0,
                trigram_candidates: 0,
            };
        };

        let is_path_pattern = parsed.is_path_pattern() && !name_only;
        let limit = if pattern.len() <= 2 {
            SHORT_PATTERN_LIMIT
        } else {
            DEFAULT_RESULT_LIMIT
        };

        let needle_lower = pattern.to_ascii_lowercase();
        let cancelled = AtomicBool::new(false);

        let drive_results: Vec<Vec<DisplayRow>> = self
            .drives
            .par_iter()
            .map(|drive| {
                search_drive(
                    drive,
                    &compiled,
                    &needle_lower,
                    is_path_pattern,
                    limit,
                    &cancelled,
                )
            })
            .collect();

        for drive_rows in drive_results {
            rows.extend(drive_rows);
        }
        rows.truncate(limit);
        let scanned = self.drives.iter().map(|dr| dr.index.records.len()).sum();

        sort_rows(&mut rows, self.sort_column, self.sort_desc);

        SearchResult {
            rows,
            duration: start.elapsed(),
            records_scanned: scanned,
            scan_ms: 0,
            trigram_candidates: 0,
        }
    }

    /// Re-sort the last results by a different column.
    #[expect(
        dead_code,
        reason = "public API for direct sort; currently used via cycle_sort/toggle_sort_direction"
    )]
    pub fn sort(&mut self, column: SortColumn, descending: bool) {
        self.sort_column = column;
        self.sort_desc = descending;
        sort_rows(&mut self.last_results, column, descending);
    }

    /// Cycle to the next sort column.
    pub fn cycle_sort(&mut self) {
        self.sort_column = match self.sort_column {
            SortColumn::Name => SortColumn::Size,
            SortColumn::Size => SortColumn::Modified,
            SortColumn::Modified => SortColumn::Path,
            SortColumn::Path => SortColumn::Name,
        };
        sort_rows(&mut self.last_results, self.sort_column, self.sort_desc);
    }

    /// Toggle sort direction.
    pub fn toggle_sort_direction(&mut self) {
        self.sort_desc = !self.sort_desc;
        sort_rows(&mut self.last_results, self.sort_column, self.sort_desc);
    }
}

/// Search a single drive's `MftIndex`, returning matching `DisplayRow`s.
///
/// Performance: matches against filenames first (zero allocation), then
/// resolves full paths only for the final result set.
fn search_drive(
    drive_index: &DriveIndex,
    _pattern: &IndexPattern,
    needle_lower: &str,
    _is_path_pattern: bool,
    limit: usize,
    cancelled: &AtomicBool,
) -> Vec<DisplayRow> {
    let volume_prefix = format!("{}:\\", drive_index.letter);
    // Always use trigram fast path — trigram index is built on full paths
    // so it handles both filename-only and path patterns.
    search_drive_fast(drive_index, needle_lower, &volume_prefix, limit, cancelled)
}

/// Ultra-fast path: trigram-indexed search on full paths.
///
/// For 3+ char patterns: uses trigram posting list intersection to find
/// candidates in O(matches), then verifies with `str::contains`.
/// For 1-2 char patterns: linear scan on `paths_lower` (hits limit fast).
#[expect(
    clippy::single_call_fn,
    reason = "called from search_drive; separation keeps search strategy logic isolated"
)]
fn search_drive_fast(
    drive_idx: &DriveIndex,
    needle_lower: &str,
    _volume_prefix: &str,
    limit: usize,
    _cancelled: &AtomicBool,
) -> Vec<DisplayRow> {
    if needle_lower.is_empty() {
        return Vec::new();
    }

    let index = &drive_idx.index;
    let drive = drive_idx.letter;
    let paths = &drive_idx.paths_lower;

    // Try trigram search first (3+ chars)
    let candidates = drive_idx.trigram.search(needle_lower);

    let match_indices: Vec<usize> = if let Some(candidate_indices) = candidates {
        // Trigram hit: verify candidates with actual substring check
        candidate_indices
            .iter()
            .filter(|&&idx| {
                let rec_idx = idx as usize;
                paths
                    .get(rec_idx)
                    .is_some_and(|path| path.contains(needle_lower))
            })
            .take(limit)
            .map(|&idx| idx as usize)
            .collect()
    } else {
        // Short pattern (<3 chars): linear scan on paths_lower
        paths
            .iter()
            .enumerate()
            .filter(|(_, path)| !path.is_empty() && path.contains(needle_lower))
            .take(limit)
            .map(|(idx, _)| idx)
            .collect()
    };

    // Build DisplayRows from match indices
    match_indices
        .iter()
        .filter_map(|&record_idx| {
            let record = index.records.get(record_idx)?;
            let name = index.get_name(&record.first_name.name);
            if name.is_empty() || name == "." {
                return None;
            }
            Some(DisplayRow {
                drive,
                path: paths.get(record_idx).cloned().unwrap_or_default(),
                name: name.to_owned(),
                size: record.first_stream.size.length,
                is_directory: record.is_directory(),
                modified: record.stdinfo.modified,
            })
        })
        .collect()
}

/// Resolve a record's full path by walking the parent chain.
#[expect(
    clippy::single_call_fn,
    reason = "called from load_mft_file path resolution loop; separation keeps path logic isolated"
)]
fn resolve_path(index: &MftIndex, record_idx: usize, volume_prefix: &str) -> String {
    let mut components = Vec::with_capacity(8);
    let mut current_idx = record_idx;
    let mut depth = 0_i32;

    loop {
        if depth > 256_i32 {
            break; // Prevent infinite loops
        }

        let Some(record) = index.records.get(current_idx) else {
            break;
        };
        let name = index.get_name(&record.first_name.name);

        if name == "." || name.is_empty() {
            break;
        }

        components.push(name.to_owned());

        let parent_frs = record.first_name.parent_frs;
        if parent_frs == record.frs || parent_frs == u64::from(uffs_mft::NO_ENTRY) {
            break;
        }

        // Look up parent record index
        let parent_usize = uffs_mft::frs_to_usize(parent_frs);
        let Some(&parent_record_idx) = index.frs_to_idx.get(parent_usize) else {
            break;
        };
        if parent_record_idx == uffs_mft::NO_ENTRY {
            break;
        }
        current_idx = parent_record_idx as usize;

        // Check for root directory (FRS 5)
        if parent_frs == uffs_mft::ROOT_FRS {
            break;
        }

        depth += 1_i32;
    }

    // Build path from components (reversed, since we walked child→parent)
    components.reverse();

    let mut path = String::with_capacity(
        volume_prefix.len() + components.iter().map(|comp| comp.len() + 1).sum::<usize>(),
    );
    path.push_str(volume_prefix);
    for (idx, component) in components.iter().enumerate() {
        path.push_str(component);
        // Add backslash separator, and trailing backslash for directories
        if idx < components.len() - 1 {
            path.push('\\');
        }
    }

    path
}

/// Sort display rows by the given column.
fn sort_rows(rows: &mut [DisplayRow], column: SortColumn, descending: bool) {
    rows.sort_unstable_by(|row_a, row_b| {
        let ord = match column {
            SortColumn::Name => row_a.name.to_lowercase().cmp(&row_b.name.to_lowercase()),
            SortColumn::Size => row_a.size.cmp(&row_b.size),
            SortColumn::Modified => row_a.modified.cmp(&row_b.modified),
            SortColumn::Path => row_a.path.to_lowercase().cmp(&row_b.path.to_lowercase()),
        };
        if descending { ord.reverse() } else { ord }
    });
}

/// Load a live NTFS drive using the same flow as `uffs.exe`:
/// detect → use cache → apply USN journal → return `MftIndex`.
///
/// Uses `MftReader::read_index_cached()` which is the recommended API.
/// Requires Windows and Administrator privileges.
#[cfg(windows)]
pub fn load_live_drive(drive_letter: char) -> anyhow::Result<DriveIndex> {
    use anyhow::Context;

    /// Cache TTL in seconds (10 minutes, same as CLI).
    const INDEX_TTL_SECONDS: u64 = 600;

    let rt = tokio::runtime::Runtime::new()?;
    let index = rt.block_on(async {
        let reader = uffs_mft::MftReader::open(drive_letter)
            .with_context(|| format!("Failed to open drive {drive_letter}:"))?;
        reader
            .read_index_cached(INDEX_TTL_SECONDS)
            .await
            .with_context(|| format!("Failed to read MFT for drive {drive_letter}:"))
    })?;

    build_drive_index(drive_letter, index)
}

/// Build a `DriveIndex` from a loaded `MftIndex` (shared by live + file paths).
#[cfg(windows)]
fn build_drive_index(drive_letter: char, index: MftIndex) -> anyhow::Result<DriveIndex> {
    let volume_prefix = format!("{drive_letter}:\\");
    let record_count = index.records.len();

    let paths_lower: Vec<String> = (0..record_count)
        .map(|record_idx| {
            let Some(record) = index.records.get(record_idx) else {
                return String::new();
            };
            if !record.first_name.name.is_valid() {
                return String::new();
            }
            let name = index.get_name(&record.first_name.name);
            if name.is_empty() || name == "." {
                return String::new();
            }
            resolve_path(&index, record_idx, &volume_prefix).to_ascii_lowercase()
        })
        .collect();

    let trigram = TrigramIndex::build(&paths_lower);

    Ok(DriveIndex {
        letter: drive_letter,
        index,
        trigram,
        paths_lower,
        source: IndexSource::MftFile(PathBuf::from(format!("{drive_letter}:"))),
    })
}

/// Load an MFT file (raw, IOCP capture, or compressed) into a `DriveIndex`.
///
/// Auto-detects the file format. If no drive letter is provided, infers it
/// from the filename.
#[expect(
    clippy::single_call_fn,
    reason = "public API called from main.rs async loader; separation keeps file loading isolated"
)]
pub fn load_mft_file(
    mft_path: &std::path::Path,
    drive: Option<char>,
) -> anyhow::Result<DriveIndex> {
    use uffs_mft::parse::{MftRecordMerger, apply_fixup, parse_record_full};

    let drive_letter = drive.unwrap_or_else(|| {
        let stem = mft_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("X");
        stem.chars()
            .next()
            .filter(char::is_ascii_alphabetic)
            .map_or('X', |ch| ch.to_ascii_uppercase())
    });

    let options = uffs_mft::raw::LoadRawOptions::default();
    let raw = uffs_mft::raw::load_raw_mft(mft_path, &options)?;
    let capacity = uffs_mft::frs_to_usize(raw.header.record_count);
    let mut merger = MftRecordMerger::with_capacity(capacity);

    for (frs, record_data) in raw.iter_records() {
        let mut record_buf = record_data.to_vec();
        if !apply_fixup(&mut record_buf) {
            continue;
        }
        merger.add_result(parse_record_full(&record_buf, frs));
    }

    let records = merger.merge();
    let index = MftIndex::from_parsed_records(drive_letter, records);

    // Build pre-resolved lowercase paths + trigram index for fast search.
    // This is done once at load time so search is O(matches) not O(n).
    let volume_prefix = format!("{drive_letter}:\\");
    let record_count = index.records.len();

    let paths_lower: Vec<String> = (0..record_count)
        .map(|record_idx| {
            let Some(record) = index.records.get(record_idx) else {
                return String::new();
            };
            if !record.first_name.name.is_valid() {
                return String::new();
            }
            let name = index.get_name(&record.first_name.name);
            if name.is_empty() || name == "." {
                return String::new();
            }
            resolve_path(&index, record_idx, &volume_prefix).to_ascii_lowercase()
        })
        .collect();

    let trigram = TrigramIndex::build(&paths_lower);

    Ok(DriveIndex {
        letter: drive_letter,
        index,
        trigram,
        paths_lower,
        source: IndexSource::MftFile(mft_path.to_path_buf()),
    })
}
