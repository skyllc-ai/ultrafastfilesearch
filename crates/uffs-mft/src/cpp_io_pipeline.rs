//! # C++ I/O Pipeline Port
//!
//! This module provides a faithful port of the C++ MFT I/O pipeline from
//! `mft_reader.hpp`. The key difference from the current Rust implementation
//! is the **synchronization point** after bitmap reading completes.
//!
//! ## C++ I/O Pipeline Architecture
//!
//! The C++ implementation uses a two-phase I/O model:
//!
//! 1. **Phase 1: Bitmap Reading** - Read $MFT::$BITMAP (sync or async)
//! 2. **Synchronization Point** - After ALL bitmap is read:
//!    - Recalculate skip_begin/skip_end for ALL data chunks
//!    - Store atomically so data reads use updated values
//! 3. **Phase 2: Data Reading** - Read $MFT::$DATA chunks with correct skip ranges
//!
//! ## Why This Matters
//!
//! The current Rust implementation calculates skip ranges during chunk generation
//! (in `generate_read_chunks`), before the bitmap is fully processed. This causes
//! ~40 files to be missed compared to C++.
//!
//! ## C++ Source Reference
//!
//! - `mft_reader.hpp` lines 40-63: `RetPtr` struct with atomic skip ranges
//! - `mft_reader.hpp` lines 245-296: Synchronization point after bitmap completes
//! - `mft_reader.hpp` lines 321-386: `queue_next()` - bitmap first, then data

#![cfg(windows)]

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tracing::{debug, info, trace, warn};
use windows::Win32::Foundation::{HANDLE, ERROR_IO_PENDING, GetLastError};
use windows::Win32::Storage::FileSystem::ReadFile;
use windows::Win32::System::IO::GetQueuedCompletionStatus;

use crate::cpp_types::CppParsePipeline;
use crate::error::Result;
use crate::io::{AlignedBuffer, IoCompletionPort, MftExtentMap};
use crate::platform::MftBitmap;

// ============================================================================
// CppDataChunk - Matches C++ RetPtr struct
// ============================================================================

/// MFT data chunk with atomic skip ranges (matches C++ `RetPtr`).
///
/// This struct represents a contiguous range of MFT records to read.
/// The skip ranges are initially 0 and are updated atomically after
/// the bitmap reading phase completes.
///
/// # C++ Reference
///
/// ```cpp
/// struct RetPtr {
///     unsigned long long vcn, cluster_count;
///     long long lcn;
///     atomic<unsigned long long> skip_begin, skip_end;
/// };
/// ```
#[derive(Debug)]
pub struct CppDataChunk {
    /// Virtual Cluster Number (VCN) - offset in the MFT file.
    pub vcn: u64,
    /// Number of clusters in this chunk.
    pub cluster_count: u64,
    /// Logical Cluster Number (LCN) - physical disk location.
    pub lcn: i64,
    /// Number of clusters to skip at the beginning (all records unused).
    /// Updated atomically after bitmap completes.
    pub skip_begin: AtomicU64,
    /// Number of clusters to skip at the end (all records unused).
    /// Updated atomically after bitmap completes.
    pub skip_end: AtomicU64,
}

impl CppDataChunk {
    /// Creates a new data chunk with zero skip ranges.
    ///
    /// Skip ranges will be updated after bitmap reading completes.
    #[must_use]
    pub fn new(vcn: u64, cluster_count: u64, lcn: i64) -> Self {
        Self {
            vcn,
            cluster_count,
            lcn,
            skip_begin: AtomicU64::new(0),
            skip_end: AtomicU64::new(0),
        }
    }

    /// Returns the effective cluster count (excluding skipped clusters).
    #[must_use]
    pub fn effective_cluster_count(&self) -> u64 {
        let skip_begin = self.skip_begin.load(Ordering::Acquire);
        let skip_end = self.skip_end.load(Ordering::Acquire);
        self.cluster_count.saturating_sub(skip_begin + skip_end)
    }

    /// Returns the effective LCN (after skipping begin clusters).
    #[must_use]
    pub fn effective_lcn(&self) -> i64 {
        let skip_begin = self.skip_begin.load(Ordering::Acquire);
        self.lcn + skip_begin as i64
    }

    /// Returns the effective VCN (after skipping begin clusters).
    #[must_use]
    pub fn effective_vcn(&self) -> u64 {
        let skip_begin = self.skip_begin.load(Ordering::Acquire);
        self.vcn + skip_begin
    }

