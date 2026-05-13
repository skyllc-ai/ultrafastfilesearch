// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tree-based path search, glob matching, and path resolution.
//!
//! For patterns containing `\` or `/`, decomposes the pattern into path
//! segments and walks the directory tree instead of flat name search.
//! Also provides glob matching (`*`, `?`, `**`) and path resolution
//! via parent chain traversal.

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::compact::DriveCompactIndex;

/// Directory path cache for `resolve_path_cached`.
///
/// Caches resolved directory paths (keyed by record index) so that sibling
/// files sharing the same parent don't re-walk the entire parent chain.
/// For 10K results in the same directory, this eliminates ~90% of parent
/// walks.  Uses `FxHashMap` (3–5× faster than `HashMap` for integer keys).
pub type DirCache = FxHashMap<u32, String>;

/// Trait extension to add `with_capacity` to `DirCache` (`FxHashMap`).
pub(crate) trait DirCacheExt {
    /// Create a new `DirCache` with the given capacity.
    fn with_capacity(capacity: usize) -> Self;
}

impl DirCacheExt for DirCache {
    fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_and_hasher(capacity, FxBuildHasher)
    }
}

/// Resolve a record's full path by walking the parent chain in the compact
/// index.
///
/// Returns path like `C:\Users\Photos\beach.jpg`.
#[must_use]
pub fn resolve_path(drive: &DriveCompactIndex, record_idx: usize, volume_prefix: &str) -> String {
    resolve_path_inner(drive, record_idx, volume_prefix, None)
}

/// Resolve a record's full path with directory caching.
///
/// Same as [`resolve_path`] but checks and populates `dir_cache` during the
/// parent-chain walk.  When a cached ancestor is found, the walk stops
/// early and the cached prefix is reused.  All intermediate directory
/// paths discovered during the walk are added to the cache.
///
/// This mirrors the `PathResolver::materialize_path_cached` pattern used
/// by the `MftIndex` search path.
#[must_use]
pub fn resolve_path_cached(
    drive: &DriveCompactIndex,
    record_idx: usize,
    volume_prefix: &str,
    dir_cache: &mut DirCache,
) -> String {
    resolve_path_inner(drive, record_idx, volume_prefix, Some(dir_cache))
}

/// Shared implementation for cached and uncached path resolution.
fn resolve_path_inner(
    drive: &DriveCompactIndex,
    record_idx: usize,
    volume_prefix: &str,
    dir_cache: Option<&mut DirCache>,
) -> String {
    let mut chain: Vec<usize> = Vec::with_capacity(8);
    let mut current_idx = record_idx;
    let mut depth = 0_u32;
    // Owned copy of cache-hit prefix (avoids borrow-vs-move conflict).
    let mut cache_hit_prefix: Option<String> = None;

    loop {
        if depth > 256 {
            break; // Prevent infinite loops
        }

        // Check cache before walking further.
        if let Some(cache) = dir_cache.as_ref()
            && let Some(cached) = cache.get(&uffs_mft::len_to_u32(current_idx))
        {
            cache_hit_prefix = Some(cached.clone());
            break;
        }

        let Some(record) = drive.records.get(current_idx) else {
            break;
        };

        let name = record.name(&drive.names);
        if name.is_empty() || name == "." {
            break;
        }

        chain.push(current_idx);

        let parent = record.parent_idx;
        if parent == u32::MAX {
            break;
        }

        current_idx = parent as usize;
        depth += 1;
    }

    // Build the path string.
    let prefix = cache_hit_prefix.as_deref().unwrap_or(volume_prefix);
    let suffix_len: usize = chain
        .iter()
        .filter_map(|&idx| {
            let rec = drive.records.get(idx)?;
            let name = rec.name(&drive.names);
            if name.is_empty() || name == "." {
                None
            } else {
                Some(1 + name.len())
            }
        })
        .sum();

    let mut path = String::with_capacity(prefix.len() + suffix_len);
    path.push_str(prefix);
    for &idx in chain.iter().rev() {
        if let Some(rec) = drive.records.get(idx) {
            let name = rec.name(&drive.names);
            if !name.is_empty() && name != "." {
                if !path.ends_with('\\') && !path.is_empty() {
                    path.push('\\');
                }
                path.push_str(name);
            }
        }
    }

    // Populate cache with intermediate directory paths.
    if let Some(cache) = dir_cache {
        // Walk the chain from root-side to leaf-side, building up
        // progressively longer directory prefixes.
        let mut dir_path = String::from(prefix);
        for &idx in chain.iter().rev() {
            if let Some(rec) = drive.records.get(idx) {
                let name = rec.name(&drive.names);
                if name.is_empty() || name == "." {
                    continue;
                }
                if !dir_path.ends_with('\\') && !dir_path.is_empty() {
                    dir_path.push('\\');
                }
                dir_path.push_str(name);
                // Only cache directories — files won't be looked up as parents.
                if rec.is_directory() {
                    cache
                        .entry(uffs_mft::len_to_u32(idx))
                        .or_insert_with(|| dir_path.clone());
                }
            }
        }
    }

    path
}

