// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Prefix search using trigram-accelerated lookup.
//!
//! Extracted from `mod.rs` to satisfy the 800-LOC file-size policy.

use crate::compact::DriveCompactIndex;
use crate::search::query::{indices_to_rows, stack_volume_prefix};

/// Whether cache profiling is enabled (`UFFS_CACHE_PROFILE` env var).
static CACHE_PROFILE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("UFFS_CACHE_PROFILE").is_some());

/// Search a single drive using trigram index for prefix queries (e.g., `win*`).
///
/// Uses the first 3 characters of the prefix to narrow candidates via the
/// trigram index, then filters by full prefix match. Significantly faster
/// than full scan for large drives.
///
/// The caller guarantees `prefix` came from
/// [`crate::search::tree::is_prefix_pattern`], so it is ≥ 3 bytes and free of
/// wildcards / path separators.
#[must_use]
pub(crate) fn search_compact_drive_prefix(
    drive: &DriveCompactIndex,
    prefix: &str,
    limit: usize,
    case_sensitive: bool,
    filters: &crate::search::filters::SearchFilters,
) -> Vec<super::DisplayRow> {
    let mut vp_buf = [0_u8; 4];
    let volume_prefix = stack_volume_prefix(&mut vp_buf, drive.letter);
    let profile = *CACHE_PROFILE;

    // Resolve the extension filter for THIS drive up front so the per-record
    // filter runs BEFORE the `limit` cutoff (see `search_compact_drive`).
    let mut local_filters = filters.clone();
    local_filters.resolve_ext_ids_for_drive(drive);
    let mut filter_buf: Vec<u8> = Vec::with_capacity(256);

    let t_tri = std::time::Instant::now();

    // Get trigram candidates using first 3 chars of prefix.
    // get() safely handles any byte boundaries; prefix is ASCII from pattern.
    let trigram_needle = prefix.get(..prefix.len().min(3)).unwrap_or(prefix);
    let candidates = drive.trigram_search(trigram_needle);

    let tri_ms = t_tri.elapsed().as_millis();
    let tri_count = candidates.as_ref().map_or(0, Vec::len);

    let t_match = std::time::Instant::now();
    let mut match_indices = Vec::new();

    if let Some(candidate_indices) = candidates {
        // Pre-fold the prefix for case-insensitive matching.
        let mut fold_buf: Vec<u8> = Vec::with_capacity(prefix.len());
        let prefix_folded = if case_sensitive {
            prefix.to_owned()
        } else {
            drive.fold.fold_into(prefix, &mut fold_buf).to_owned()
        };

        for rec_idx in candidate_indices {
            let Some(rec) = drive.records.get(rec_idx as usize) else {
                continue;
            };

            let name = rec.name(&drive.names);
            if name.is_empty() {
                continue;
            }

            // Check prefix match.
            let matches = if case_sensitive {
                name.starts_with(prefix)
            } else {
                let mut name_buf: Vec<u8> = Vec::with_capacity(name.len());
                let name_folded = drive.fold.fold_into(name, &mut name_buf);
                name_folded.starts_with(&prefix_folded)
            };

            if matches
                && local_filters.matches_record(rec, &drive.names, &mut filter_buf, drive.fold)
            {
                match_indices.push(rec_idx);
                if match_indices.len() >= limit {
                    break;
                }
            }
        }
    }

    let match_ms = t_match.elapsed().as_millis();
    let match_count = match_indices.len();

    let t_resolve = std::time::Instant::now();
    let rows = indices_to_rows(drive, &match_indices, volume_prefix);
    let resolve_ms = t_resolve.elapsed().as_millis();

    if profile {
        tracing::debug!(
            target: "cache_profile",
            drive = %drive.letter,
            tri_ms = %tri_ms,
            tri_count,
            match_ms = %match_ms,
            match_count,
            resolve_ms = %resolve_ms,
            prefix = %prefix,
            "search_prefix"
        );
    }

    rows
}
