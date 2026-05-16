// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Chunk planning helpers for extent-aware MFT reads.

use tracing::{debug, info, trace, warn};

use super::MftExtentMap;

/// A read chunk representing a contiguous range of MFT records.
#[derive(Debug, Clone)]
pub struct ReadChunk {
    /// Physical byte offset on disk.
    pub disk_offset: u64,
    /// First FRS in this chunk.
    pub start_frs: u64,
    /// Number of records in this chunk.
    pub record_count: u64,
    /// Number of records to skip at the beginning (all unused).
    pub skip_begin: u64,
    /// Number of records to skip at the end (all unused).
    pub skip_end: u64,
}

impl ReadChunk {
    /// Returns the effective first FRS (after skipping unused records).
    #[must_use]
    pub const fn effective_start_frs(&self) -> u64 {
        self.start_frs + self.skip_begin
    }

    /// Returns the effective record count (excluding skipped records).
    #[must_use]
    pub const fn effective_record_count(&self) -> u64 {
        self.record_count
            .saturating_sub(self.skip_begin + self.skip_end)
    }

    /// Returns the byte size to read (after accounting for skips).
    #[must_use]
    pub fn read_size(&self, record_size: u32) -> u64 {
        self.effective_record_count() * u64::from(record_size)
    }
}

/// Generates optimized read chunks for the MFT.
///
/// This function implements the optimization of:
/// 1. Splitting the MFT into chunks based on extents
/// 2. Using the bitmap to skip clusters with no in-use records
/// 3. Calculating `skip_begin/skip_end` for each chunk
///
/// # Arguments
///
/// * `extent_map` - The MFT extent map
/// * `bitmap` - Optional bitmap for skip optimization
/// * `chunk_size` - Target chunk size in bytes (default 1MB)
///
/// # Returns
///
/// Vector of read chunks optimized for I/O.
pub fn generate_read_chunks(
    extent_map: &MftExtentMap,
    bitmap: Option<&crate::platform::MftBitmap>,
    chunk_size: usize,
) -> Vec<ReadChunk> {
    let mut chunks = Vec::new();
    let record_size = extent_map.bytes_per_record;
    let cluster_size = extent_map.bytes_per_cluster;
    let records_per_cluster = cluster_size / record_size;

    let num_extents = extent_map.extent_count();
    let mut sparse_extents = 0_u64;
    let mut total_records_to_read = 0_u64;
    let mut total_records_skipped = 0_u64;

    debug!(
        num_extents,
        record_size, cluster_size, records_per_cluster, chunk_size, "📐 Generating read chunks"
    );

    for (extent_idx, extent) in extent_map.extents().enumerate() {
        if extent.lcn.is_hole() {
            sparse_extents += 1;
            trace!(extent_idx, vcn = extent.vcn, "Skipping sparse extent");
            continue;
        }

        split_extent_into_chunks(
            extent,
            records_per_cluster,
            record_size,
            cluster_size,
            chunk_size,
            bitmap,
            &mut chunks,
            &mut total_records_to_read,
            &mut total_records_skipped,
        );
    }

    if sparse_extents > 0 {
        debug!(sparse_extents, "Skipped sparse extents");
    }

    let merged_chunks = maybe_merge_chunks(chunks, record_size, bitmap.is_some());

    log_chunk_summary(
        &merged_chunks,
        total_records_to_read,
        total_records_skipped,
        bitmap.is_some(),
    );

    merged_chunks
}