/// Returns `true` if the pattern contains a path separator (`\` or `/`),
/// indicating it should be handled by tree search rather than name trigram.
#[must_use]
pub(crate) fn is_path_pattern(pattern: &str) -> bool {
    pattern.contains('\\') || pattern.contains('/')
}

/// Search using tree traversal for path patterns like `\photos\*.jpg`.
///
/// Strategy:
/// 1. Split pattern on path separators into segments
/// 2. Find directories matching intermediate segments via trigram + name verify
/// 3. Collect children of those directories
/// 4. Filter leaf matches on the final segment
#[must_use]
pub(crate) fn tree_search(
    drive: &DriveCompactIndex,
    pattern_lower: &str,
    limit: usize,
) -> Vec<u32> {
    // Normalize separators to backslash, strip leading separator
    let normalized = pattern_lower.replace('/', "\\");
    let stripped = normalized.strip_prefix('\\').unwrap_or(&normalized);

    let segments: Vec<&str> = stripped.split('\\').filter(|seg| !seg.is_empty()).collect();

    if segments.is_empty() {
        return Vec::new();
    }

    // Single segment = just a name search, no tree walk needed
    let Some(first_segment) = segments.first() else {
        return Vec::new();
    };
    let fold = drive.fold;
    let mut fold_buf: Vec<u8> = Vec::with_capacity(256);

    if segments.len() == 1 {
        return trigram_filtered_records(drive, first_segment, limit, |rec| {
            let name = rec.name(&drive.names);
            let folded = fold.fold_into(name, &mut fold_buf);
            !folded.is_empty() && folded != "." && folded.contains(first_segment)
        });
    }

    // Multi-segment path search with ** support.
    let Some(leaf_pattern) = segments.last() else {
        return Vec::new();
    };
    let dir_segments = segments.get(..segments.len() - 1).unwrap_or(&[]);

    // Start: first segment determines initial candidate dirs
    let mut candidate_dirs: Vec<u32> = if *first_segment == "**" {
        drive
            .records
            .iter()
            .enumerate()
            .filter(|(_, rec)| rec.is_directory() && rec.name_len > 0)
            .map(|(idx, _)| uffs_mft::len_to_u32(idx))
            .collect()
    } else {
        trigram_filtered_records(drive, first_segment, usize::MAX, |rec| {
            rec.is_directory()
                && segment_matches(
                    fold.fold_into(rec.name(&drive.names), &mut fold_buf),
                    first_segment,
                )
        })
    };

    // Walk through intermediate dir segments
    for &segment in dir_segments.get(1..).unwrap_or(&[]) {
        if segment == "**" {
            let mut all_descendants = Vec::new();
            for &dir_idx in &candidate_dirs {
                collect_descendant_dirs(drive, dir_idx, &mut all_descendants, limit * 10);
            }
            candidate_dirs = all_descendants;
        } else {
            let mut next_dirs = Vec::new();
            for &dir_idx in &candidate_dirs {
                for &child_idx in drive.children.get(dir_idx as usize) {
                    if let Some(child_rec) = drive.records.get(child_idx as usize)
                        && child_rec.is_directory()
                    {
                        let child_name =
                            fold.fold_into(child_rec.name(&drive.names), &mut fold_buf);
                        if segment_matches(child_name, segment) {
                            next_dirs.push(child_idx);
                        }
                    }
                }
            }
            candidate_dirs = next_dirs;
        }
        if candidate_dirs.is_empty() {
            return Vec::new();
        }
    }

    // Collect results
    let mut results = Vec::new();
    if *leaf_pattern == "**" {
        for &dir_idx in &candidate_dirs {
            collect_all_descendants(drive, dir_idx, &mut results, limit);
            if results.len() >= limit {
                break;
            }
        }
    } else {
        for &dir_idx in &candidate_dirs {
            for &child_idx in drive.children.get(dir_idx as usize) {
                if let Some(child_rec) = drive.records.get(child_idx as usize) {
                    let child_name = fold.fold_into(child_rec.name(&drive.names), &mut fold_buf);
                    if name_matches(child_name, leaf_pattern) {
                        results.push(child_idx);
                        if results.len() >= limit {
                            return results;
                        }
                    }
                }
            }
        }
    }

    results
}