    /// Updates the skip ranges atomically.
    ///
    /// Called after bitmap reading completes to set the correct skip ranges.
    pub fn update_skip_ranges(&self, skip_begin: u64, skip_end: u64) {
        debug_assert!(
            skip_begin + skip_end <= self.cluster_count,
            "Skip ranges exceed cluster count: {} + {} > {}",
            skip_begin,
            skip_end,
            self.cluster_count
        );
        self.skip_begin.store(skip_begin, Ordering::Release);
        self.skip_end.store(skip_end, Ordering::Release);
    }

    /// Returns the byte offset on disk for this chunk.
    #[must_use]
    pub fn disk_offset(&self, cluster_size: u32) -> u64 {
        let effective_lcn = self.effective_lcn();
        if effective_lcn < 0 {
            0 // Sparse extent
        } else {
            effective_lcn as u64 * u64::from(cluster_size)
        }
    }

    /// Returns the byte size to read (after accounting for skips).
    #[must_use]
    pub fn read_size(&self, cluster_size: u32) -> u64 {
        self.effective_cluster_count() * u64::from(cluster_size)
    }

    /// Returns the virtual byte offset in the MFT file.
    #[must_use]
    pub fn virtual_offset(&self, cluster_size: u32) -> u64 {
        self.effective_vcn() * u64::from(cluster_size)
    }

    /// Returns the first FRS in this chunk (after skipping).
    #[must_use]
    pub fn start_frs(&self, cluster_size: u32, record_size: u32) -> u64 {
        self.virtual_offset(cluster_size) / u64::from(record_size)
    }

    /// Returns the number of records in this chunk (after skipping).
    #[must_use]
    pub fn record_count(&self, cluster_size: u32, record_size: u32) -> u64 {
        self.read_size(cluster_size) / u64::from(record_size)
    }

    /// Returns the first FRS in this chunk (before skipping) - for skip range calculation.
    #[must_use]
    pub fn start_frs_raw(&self, cluster_size: u32, record_size: u32) -> u64 {
        (self.vcn * u64::from(cluster_size)) / u64::from(record_size)
    }

    /// Returns the total number of records in this chunk (before skipping).
    #[must_use]
    pub fn record_count_raw(&self, cluster_size: u32, record_size: u32) -> u64 {
        (self.cluster_count * u64::from(cluster_size)) / u64::from(record_size)
    }
}

// ============================================================================
// CppIoPipeline - C++ style I/O orchestrator
// ============================================================================

/// C++ style I/O pipeline that reads bitmap first, then computes skip ranges
/// for all data chunks before starting data I/O.
///
/// This is the key difference from the current Rust implementation:
/// - Current Rust: computes skip ranges during chunk generation
/// - C++ (and this): computes skip ranges AFTER bitmap is fully read
pub struct CppIoPipeline {
    /// Data chunks built from MFT extents
    data_chunks: Vec<Arc<CppDataChunk>>,
    /// Bytes per cluster
    bytes_per_cluster: u32,
    /// Bytes per MFT record
    bytes_per_record: u32,
}

impl CppIoPipeline {
    /// Creates a new pipeline from an MFT extent map.
    ///
    /// This builds `CppDataChunk`s from the extents. Skip ranges are initially 0
    /// and will be computed after the bitmap is read.
    #[must_use]
    pub fn from_extent_map(extent_map: &MftExtentMap) -> Self {
        let mut data_chunks = Vec::with_capacity(extent_map.extents().len());

        for extent in extent_map.extents() {
            // Each extent becomes one CppDataChunk
            // (We could split large extents into smaller chunks here if needed)
            let chunk = Arc::new(CppDataChunk::new(
                extent.vcn,
                extent.cluster_count,
                extent.lcn,
            ));
            data_chunks.push(chunk);
        }

        debug!(
            num_chunks = data_chunks.len(),
            bytes_per_cluster = extent_map.bytes_per_cluster,
            bytes_per_record = extent_map.bytes_per_record,
            "Built CppDataChunks from extent map"
        );

        Self {
            data_chunks,
            bytes_per_cluster: extent_map.bytes_per_cluster,
            bytes_per_record: extent_map.bytes_per_record,
        }
    }

