//! Benchmarking helper types and small construction utilities.

#[cfg(windows)]
use crate::platform::DriveType;

/// Phase timing breakdown for MFT reading operations.
///
/// Each phase is measured independently to identify bottlenecks.
#[derive(Debug, Clone, Default)]
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
    #[expect(
        clippy::cast_possible_wrap,
        reason = "overhead can be negative; u64 values are bounded by total runtime"
    )]
    pub const fn overhead_ms(&self) -> i64 {
        self.total_ms as i64 - self.sum_phases() as i64
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

/// Builds the drive characteristics payload for benchmark output.
#[must_use]
#[cfg(windows)]
pub(super) fn build_drive_characteristics(
    drive_letter: char,
    drive_type: DriveType,
    mft_size_bytes: u64,
    total_records: u64,
    in_use_records: Option<u64>,
    extent_count: usize,
    bytes_per_record: u32,
    chunk_size_bytes: usize,
    chunk_count: usize,
) -> DriveCharacteristics {
    DriveCharacteristics {
        drive_letter,
        drive_type: format!("{drive_type:?}"),
        mft_size_bytes,
        total_records,
        in_use_records,
        extent_count,
        bytes_per_record,
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