/// Split a single non-sparse extent into `chunk_size`-bounded read chunks.
#[expect(
    clippy::too_many_arguments,
    reason = "pass-through of parent context; struct would add indirection"
)]
fn split_extent_into_chunks(
    extent: &crate::platform::MftExtent,
    records_per_cluster: u32,
    record_size: u32,
    cluster_size: u32,
    chunk_size: usize,
    bitmap: Option<&crate::platform::MftBitmap>,
    chunks: &mut Vec<ReadChunk>,
    total_to_read: &mut u64,
    total_skipped: &mut u64,
) {
    let extent_start_frs = extent.vcn * u64::from(records_per_cluster);
    let extent_records = extent.cluster_count * u64::from(records_per_cluster);
    let extent_disk_offset = extent.lcn.raw_unsigned() * u64::from(cluster_size);
    let records_per_chunk = (chunk_size / record_size as usize) as u64;

    let mut chunk_start = 0_u64;
    while chunk_start < extent_records {
        let chunk_records = (extent_records - chunk_start).min(records_per_chunk);
        let chunk_frs_start = extent_start_frs + chunk_start;
        let chunk_frs_end = chunk_frs_start + chunk_records;

        // Bitmap edge-trimming disabled: the bitmap is stale for records at
        // chunk boundaries — the IN_USE flag in each record header is the
        // authoritative filter. Full chunks are always read.
        let (skip_begin, skip_end) = (0_u64, 0_u64);
        log_bitmap_diagnostic(bitmap, chunk_frs_start, chunk_frs_end);

        let effective = chunk_records - skip_begin - skip_end;
        *total_to_read += effective;
        *total_skipped += skip_begin + skip_end;

        chunks.push(ReadChunk {
            disk_offset: extent_disk_offset + chunk_start * u64::from(record_size),
            start_frs: chunk_frs_start,
            record_count: chunk_records,
            skip_begin,
            skip_end,
        });

        chunk_start += chunk_records;
    }
}

/// Log bitmap skip diagnostics at trace level (informational only).
fn log_bitmap_diagnostic(
    bitmap: Option<&crate::platform::MftBitmap>,
    frs_start: u64,
    frs_end: u64,
) {
    if let Some(bm) = bitmap {
        let diag = bm.calculate_skip_range(frs_start, frs_end);
        if diag.0 > 0 || diag.1 > 0 {
            trace!(
                frs_start,
                frs_end,
                bitmap_skip_begin = diag.0,
                bitmap_skip_end = diag.1,
                "Bitmap suggests skipping (ignored — reading full chunk for correctness)"
            );
        }
    }
}

/// Log a summary of the generated read plan.
fn log_chunk_summary(
    chunks: &[ReadChunk],
    records_to_read: u64,
    records_skipped: u64,
    bitmap_used: bool,
) {
    let pct = percentage_f64(records_skipped, records_to_read + records_skipped);

    info!(
        chunks = chunks.len(),
        records_to_read,
        records_skipped,
        skip_percentage = format!("{:.1}%", pct),
        bitmap_used,
        "📊 Read plan generated"
    );

    if records_skipped > 0 {
        warn!(
            records_skipped,
            skip_percentage = format!("{:.1}%", pct),
            "⚠️  {} records will be skipped based on bitmap",
            records_skipped
        );
    }
}