    /// Computes skip ranges for all data chunks using the complete bitmap.
    ///
    /// This is the **synchronization point** - called after the bitmap is fully read.
    /// It updates the atomic skip ranges in each `CppDataChunk`.
    pub fn compute_skip_ranges(&self, bitmap: &MftBitmap) {
        let records_per_cluster = self.bytes_per_cluster / self.bytes_per_record;

        for chunk in &self.data_chunks {
            // Get the raw (unskipped) record range for this chunk
            let start_frs = chunk.start_frs_raw(self.bytes_per_cluster, self.bytes_per_record);
            let record_count = chunk.record_count_raw(self.bytes_per_cluster, self.bytes_per_record);
            let end_frs = start_frs + record_count;

            // Calculate skip ranges in records
            let (skip_records_begin, skip_records_end) = bitmap.calculate_skip_range(start_frs, end_frs);

            // Convert to cluster counts (C++ stores skips in clusters)
            let skip_clusters_begin = skip_records_begin / u64::from(records_per_cluster);
            let skip_clusters_end = skip_records_end / u64::from(records_per_cluster);

            // Update atomically
            chunk.update_skip_ranges(skip_clusters_begin, skip_clusters_end);

            if skip_clusters_begin > 0 || skip_clusters_end > 0 {
                trace!(
                    vcn = chunk.vcn,
                    cluster_count = chunk.cluster_count,
                    skip_clusters_begin,
                    skip_clusters_end,
                    "Updated skip ranges for chunk"
                );
            }
        }

        // Log summary
        let total_clusters: u64 = self.data_chunks.iter().map(|c| c.cluster_count).sum();
        let effective_clusters: u64 = self.data_chunks.iter().map(|c| c.effective_cluster_count()).sum();
        let skipped_clusters = total_clusters - effective_clusters;

        info!(
            total_clusters,
            effective_clusters,
            skipped_clusters,
            skip_pct = format!("{:.1}%", (skipped_clusters as f64 / total_clusters as f64) * 100.0),
            "Computed skip ranges for all chunks (C++ style sync point)"
        );
    }

