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
    /// Whether this is a directory (used for --files-only/--dirs-only filter).
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
        use rayon::prelude::*;

        const CHUNK_SIZE: usize = 64 * 1024;

        // Phase 1: parallel — each chunk builds a local postings map
        let chunk_maps: Vec<std::collections::HashMap<[u8; 3], Vec<u32>>> = paths_lower
            .par_chunks(CHUNK_SIZE)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base = chunk_idx * CHUNK_SIZE;
                let mut local: std::collections::HashMap<[u8; 3], Vec<u32>> =
                    std::collections::HashMap::new();

                for (offset, path) in chunk.iter().enumerate() {
                    let bytes = path.as_bytes();
                    if bytes.len() < 3 {
                        continue;
                    }
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "MFT record count bounded by NTFS limits"
                    )]
                    let record_idx = (base + offset) as u32;

                    // Track last pushed idx per trigram to skip consecutive dupes
                    // (cheaper than HashSet — paths have many repeated trigrams)
                    for window in bytes.windows(3) {
                        let tri: [u8; 3] = match <[u8; 3]>::try_from(window) {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        let list = local.entry(tri).or_default();
                        if list.last() != Some(&record_idx) {
                            list.push(record_idx);
                        }
                    }
                }
                local
            })
            .collect();

        // Phase 2: merge all chunk maps into one (sequential but fast)
        let mut postings: std::collections::HashMap<[u8; 3], Vec<u32>> =
            std::collections::HashMap::new();

        for chunk_map in chunk_maps {
            let mut sorted_entries: Vec<_> = chunk_map.into_iter().collect();
            sorted_entries.sort_unstable_by_key(|(tri, _)| *tri);
            for (tri, indices) in sorted_entries {
                postings.entry(tri).or_default().extend(indices);
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
    /// Sort by drive letter.
    Drive,
    /// Sort by file extension.
    Extension,
    /// Sort by devicon file type (groups similar types: music, images, code).
    Type,
}

/// Filter mode for file/directory results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Show all results.
    All,
    /// Show only files.
    FilesOnly,
    /// Show only directories.
    DirsOnly,
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
            };
        }

        let Ok(parsed) = ParsedPattern::parse(pattern) else {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
            };
        };

        let Ok(compiled) = compile_parsed_pattern(&parsed) else {
            self.last_results.clear();
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
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

        self.last_results.clone_from(&rows);
        SearchResult {
            rows,
            duration: start.elapsed(),
            records_scanned: scanned,
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
            };
        }

        let Ok(parsed) = ParsedPattern::parse(pattern) else {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
            };
        };

        let Ok(compiled) = compile_parsed_pattern(&parsed) else {
            return SearchResult {
                rows: Vec::new(),
                duration: start.elapsed(),
                records_scanned: 0,
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
            SortColumn::Path => SortColumn::Drive,
            SortColumn::Drive => SortColumn::Extension,
            SortColumn::Extension => SortColumn::Type,
            SortColumn::Type => SortColumn::Name,
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

/// Maximally distinct color palettes for 1–10 drives.
///
/// Each sub-palette is hand-tuned so that N drives get N maximally
/// distinguishable colors on a dark terminal background.
const PALETTES: &[&[(u8, u8, u8)]] = &[
    // 1 drive
    &[(255, 255, 255)],
    // 2 drives
    &[(100, 180, 255), (255, 150, 50)],
    // 3 drives
    &[(100, 180, 255), (80, 220, 80), (255, 150, 50)],
    // 4 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
    ],
    // 5 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
    ],
    // 6 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
        (255, 100, 100),
    ],
    // 7 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
        (255, 100, 100),
        (100, 255, 220),
    ],
    // 8 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
        (255, 100, 100),
        (100, 255, 220),
        (255, 130, 200),
    ],
    // 9 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
        (255, 100, 100),
        (100, 255, 220),
        (255, 130, 200),
        (255, 255, 255),
    ],
    // 10 drives
    &[
        (100, 180, 255),
        (80, 220, 80),
        (255, 150, 50),
        (200, 100, 255),
        (255, 255, 80),
        (255, 100, 100),
        (100, 255, 220),
        (255, 130, 200),
        (255, 255, 255),
        (180, 140, 100),
    ],
];

/// Build a drive-letter → color mapping for the currently loaded drives.
///
/// Assigns colors from the optimal palette for the given number of drives.
/// Drives are sorted alphabetically so the mapping is deterministic.
#[must_use]
#[expect(
    clippy::single_call_fn,
    reason = "public API: intentionally a standalone function for reuse and clarity"
)]
pub fn build_drive_colors(
    drives: &[DriveIndex],
) -> std::collections::HashMap<char, ratatui::style::Color> {
    use ratatui::style::Color;

    let mut letters: Vec<char> = drives.iter().map(|dr| dr.letter).collect();
    letters.sort_unstable();
    letters.dedup();

    let count = letters.len();
    let palette_idx = count
        .saturating_sub(1)
        .min(PALETTES.len().saturating_sub(1));
    let default_palette: &[(u8, u8, u8)] = &[(255, 255, 255)];
    let palette = PALETTES.get(palette_idx).unwrap_or(&default_palette);

    letters
        .into_iter()
        .enumerate()
        .map(|(idx, letter)| {
            let &(red, green, blue) = palette
                .get(idx % palette.len().max(1))
                .unwrap_or(&(255, 255, 255));
            (letter, Color::Rgb(red, green, blue))
        })
        .collect()
}

