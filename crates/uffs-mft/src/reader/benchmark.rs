// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Benchmarking helper types and small construction utilities.

#[cfg(windows)]
use crate::platform::DriveType;

/// Phase timing breakdown for MFT reading operations.
///
/// Each phase is measured independently to identify bottlenecks.
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_field_names,
    reason = "_ms suffix documents the unit — removing it loses critical information"
)]
pub struct PhaseTimings {
    /// Time to open volume and retrieve MFT metadata.
    pub open_ms: u64,
    /// Time spent reading chunks from disk (I/O).
    pub read_ms: u64,
    /// Time spent parsing MFT records (CPU, parallel).
    pub parse_ms: u64,
    /// Time spent merging extension records.
    pub merge_ms: u64,
    /// Time spent building the `DataFrame` from parsed records.
    pub df_build_ms: u64,
    /// Time spent building the lean `MftIndex`.
    pub index_build_ms: u64,
    /// Time spent computing tree metrics.
    pub tree_metrics_ms: u64,
    /// Total wall-clock time.
    pub total_ms: u64,
}

impl PhaseTimings {
    /// Returns the sum of individual phases (may differ from total due to
    /// overlap).
    #[must_use]
    pub const fn sum_phases(&self) -> u64 {
        self.open_ms
            + self.read_ms
            + self.parse_ms
            + self.merge_ms
            + self.df_build_ms
            + self.index_build_ms
            + self.tree_metrics_ms
    }

    /// Returns the overhead (total - sum of phases).
    #[must_use]
    pub fn overhead_ms(&self) -> i64 {
        let total = i64::try_from(self.total_ms).unwrap_or(i64::MAX);
        let phases = i64::try_from(self.sum_phases()).unwrap_or(i64::MAX);
        total.saturating_sub(phases)
    }
}

/// Drive and MFT characteristics for benchmarking.
#[derive(Debug, Clone)]
pub struct DriveCharacteristics {
    /// Drive letter (e.g., 'C').
    pub drive_letter: char,
    /// Detected drive type (SSD, HDD, Unknown).
    pub drive_type: String,
    /// Total MFT size in bytes.
    pub mft_size_bytes: u64,
    /// Total number of MFT records.
    pub total_records: u64,
    /// Number of in-use records (if bitmap available).
    pub in_use_records: Option<u64>,
    /// Number of MFT extents.
    pub extent_count: usize,
    /// Bytes per MFT record.
    pub bytes_per_record: u32,
    /// Chunk size used for I/O (bytes).
    pub chunk_size_bytes: usize,
    /// Number of read chunks generated.
    pub chunk_count: usize,
}

/// Complete benchmark result including timings and characteristics.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Phase timing breakdown.
    pub timings: PhaseTimings,
    /// Drive and MFT characteristics.
    pub characteristics: DriveCharacteristics,
    /// Number of records successfully parsed.
    pub records_parsed: usize,
    /// Throughput in MB/s.
    pub throughput_mb_s: f64,
    /// Records processed per second.
    pub records_per_sec: f64,
}

impl BenchmarkResult {
    /// Formats the result as JSON for scripting.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            r#"{{
  "drive": "{}",
  "drive_type": "{}",
  "mft_size_bytes": {},
  "total_records": {},
  "in_use_records": {},
  "extent_count": {},
  "bytes_per_record": {},
  "chunk_size_bytes": {},
  "chunk_count": {},
  "records_parsed": {},
  "timings_ms": {{
    "open": {},
    "read": {},
    "parse": {},
    "merge": {},
    "df_build": {},
    "index_build": {},
    "tree_metrics": {},
    "total": {}
  }},
  "throughput": {{
    "mb_per_sec": {:.2},
    "records_per_sec": {:.0}
  }}
}}"#,
            self.characteristics.drive_letter,
            self.characteristics.drive_type,
            self.characteristics.mft_size_bytes,
            self.characteristics.total_records,
            self.characteristics
                .in_use_records
                .map_or_else(|| "null".to_owned(), |val| val.to_string()),
            self.characteristics.extent_count,
            self.characteristics.bytes_per_record,
            self.characteristics.chunk_size_bytes,
            self.characteristics.chunk_count,
            self.records_parsed,
            self.timings.open_ms,
            self.timings.read_ms,
            self.timings.parse_ms,
            self.timings.merge_ms,
            self.timings.df_build_ms,
            self.timings.index_build_ms,
            self.timings.tree_metrics_ms,
            self.timings.total_ms,
            self.throughput_mb_s,
            self.records_per_sec,
        )
    }
}

/// Per-MFT metrics passed to [`build_drive_characteristics`].
///
/// Bundling the five MFT-shape numbers into a struct keeps
/// `build_drive_characteristics` under the 7-argument cap without splitting it
/// into multiple helpers (the function itself is a trivial constructor and
/// further extraction would obscure intent).
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub(super) struct MftMetrics {
    /// Logical MFT size in bytes (`total_records * bytes_per_record`).
    pub size_bytes: u64,
    /// Total record count reported by the MFT extent map.
    pub total_records: u64,
    /// In-use record count from the bitmap, when available.
    pub in_use_records: Option<u64>,
    /// Number of extents in the MFT $DATA attribute.
    pub extent_count: usize,
    /// Bytes per MFT record (typically 1024).
    pub bytes_per_record: u32,
}

/// Builds the drive characteristics payload for benchmark output.
#[must_use]
#[cfg(windows)]
pub(super) fn build_drive_characteristics(
    drive_letter: char,
    drive_type: DriveType,
    mft: MftMetrics,
    chunk_size_bytes: usize,
    chunk_count: usize,
) -> DriveCharacteristics {
    DriveCharacteristics {
        drive_letter,
        drive_type: format!("{drive_type:?}"),
        mft_size_bytes: mft.size_bytes,
        total_records: mft.total_records,
        in_use_records: mft.in_use_records,
        extent_count: mft.extent_count,
        bytes_per_record: mft.bytes_per_record,
        chunk_size_bytes,
        chunk_count,
    }
}

/// Builds a benchmark result from the measured fields.
#[must_use]
#[cfg(windows)]
pub(super) const fn build_benchmark_result(
    timings: PhaseTimings,
    characteristics: DriveCharacteristics,
    records_parsed: usize,
    throughput_mb_s: f64,
    records_per_sec: f64,
) -> BenchmarkResult {
    BenchmarkResult {
        timings,
        characteristics,
        records_parsed,
        throughput_mb_s,
        records_per_sec,
    }
}

/// Estimates read/parse/merge timing split when only combined timing is
/// available.
#[must_use]
#[cfg(windows)]
pub(super) const fn estimate_combined_phase_timings(
    drive_type: DriveType,
    read_parse_ms: u64,
) -> (u64, u64, u64) {
    match drive_type {
        DriveType::Nvme => {
            let read_est = read_parse_ms * 20 / 100;
            let parse_est = read_parse_ms * 60 / 100;
            let merge_est = read_parse_ms * 20 / 100;
            (read_est, parse_est, merge_est)
        }
        DriveType::Ssd => {
            let read_est = read_parse_ms * 30 / 100;
            let parse_est = read_parse_ms * 50 / 100;
            let merge_est = read_parse_ms * 20 / 100;
            (read_est, parse_est, merge_est)
        }
        DriveType::Hdd | DriveType::Unknown => {
            let read_est = read_parse_ms * 70 / 100;
            let parse_est = read_parse_ms * 20 / 100;
            let merge_est = read_parse_ms * 10 / 100;
            (read_est, parse_est, merge_est)
        }
    }
}