    /// Runs the IOCP sliding window I/O loop and parses data using `CppParsePipeline`.
    ///
    /// This is the main I/O loop that:
    /// 1. Queues data reads using the computed skip ranges
    /// 2. Processes completions and feeds data to the parse pipeline
    /// 3. Returns the parsed index
    #[allow(unsafe_code, clippy::too_many_lines)]
    pub fn run(
        self,
        overlapped_handle: HANDLE,
        volume: char,
        concurrency: usize,
        io_chunk_size: usize,
        pipeline: CppParsePipeline,
    ) -> Result<crate::cpp_types::CppMftIndex> {
        // Build I/O operations from data chunks (respecting skip ranges)
        struct IoOp {
            disk_offset: u64,
            virtual_offset: u64,
            size: usize,
        }

        let mut io_ops: VecDeque<IoOp> = VecDeque::new();
        let mut max_io_size = 0usize;

        for chunk in &self.data_chunks {
            let effective_clusters = chunk.effective_cluster_count();
            if effective_clusters == 0 {
                continue; // Entire chunk is skipped
            }

            let disk_offset = chunk.disk_offset(self.bytes_per_cluster);
            let virtual_offset = chunk.virtual_offset(self.bytes_per_cluster);
            let total_bytes = chunk.read_size(self.bytes_per_cluster) as usize;

            // Split into smaller I/O operations if needed
            let mut offset = 0usize;
            while offset < total_bytes {
                let remaining = total_bytes - offset;
                let io_size = remaining.min(io_chunk_size);
                max_io_size = max_io_size.max(io_size);

                io_ops.push_back(IoOp {
                    disk_offset: disk_offset + offset as u64,
                    virtual_offset: virtual_offset + offset as u64,
                    size: io_size,
                });
                offset += io_size;
            }
        }

        let total_io_ops = io_ops.len();
        let total_bytes_to_read: u64 = io_ops.iter().map(|op| op.size as u64).sum();

        info!(
            io_ops = total_io_ops,
            bytes_to_read_mb = total_bytes_to_read / (1024 * 1024),
            max_io_size_kb = max_io_size / 1024,
            concurrency,
            "Starting C++ I/O pipeline data reads"
        );

        // Create IOCP
        let read_start = std::time::Instant::now();
        let iocp = IoCompletionPort::new(0)?;
        iocp.associate(overlapped_handle, 0)?;

        // Sliding window state
        struct InFlightOp {
            overlapped: windows::Win32::System::IO::OVERLAPPED,
            buffer: AlignedBuffer,
            op: IoOp,
        }

        // Allocate buffers
        let mut buffer_pool: Vec<AlignedBuffer> = (0..concurrency)
            .map(|_| AlignedBuffer::new(max_io_size))
            .collect();

        let mut in_flight: Vec<Option<Pin<Box<InFlightOp>>>> =
            (0..concurrency).map(|_| None).collect();

        let mut completed_count = 0usize;
        let mut bytes_read_total = 0u64;

        // Queue initial reads
        for slot_id in 0..concurrency {
            if let Some(op) = io_ops.pop_front() {
                let buffer = buffer_pool.pop().unwrap();
                let mut in_flight_op = Box::pin(InFlightOp {
                    overlapped: unsafe { std::mem::zeroed() },
                    buffer,
                    op,
                });

                let offset = in_flight_op.op.disk_offset;
                let op_mut = unsafe { in_flight_op.as_mut().get_unchecked_mut() };
                op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                let overlapped_ptr = &mut op_mut.overlapped as *mut _;
                let read_size = op_mut.op.size;
                let result = unsafe {
                    ReadFile(
                        overlapped_handle,
                        Some(&mut op_mut.buffer.as_mut_slice()[..read_size]),
                        None,
                        Some(overlapped_ptr),
                    )
                };

                match result {
                    Ok(()) => {}
                    Err(_) => {
                        let last_error = unsafe { GetLastError() };
                        if last_error != ERROR_IO_PENDING {
                            return Err(crate::error::MftError::Io(std::io::Error::from_raw_os_error(
                                last_error.0 as i32,
                            )));
                        }
                    }
                }

                in_flight[slot_id] = Some(in_flight_op);
            }
        }

        // Process completions
        while completed_count < total_io_ops {
            let mut bytes_transferred: u32 = 0;
            let mut completion_key: usize = 0;
            let mut overlapped_ptr: *mut windows::Win32::System::IO::OVERLAPPED =
                std::ptr::null_mut();

            let result = unsafe {
                GetQueuedCompletionStatus(
                    iocp.raw_handle(),
                    &mut bytes_transferred,
                    &mut completion_key,
                    &mut overlapped_ptr,
                    u32::MAX,
                )
            };

            if result.is_err() {
                let err = std::io::Error::last_os_error();
                warn!(error = %err, "GetQueuedCompletionStatus failed");
                continue;
            }

            // Find completed slot
            let mut completed_slot = None;
            for (idx, slot) in in_flight.iter().enumerate() {
                if let Some(op) = slot {
                    let op_overlapped_ptr =
                        &op.overlapped as *const _ as *mut windows::Win32::System::IO::OVERLAPPED;
                    if op_overlapped_ptr == overlapped_ptr {
                        completed_slot = Some(idx);
                        break;
                    }
                }
            }

            if let Some(slot_idx) = completed_slot {
                if let Some(mut completed_op) = in_flight[slot_idx].take() {
                    let op_mut = unsafe { completed_op.as_mut().get_unchecked_mut() };

                    // Process chunk using C++ two-phase pipeline
                    let buffer_slice =
                        &mut op_mut.buffer.as_mut_slice()[..bytes_transferred as usize];
                    let virtual_offset = op_mut.op.virtual_offset;

                    pipeline.process_chunk(buffer_slice, virtual_offset);

                    bytes_read_total += bytes_transferred as u64;
                    completed_count += 1;

                    // Recycle buffer and queue next read
                    let recycled_buffer = std::mem::replace(
                        &mut op_mut.buffer,
                        AlignedBuffer::new(0),
                    );
                    buffer_pool.push(recycled_buffer);

                    // Queue next read if available
                    if let Some(next_op) = io_ops.pop_front() {
                        let buffer = buffer_pool.pop().unwrap();
                        let mut new_in_flight = Box::pin(InFlightOp {
                            overlapped: unsafe { std::mem::zeroed() },
                            buffer,
                            op: next_op,
                        });

                        let offset = new_in_flight.op.disk_offset;
                        let new_op_mut = unsafe { new_in_flight.as_mut().get_unchecked_mut() };
                        new_op_mut.overlapped.Anonymous.Anonymous.Offset = offset as u32;
                        new_op_mut.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

                        let overlapped_ptr = &mut new_op_mut.overlapped as *mut _;
                        let read_size = new_op_mut.op.size;
                        let result = unsafe {
                            ReadFile(
                                overlapped_handle,
                                Some(&mut new_op_mut.buffer.as_mut_slice()[..read_size]),
                                None,
                                Some(overlapped_ptr),
                            )
                        };

                        match result {
                            Ok(()) => {}
                            Err(_) => {
                                let last_error = unsafe { GetLastError() };
                                if last_error != ERROR_IO_PENDING {
                                    return Err(crate::error::MftError::Io(
                                        std::io::Error::from_raw_os_error(last_error.0 as i32),
                                    ));
                                }
                            }
                        }

                        in_flight[slot_idx] = Some(new_in_flight);
                    }
                }
            }
        }

        let read_elapsed = read_start.elapsed();
        let read_ms = read_elapsed.as_millis();
        let throughput_mbps = if read_ms > 0 {
            (bytes_read_total as u128 * 1000) / (read_ms * 1024 * 1024)
        } else {
            0
        };

        info!(
            read_ms,
            throughput_mbps,
            bytes_read_mb = bytes_read_total / (1024 * 1024),
            "C++ I/O pipeline data reads complete"
        );

        // Return the parsed index
        Ok(pipeline.into_index())
    }
}