/// Sort display rows by the given column with name as secondary tiebreaker.
fn sort_rows(rows: &mut [DisplayRow], column: SortColumn, descending: bool) {
    rows.sort_unstable_by(|row_a, row_b| {
        let primary = match column {
            SortColumn::Name => row_a.name.to_lowercase().cmp(&row_b.name.to_lowercase()),
            SortColumn::Size => row_a.size.cmp(&row_b.size),
            SortColumn::Modified => row_a.modified.cmp(&row_b.modified),
            SortColumn::Path => row_a.path.to_lowercase().cmp(&row_b.path.to_lowercase()),
            SortColumn::Drive => row_a.drive.cmp(&row_b.drive),
            SortColumn::Extension => {
                let ext_a = row_a.name.rsplit('.').next().unwrap_or("").to_lowercase();
                let ext_b = row_b.name.rsplit('.').next().unwrap_or("").to_lowercase();
                ext_a.cmp(&ext_b)
            }
            SortColumn::Type => {
                let icon_a = devicons::icon_for_file(&row_a.name, &None).icon;
                let icon_b = devicons::icon_for_file(&row_b.name, &None).icon;
                icon_a.cmp(&icon_b)
            }
        };
        // Multi-tier: if primary column is equal, break ties by name (ascending)
        let ord = if primary == core::cmp::Ordering::Equal && column != SortColumn::Name {
            row_a.name.to_lowercase().cmp(&row_b.name.to_lowercase())
        } else {
            primary
        };
        if descending { ord.reverse() } else { ord }
    });
}

/// Apply filter mode to a set of display rows.
pub fn apply_filter(rows: &mut Vec<DisplayRow>, filter: FilterMode) {
    match filter {
        FilterMode::All => {} // no-op
        FilterMode::FilesOnly => rows.retain(|row| !row.is_directory),
        FilterMode::DirsOnly => rows.retain(|row| row.is_directory),
    }
}

/// Load a live NTFS drive using the same flow as `uffs.exe`:
/// detect → use cache → apply USN journal → return `MftIndex`.
///
/// Uses `MftReader::read_index_cached()` which is the recommended API.
/// Requires Windows and Administrator privileges.
/// Timing info returned alongside a loaded `DriveIndex`.
pub struct LoadTiming {
    /// Time to load/read the MFT (milliseconds).
    pub mft: u128,
    /// Time to resolve all paths (milliseconds).
    pub path: u128,
    /// Time to build trigram index (milliseconds).
    pub trigram: u128,
}

#[cfg(windows)]
pub fn load_live_drive(
    drive_letter: char,
    no_cache: bool,
) -> anyhow::Result<(DriveIndex, LoadTiming)> {
    use anyhow::Context;

    /// Cache TTL in seconds (10 minutes, same as CLI).
    const INDEX_TTL_SECONDS: u64 = 600;

    let mft_start = Instant::now();
    let rt = tokio::runtime::Runtime::new()?;
    let index = rt.block_on(async {
        let reader = uffs_mft::MftReader::open(drive_letter)
            .with_context(|| format!("Failed to open drive {drive_letter}:"))?;
        if no_cache {
            reader
                .read_all_index()
                .await
                .with_context(|| format!("Failed to read MFT fresh for drive {drive_letter}:"))
        } else {
            reader
                .read_index_cached(INDEX_TTL_SECONDS)
                .await
                .with_context(|| format!("Failed to read MFT for drive {drive_letter}:"))
        }
    })?;
    let mft_elapsed = mft_start.elapsed().as_millis();

    let (drive_index, path_elapsed, tri_elapsed) = build_drive_index(drive_letter, index);
    Ok((
        drive_index,
        LoadTiming {
            mft: mft_elapsed,
            path: path_elapsed,
            trigram: tri_elapsed,
        },
    ))
}

/// Build a `DriveIndex` from a loaded `MftIndex` (shared by live + file paths).
///
/// Returns `(DriveIndex, path_resolve_ms, trigram_build_ms)`.
/// Called from both `load_live_drive` (cfg(windows)) and `load_mft_file`,
/// so two call sites exist even though only one is visible per platform.
#[expect(
    clippy::single_call_fn,
    reason = "called from both load_live_drive (cfg(windows)) and load_mft_file"
)]
fn build_drive_index(drive_letter: char, index: MftIndex) -> (DriveIndex, u128, u128) {
    let volume_prefix = format!("{drive_letter}:\\");
    let record_count = index.records.len();

    let path_start = Instant::now();
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
    let path_elapsed = path_start.elapsed().as_millis();

    let tri_start = Instant::now();
    let trigram = TrigramIndex::build(&paths_lower);
    let tri_elapsed = tri_start.elapsed().as_millis();

    (
        DriveIndex {
            letter: drive_letter,
            index,
            trigram,
            paths_lower,
            source: IndexSource::MftFile(PathBuf::from(format!("{drive_letter}:"))),
        },
        path_elapsed,
        tri_elapsed,
    )
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
) -> anyhow::Result<(DriveIndex, LoadTiming)> {
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

    let mft_start = Instant::now();
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
    let mft_elapsed = mft_start.elapsed().as_millis();

    let (drive_index, path_elapsed, tri_elapsed) = build_drive_index(drive_letter, index);
    Ok((
        DriveIndex {
            source: IndexSource::MftFile(mft_path.to_path_buf()),
            ..drive_index
        },
        LoadTiming {
            mft: mft_elapsed,
            path: path_elapsed,
            trigram: tri_elapsed,
        },
    ))
}