/// Recursively collect all descendant DIRECTORY indices from a directory.
fn collect_descendant_dirs(
    drive: &DriveCompactIndex,
    dir_idx: u32,
    out: &mut Vec<u32>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }
    for &child_idx in drive.children.get(dir_idx as usize) {
        if let Some(child_rec) = drive.records.get(child_idx as usize)
            && child_rec.is_directory()
            && child_rec.name_len > 0
        {
            out.push(child_idx);
            if out.len() >= max {
                return;
            }
            collect_descendant_dirs(drive, child_idx, out, max);
        }
    }
}

/// Recursively collect ALL descendants (files + dirs) from a directory.
fn collect_all_descendants(
    drive: &DriveCompactIndex,
    dir_idx: u32,
    out: &mut Vec<u32>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }
    for &child_idx in drive.children.get(dir_idx as usize) {
        if let Some(child_rec) = drive.records.get(child_idx as usize)
            && child_rec.name_len > 0
        {
            let name = child_rec.name(&drive.names);
            if !name.is_empty() && name != "." {
                out.push(child_idx);
                if out.len() >= max {
                    return;
                }
            }
            if child_rec.is_directory() {
                collect_all_descendants(drive, child_idx, out, max);
            }
        }
    }
}

/// Search records using trigram pre-filter and a predicate.
///
/// If a trigram candidate set exists for `needle`, only those records are
/// checked; otherwise a full scan is performed, capped at `limit`.
fn trigram_filtered_records(
    drive: &DriveCompactIndex,
    needle: &str,
    limit: usize,
    mut predicate: impl FnMut(&crate::compact::CompactRecord) -> bool,
) -> Vec<u32> {
    let candidates = drive.trigram.search(needle, drive.fold);
    match candidates {
        None => drive
            .records
            .iter()
            .enumerate()
            .filter(|(_, rec)| predicate(rec))
            .take(limit)
            .map(|(idx, _)| uffs_mft::len_to_u32(idx))
            .collect(),
        Some(candidate_indices) => {
            let mut out = Vec::with_capacity(candidate_indices.len().min(limit));
            for &idx in &candidate_indices {
                if out.len() >= limit {
                    break;
                }
                if let Some(rec) = drive.records.get(idx as usize)
                    && predicate(rec)
                {
                    out.push(idx);
                }
            }
            out
        }
    }
}

/// Check if a name matches a glob pattern (case-insensitive, both already
/// lowercase).
///
/// Supports:
/// - `*`: matches any sequence of characters (including empty)
/// - `?`: matches exactly one character
/// - Multiple wildcards: `*sex*Ge*` matches "I want your Sex - George Michael"
/// - OR operator: `*.rs|*.py` → match if ANY sub-pattern matches
/// - No wildcards: plain substring match
#[must_use]
pub(crate) fn name_matches(name: &str, pattern: &str) -> bool {
    if name.is_empty() || name == "." {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    // OR operator: `*.rs|*.py` → match if ANY sub-pattern matches
    if pattern.contains('|') {
        return pattern.split('|').any(|sub| name_matches_single(name, sub));
    }
    name_matches_single(name, pattern)
}

/// Match a single pattern (no `|` alternation) against a filename.
fn name_matches_single(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        // No wildcards → substring match
        return name.contains(pattern);
    }
    glob_match(name.as_bytes(), pattern.as_bytes())
}

/// Match a path segment exactly against a directory/file name.
///
/// Unlike [`name_matches`] which does substring matching for bare literals
/// (search behaviour), this requires an **exact** match for non-glob segments.
#[must_use]
pub(crate) fn segment_matches(name: &str, segment: &str) -> bool {
    if name.is_empty() || name == "." {
        return false;
    }
    if segment == "*" || segment == "**" {
        return true;
    }
    if !segment.contains('*') && !segment.contains('?') {
        return name == segment;
    }
    glob_match(name.as_bytes(), segment.as_bytes())
}

/// Iterative glob matching: `*` matches any sequence, `?` matches one byte.
#[expect(
    clippy::indexing_slicing,
    reason = "all index accesses are bounds-checked by the while/if conditions"
)]
fn glob_match(text: &[u8], pattern: &[u8]) -> bool {
    let mut ti = 0_usize;
    let mut pi = 0_usize;
    let mut last_star_p = usize::MAX;
    let mut last_star_t = 0_usize;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            last_star_p = pi;
            last_star_t = ti;
            pi += 1;
        } else if last_star_p != usize::MAX {
            pi = last_star_p + 1;
            last_star_t += 1;
            ti = last_star_t;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}