/// Generate precise read chunks for `NVMe`/SSD drives.
///
/// Unlike `generate_read_chunks` which creates large chunks and only skips at
/// the beginning/end, this function generates smaller, more precise I/O
/// operations that skip unused regions entirely.
///
/// For `NVMe`/SSD drives, there's no seek penalty, so many small reads are
/// actually more efficient than fewer large reads that include unused data.
///
/// # Arguments
///
/// * `extent_map` - MFT extent map for physical offset calculation
/// * `bitmap` - MFT bitmap indicating which records are in use
/// * `max_io_size` - Maximum I/O size (e.g., 4MB for `NVMe`)
/// * `min_gap_records` - Minimum gap size to split (smaller gaps are read
///   through)
///
/// # Returns
///
/// Vector of read chunks optimized for `NVMe`/SSD I/O.
pub fn generate_precise_read_chunks(
    extent_map: &MftExtentMap,
    bitmap: &crate::platform::MftBitmap,
    max_io_size: usize,
    min_gap_records: usize,
) -> Vec<ReadChunk> {
    let record_size = extent_map.bytes_per_record;
    let cluster_size = extent_map.bytes_per_cluster;
    let records_per_cluster = cluster_size / record_size;

    let cluster_ranges: Vec<(u64, u64)> =
        bitmap.in_use_cluster_ranges(records_per_cluster).collect();

    if cluster_ranges.is_empty() {
        info!("📊 No in-use clusters found in bitmap");
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut total_records_to_read = 0_u64;
    let mut total_records_skipped = 0_u64;

    for extent in extent_map.extents() {
        if extent.lcn.is_hole() {
            continue;
        }

        split_extent_into_precise_chunks(
            extent,
            &cluster_ranges,
            records_per_cluster,
            record_size,
            cluster_size,
            max_io_size,
            bitmap,
            &mut chunks,
            &mut total_records_to_read,
            &mut total_records_skipped,
        );
    }

    // Merge adjacent chunks with small gaps (for NVMe, small gaps are cheaper to
    // read through)
    let min_gap_bytes = min_gap_records as u64 * u64::from(record_size);
    let merged_chunks = merge_precise_chunks(chunks, record_size, min_gap_bytes, max_io_size);

    let total_records = total_records_to_read + total_records_skipped;
    info!(
        chunks = merged_chunks.len(),
        records_to_read = total_records_to_read,
        records_skipped = total_records_skipped,
        skip_percentage = format!(
            "{:.1}%",
            percentage_f64(total_records_skipped, total_records)
        ),
        "📊 Precise read plan generated (NVMe/SSD optimized)"
    );

    merged_chunks
}

/// Split a single extent's overlap with in-use cluster ranges into
/// `max_io_size`-bounded read chunks and accumulate into `chunks`.
#[expect(
    clippy::too_many_arguments,
    reason = "pass-through of parent context; struct would add indirection for no clarity"
)]
fn split_extent_into_precise_chunks(
    extent: &crate::platform::MftExtent,
    cluster_ranges: &[(u64, u64)],
    records_per_cluster: u32,
    record_size: u32,
    cluster_size: u32,
    max_io_size: usize,
    bitmap: &crate::platform::MftBitmap,
    chunks: &mut Vec<ReadChunk>,
    total_to_read: &mut u64,
    total_skipped: &mut u64,
) {
    let rpc = u64::from(records_per_cluster);
    let extent_start_frs = extent.vcn * rpc;
    let extent_end_frs = extent_start_frs + extent.cluster_count * rpc;
    let extent_disk_offset = extent.lcn.raw_unsigned() * u64::from(cluster_size);
    let records_per_io = max_io_size / record_size as usize;

    for &(range_start_cluster, range_cluster_count) in cluster_ranges {
        let range_start_frs = range_start_cluster * rpc;
        let range_end_frs = range_start_frs + range_cluster_count * rpc;

        // No overlap with this extent
        if range_end_frs <= extent_start_frs || range_start_frs >= extent_end_frs {
            continue;
        }

        // Clip range to extent boundaries
        let clipped_start = range_start_frs.max(extent_start_frs);
        let clipped_end = range_end_frs.min(extent_end_frs);
        let clipped_records = clipped_end - clipped_start;
        if clipped_records == 0 {
            continue;
        }

        let offset_in_extent = (clipped_start - extent_start_frs) * u64::from(record_size);
        let disk_offset = extent_disk_offset + offset_in_extent;

        emit_io_chunks(
            clipped_start,
            clipped_records,
            disk_offset,
            records_per_io as u64,
            record_size,
            bitmap,
            chunks,
            total_to_read,
            total_skipped,
        );
    }
}

/// Subdivide a contiguous record range into `records_per_io`-sized read
/// chunks, appending each to `chunks`.
#[expect(
    clippy::too_many_arguments,
    reason = "accumulator pattern — struct would add indirection for a single call site"
)]
fn emit_io_chunks(
    start_frs: u64,
    total_records: u64,
    base_disk_offset: u64,
    records_per_io: u64,
    record_size: u32,
    bitmap: &crate::platform::MftBitmap,
    chunks: &mut Vec<ReadChunk>,
    total_to_read: &mut u64,
    total_skipped: &mut u64,
) {
    let mut pos = 0_u64;
    while pos < total_records {
        let chunk_records = (total_records - pos).min(records_per_io);
        let chunk_frs_start = start_frs + pos;
        let chunk_frs_end = chunk_frs_start + chunk_records;

        // Bitmap edge-trimming disabled: same rationale as generate_read_chunks().
        // The bitmap is stale at chunk boundaries, causing valid IN_USE records
        // to be skipped. Always read full chunks; IN_USE flag is authoritative.
        let (skip_begin, skip_end) = (0_u64, 0_u64);
        let diag = bitmap.calculate_skip_range(chunk_frs_start, chunk_frs_end);
        if diag.0 > 0 || diag.1 > 0 {
            trace!(
                chunk_frs_start,
                chunk_frs_end,
                bitmap_skip_begin = diag.0,
                bitmap_skip_end = diag.1,
                "Bitmap suggests skipping (ignored — reading full chunk for correctness)"
            );
        }

        let effective = chunk_records - skip_begin - skip_end;
        if effective > 0 {
            *total_to_read += effective;
            *total_skipped += skip_begin + skip_end;
            chunks.push(ReadChunk {
                disk_offset: base_disk_offset + pos * u64::from(record_size),
                start_frs: chunk_frs_start,
                record_count: chunk_records,
                skip_begin,
                skip_end,
            });
        } else {
            *total_skipped += chunk_records;
        }

        pos += chunk_records;
    }
}

