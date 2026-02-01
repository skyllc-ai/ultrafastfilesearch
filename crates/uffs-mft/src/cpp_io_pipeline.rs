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
//! 1. **Phase 1: Bitmap Reading** - Read $MFT::$BITMAP chunks asynchronously
//! 2. **Synchronization Point** - After ALL bitmap chunks complete:
//!    - Recalculate skip_begin/skip_end for ALL data chunks
//!    - Store atomically so data reads use updated values
//! 3. **Phase 2: Data Reading** - Read $MFT::$DATA chunks with correct skip ranges
//!
//! ## Why This Matters
//!
//! The current Rust implementation calculates skip ranges BEFORE reading the
//! bitmap, leading to incorrect skip ranges for some chunks. This causes ~40
//! files to be missed compared to C++.
//!
//! ## C++ Source Reference
//!
//! - `mft_reader.hpp` lines 40-63: `RetPtr` struct with atomic skip ranges
//! - `mft_reader.hpp` lines 245-296: Synchronization point after bitmap completes
//! - `mft_reader.hpp` lines 321-386: `queue_next()` - bitmap first, then data

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{debug, info, trace};

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
}

