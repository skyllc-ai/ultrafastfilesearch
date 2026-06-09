// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Record-index → [`DisplayRow`] path resolution.
//!
//! Extracted from `mod.rs` to keep that file under the 800-LOC file-size
//! policy. Hosts the size-adaptive dispatch ([`indices_to_rows`]) and its
//! sequential / parallel variants, plus the chunk-size constant that doubles
//! as the sequential-vs-parallel threshold.

use rayon::prelude::*;

use super::{DisplayRow, make_display_row, row_forensics};
use crate::compact::DriveCompactIndex;
use crate::search::tree;

/// Chunk size for parallel path resolution. At ~370 ns per candidate,
/// a 4 K chunk runs in ~1.5 ms — well above rayon's task-dispatch floor
/// (~1 μs). Also the threshold below which path resolution stays
/// sequential: tiny result sets (e.g. an `exact` query returning a
/// handful of rows) must NOT pay rayon's submission cost, which shows up
/// as p95 tail jitter rather than mean latency. Above it, prefix /
/// substring queries (10 K–35 K rows) fan out across workers.
const RESOLVE_CHUNK_SIZE: usize = 4096;

/// Convert a list of record indices into `DisplayRow`s with resolved paths.
///
/// Dispatches on result-set size: small sets (`< RESOLVE_CHUNK_SIZE`) resolve
/// sequentially to avoid rayon's task-submission cost (which would otherwise
/// surface as p95 tail jitter on tiny `exact` queries), while large sets fan
/// out across rayon workers with one `DirCache` per chunk.
pub(crate) fn indices_to_rows(
    drive: &DriveCompactIndex,
    indices: &[u32],
    volume_prefix: &str,
) -> Vec<DisplayRow> {
    // Parallel overhead is only worth it above a chunk's worth of candidates.
    if indices.len() < RESOLVE_CHUNK_SIZE {
        return indices_to_rows_sequential(drive, indices, volume_prefix);
    }
    indices_to_rows_parallel(drive, indices, volume_prefix)
}

/// Sequential path resolution for small candidate sets (`<
/// RESOLVE_CHUNK_SIZE`).
fn indices_to_rows_sequential(
    drive: &DriveCompactIndex,
    indices: &[u32],
    volume_prefix: &str,
) -> Vec<DisplayRow> {
    let mut dir_cache = tree::dir_cache_with_capacity(256);
    let mut mal_cache = tree::malformed_cache_with_capacity(256);
    indices
        .iter()
        .filter_map(|&record_idx| {
            let rec = drive.records.get(record_idx as usize)?;
            let name = rec.name(&drive.names);
            if name.is_empty() {
                return None;
            }
            let (path, path_malformed) = tree::resolve_path_cached_with_malformed(
                drive,
                record_idx as usize,
                volume_prefix,
                &mut dir_cache,
                &mut mal_cache,
            );
            let forensics = row_forensics(rec, &drive.names, path_malformed);
            Some(make_display_row(
                record_idx,
                drive.letter,
                rec,
                name,
                path,
                forensics,
            ))
        })
        .collect()
}

/// Parallel path resolution for large candidate sets using rayon.
///
/// Each chunk owns its own `DirCache` / `MalformedCache` so workers never
/// contend; sibling records within a chunk keep the cache warm. Chunk-local
/// row vectors are concatenated in order via `reduce`, preserving the input
/// ordering exactly so the downstream sort sees the same candidate sequence
/// the sequential path would produce.
fn indices_to_rows_parallel(
    drive: &DriveCompactIndex,
    indices: &[u32],
    volume_prefix: &str,
) -> Vec<DisplayRow> {
    indices
        .par_chunks(RESOLVE_CHUNK_SIZE)
        .map(|chunk| {
            let mut dir_cache = tree::dir_cache_with_capacity(256);
            let mut mal_cache = tree::malformed_cache_with_capacity(256);
            let mut local_rows = Vec::with_capacity(chunk.len());

            for &record_idx in chunk {
                let Some(rec) = drive.records.get(record_idx as usize) else {
                    continue;
                };
                let name = rec.name(&drive.names);
                if name.is_empty() {
                    continue;
                }
                let (path, path_malformed) = tree::resolve_path_cached_with_malformed(
                    drive,
                    record_idx as usize,
                    volume_prefix,
                    &mut dir_cache,
                    &mut mal_cache,
                );
                let forensics = row_forensics(rec, &drive.names, path_malformed);
                local_rows.push(make_display_row(
                    record_idx,
                    drive.letter,
                    rec,
                    name,
                    path,
                    forensics,
                ));
            }
            local_rows
        })
        .reduce(Vec::new, |mut acc, mut chunk_rows| {
            acc.append(&mut chunk_rows);
            acc
        })
}