/// Merge adjacent chunks when no bitmap is available.
///
/// With bitmap: keep all chunks to preserve per-chunk skip optimization.
/// Without bitmap: merge adjacent chunks within a gap threshold to reduce I/O
/// ops.
fn maybe_merge_chunks(
    chunks: Vec<ReadChunk>,
    record_size: u32,
    has_bitmap: bool,
) -> Vec<ReadChunk> {
    if has_bitmap {
        // Keep all chunks to preserve per-chunk skip optimization.
        // Per-~1MB-chunk skip gives ~23,000 skip opportunities
        // for 11.5GB MFT, vs only 124 if merged into 62 extent-sized chunks.
        return chunks;
    }

    let merge_threshold = 64_u64; // Records — about 64KB at 1024 bytes/record
    let before = chunks.len();
    let merged = merge_adjacent_chunks(chunks, record_size, merge_threshold);
    let after = merged.len();

    if before != after {
        debug!(
            before,
            after,
            merged_count = before - after,
            "🔗 Merged adjacent chunks"
        );
    }

    merged
}

/// Merge adjacent precise chunks with small gaps.
///
/// For `NVMe`/SSD, if two chunks are very close together, it's more efficient
/// to read them as one chunk than to issue two separate I/O operations.
fn merge_precise_chunks(
    mut chunks: Vec<ReadChunk>,
    record_size: u32,
    min_gap_bytes: u64,
    max_io_size: usize,
) -> Vec<ReadChunk> {
    if chunks.len() < 2 {
        return chunks;
    }

    // Sort by disk offset for merging
    chunks.sort_by_key(|ch| ch.disk_offset);

    let mut merged = Vec::with_capacity(chunks.len());
    let mut current = chunks.remove(0);

    for next in chunks {
        let current_end_offset =
            current.disk_offset + current.record_count * u64::from(record_size);

        // Check if chunks are close enough to merge
        let gap_bytes = next.disk_offset.saturating_sub(current_end_offset);
        let merged_bytes =
            (next.disk_offset + next.record_count * u64::from(record_size)) - current.disk_offset;

        if gap_bytes <= min_gap_bytes && merged_bytes <= max_io_size as u64 {
            // Merge: extend current chunk to include next
            let new_record_count = ((next.disk_offset
                + next.record_count * u64::from(record_size))
                - current.disk_offset)
                / u64::from(record_size);
            current.record_count = new_record_count;
            current.skip_end = next.skip_end;
        } else {
            // Can't merge, push current and start new
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);

    merged
}

/// M1 8.6: Merge adjacent chunks with small gaps.
///
/// When two chunks are close together (gap < threshold), reading them as one
/// chunk is more efficient than two separate I/O operations. The overhead of
/// reading a few extra unused records is less than the syscall overhead.
///
/// **Important**: Merged chunks are capped at `MAX_CHUNK_BYTES` (1GB) to avoid
/// exceeding the Windows `ReadFile` API's 4GB buffer limit (`u32::MAX`).
fn merge_adjacent_chunks(
    mut chunks: Vec<ReadChunk>,
    record_size: u32,
    threshold: u64,
) -> Vec<ReadChunk> {
    // Maximum merged chunk size: 1GB (well below u32::MAX to be safe)
    // Windows ReadFile API takes buffer length as u32, so >4GB would panic.
    const MAX_CHUNK_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB

    if chunks.len() < 2 {
        return chunks;
    }

    let mut merged = Vec::with_capacity(chunks.len());
    let mut current = chunks.remove(0);

    for next in chunks {
        // Check if chunks are PHYSICALLY adjacent on disk.
        // This is critical for fragmented MFTs where FRS numbers may be contiguous
        // but disk locations are NOT (e.g., extent 4 at LCN 9M, extent 5 at LCN 3M).
        //
        // BUG FIX: Previously we only checked if gap_bytes (using saturating_sub) was
        // small, but saturating_sub returns 0 when next.disk_offset <
        // current_end_offset, causing chunks from different extents to be
        // incorrectly merged.
        let current_end_offset =
            current.disk_offset + current.record_count * u64::from(record_size);

        // Check for physical contiguity: next chunk must start at or very close to
        // where current chunk ends. We check BOTH directions to catch non-contiguous
        // extents regardless of their relative disk positions.
        let is_physically_contiguous = if next.disk_offset >= current_end_offset {
            // Normal case: next chunk is after current on disk
            let gap_bytes = next.disk_offset - current_end_offset;
            gap_bytes <= threshold * u64::from(record_size)
        } else {
            // Next chunk is BEFORE current on disk - NOT contiguous!
            // This happens with fragmented MFTs where extents are scattered.
            false
        };

        // Also check if they're in the same extent (contiguous FRS range)
        let current_end_frs = current.start_frs + current.record_count;
        let frs_gap = next.start_frs.saturating_sub(current_end_frs);
        let is_frs_contiguous = frs_gap <= threshold;

        // Calculate merged size to check against limit
        let new_record_count = (next.start_frs + next.record_count) - current.start_frs;
        let merged_bytes = new_record_count * u64::from(record_size);

        // Only merge if BOTH physically contiguous AND FRS contiguous
        if is_physically_contiguous && is_frs_contiguous && merged_bytes <= MAX_CHUNK_BYTES {
            // Merge: extend current chunk to include next
            current.record_count = new_record_count;
            // Update skip_end to be from the merged chunk
            current.skip_end = next.skip_end;
        } else {
            // Not contiguous or merged chunk would exceed size limit
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);

    merged
}

/// Compute `(part / total) * 100.0` as `f64` for display percentages.
///
/// Returns `0.0` when `total == 0`. Precision loss from `u64→f64` is
/// irrelevant for human-readable percentage display.
fn percentage_f64(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0_f64
    } else {
        #[expect(
            clippy::float_arithmetic,
            reason = "display-only percentage calculation"
        )]
        let pct = (crate::index::u64_to_f64(part) / crate::index::u64_to_f64(total)) * 100.0_f64;
        pct
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known-valid indices"
)]
mod tests {
    use super::*;

    #[test]
    fn merge_adjacent_chunks_contiguous() {
        let chunks = vec![
            ReadChunk {
                disk_offset: 0,
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 100 * 1024,
                start_frs: 100,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(merged.len(), 1, "Contiguous chunks should be merged");
        assert_eq!(merged[0].start_frs, 0);
        assert_eq!(merged[0].record_count, 200);
        assert_eq!(merged[0].disk_offset, 0);
    }

    #[test]
    fn merge_adjacent_chunks_non_contiguous_disk() {
        let chunks = vec![
            ReadChunk {
                disk_offset: 1_000_000_000,
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 500_000_000,
                start_frs: 100,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(
            merged.len(),
            2,
            "Non-contiguous disk chunks should NOT be merged"
        );
        assert_eq!(merged[0].disk_offset, 1_000_000_000);
        assert_eq!(merged[0].record_count, 100);
        assert_eq!(merged[1].disk_offset, 500_000_000);
        assert_eq!(merged[1].record_count, 100);
    }

    #[test]
    fn merge_adjacent_chunks_gap_too_large() {
        let chunks = vec![
            ReadChunk {
                disk_offset: 0,
                start_frs: 0,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
            ReadChunk {
                disk_offset: 200 * 1024,
                start_frs: 200,
                record_count: 100,
                skip_begin: 0,
                skip_end: 0,
            },
        ];

        let merged = merge_adjacent_chunks(chunks, 1024, 64);
        assert_eq!(
            merged.len(),
            2,
            "Chunks with large gaps should NOT be merged"
        );
    }
}
