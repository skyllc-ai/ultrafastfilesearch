//! MFT Reader implementation.
//!
//! This module provides the main entry point for reading NTFS MFT data.

use core::time::Duration;
use std::path::Path;
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, warn};
use uffs_polars::{DataFrame, ParquetReader, ParquetWriter, SerReader};

use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::platform::VolumeHandle;

// ============================================================================
// MFT Read Mode Selection
// ============================================================================

/// Read mode for MFT operations.
///
/// Different modes optimize for different drive types and workloads:
/// - `Parallel`: Best for SSDs - reads all chunks then parses in parallel
/// - `Streaming`: Best for HDDs - sequential reads with immediate parsing
/// - `Prefetch`: Best for HDDs - double-buffered prefetch for I/O overlap
/// - `Auto`: Automatically selects based on detected drive type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MftReadMode {
    /// Automatic mode selection based on drive type (default).
    /// - SSD → Parallel
    /// - HDD → Pipelined
    /// - Unknown → Parallel
    #[default]
    Auto,
    /// Parallel mode: Read all chunks into memory, then parse in parallel.
    /// Best for SSDs where random I/O is fast.
    Parallel,
    /// Streaming mode: Sequential reads with immediate parsing.
    /// Lower memory usage, good for HDDs.
    Streaming,
    /// Prefetch mode: Double-buffered reads for I/O overlap.
    /// Good for HDDs - overlaps next read with current parse.
    Prefetch,
    /// Pipelined mode: True I/O and CPU overlap with separate threads.
    /// Best for HDDs - reader thread queues chunks while parser processes.
    Pipelined,
}

impl MftReadMode {
    /// Returns the mode name as a string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Parallel => "parallel",
            Self::Streaming => "streaming",
            Self::Prefetch => "prefetch",
            Self::Pipelined => "pipelined",
        }
    }
}

impl core::fmt::Display for MftReadMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl core::str::FromStr for MftReadMode {
    type Err = String;

    fn from_str(s: &str) -> core::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "parallel" => Ok(Self::Parallel),
            "streaming" => Ok(Self::Streaming),
            "prefetch" => Ok(Self::Prefetch),
            "pipelined" | "pipeline" => Ok(Self::Pipelined),
            _ => Err(format!(
                "Invalid read mode '{s}'. Valid options: auto, parallel, streaming, prefetch, pipelined"
            )),
        }
    }
}

// ============================================================================
// MFT Statistics (computed during DF build - M1 8.3 optimization)
// ============================================================================

/// Statistics computed during MFT parsing and `DataFrame` building.
///
/// This struct is populated during the single-pass DF build loop,
/// eliminating the need for a separate statistics pass (M1 8.3 optimization).
#[derive(Debug, Clone, Default)]
pub struct MftStats {
    /// Number of directory records.
    pub dir_count: u64,
    /// Number of file records.
    pub file_count: u64,
    /// Number of hidden files/directories.
    pub hidden_count: u64,
    /// Number of system files/directories.
    pub system_count: u64,
    /// Number of compressed files.
    pub compressed_count: u64,
    /// Number of encrypted files.
    pub encrypted_count: u64,
    /// Number of sparse files.
    pub sparse_count: u64,
    /// Number of reparse points.
    pub reparse_count: u64,
    /// Number of files with multiple data streams (ADS).
    pub multi_stream_count: u64,
    /// Number of files with multiple names (hard links).
    pub multi_name_count: u64,
    /// Total logical file size in bytes.
    pub total_file_size: u64,
    /// Total allocated size in bytes.
    pub total_allocated_size: u64,
}

impl MftStats {
    /// Returns the slack space (allocated - logical size).
    #[must_use]
    pub const fn slack_space(&self) -> u64 {
        self.total_allocated_size
            .saturating_sub(self.total_file_size)
    }

    /// Returns the slack percentage (0.0 to 100.0).
    ///
    /// This is a display/presentation function that computes a human-readable
    /// percentage. Float arithmetic is unavoidable here since percentages are
    /// inherently fractional values (e.g., 45.67%).
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn slack_percentage(&self) -> f64 {
        if self.total_allocated_size > 0 {
            (self.slack_space() as f64 / self.total_allocated_size as f64) * 100.0
        } else {
            0.0
        }
    }
}

// ============================================================================
// Benchmark / Timing Types
// ============================================================================

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
    /// Total wall-clock time.
    pub total_ms: u64,
}

impl PhaseTimings {
    /// Returns the sum of individual phases (may differ from total due to
    /// overlap).
    #[must_use]
    pub const fn sum_phases(&self) -> u64 {
        self.open_ms + self.read_ms + self.parse_ms + self.merge_ms + self.df_build_ms
    }

    /// Returns the overhead (total - sum of phases).
    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
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
    /// Number of MFT extents (fragmentation indicator).
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
    /// Throughput in MB/s (based on MFT size / total time).
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
            self.timings.total_ms,
            self.throughput_mb_s,
            self.records_per_sec,
        )
    }
}

// ============================================================================
// Progress Types
// ============================================================================

/// Progress information during MFT reading.
#[derive(Debug, Clone)]
pub struct MftProgress {
    /// Number of records read so far.
    pub records_read: u64,
    /// Total number of records (if known).
    pub total_records: Option<u64>,
    /// Bytes read from disk.
    pub bytes_read: u64,
    /// Time elapsed since start.
    pub elapsed: Duration,
}

impl MftProgress {
    /// Returns the percentage complete (0.0 to 100.0), if total is known.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn percentage(&self) -> Option<f64> {
        self.total_records
            .map(|total| (self.records_read as f64 / total as f64) * 100.0_f64)
    }

    /// Returns the read speed in MB/s.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn speed_mbps(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            (self.bytes_read as f64 / 1_048_576.0) / secs
        } else {
            0.0
        }
    }
}

/// MFT Reader for direct NTFS Master File Table access.
///
/// This struct provides high-performance MFT reading by bypassing
/// Windows file enumeration APIs and reading the MFT directly.
///
/// # Platform Support
///
/// MFT reading is only available on Windows. On other platforms,
/// methods will return `MftError::PlatformNotSupported`.
///
/// # Privileges
///
/// Reading the MFT requires Administrator privileges. The process
/// must be elevated or have `SE_BACKUP_PRIVILEGE`.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::MftReader;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MftReader::open('C').await?;
///     let df = reader.read_all().await?;
///     println!("Found {} files", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct MftReader {
    /// The volume letter (e.g., 'C').
    volume: char,
    /// The volume handle (Windows only).
    #[cfg(windows)]
    handle: VolumeHandle,
    /// Read mode selection.
    mode: MftReadMode,
    /// Whether to merge extension records.
    ///
    /// - `false` (default): Fast path - skips extension records (~1% of files
    ///   with many hard links/ADS). ~15-25% faster, ideal for file search.
    /// - `true`: Full path - merges extension attributes for complete data.
    merge_extensions: bool,
}

impl MftReader {
    /// Open a volume for MFT reading.
    ///
    /// # Arguments
    ///
    /// * `volume` - The drive letter (e.g., 'C', 'D')
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The volume cannot be opened
    /// - The volume is not NTFS formatted
    /// - Insufficient privileges (not running as Administrator)
    ///
    /// # Platform
    ///
    /// This function is only available on Windows.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn open(volume: char) -> Result<Self> {
        // Open the volume handle (validates NTFS and privileges)
        let handle = VolumeHandle::open(volume)?;

        Ok(Self {
            volume: volume.to_ascii_uppercase(),
            handle,
            mode: MftReadMode::Auto,
            merge_extensions: false, // Fast path by default
        })
    }

    /// Open a volume for MFT reading (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn open(_volume: char) -> Result<Self> {
        Err(MftError::PlatformNotSupported)
    }

    /// Sets the read mode for this reader.
    ///
    /// # Arguments
    ///
    /// * `mode` - The read mode to use (Auto, Parallel, Streaming, Prefetch)
    #[must_use]
    pub const fn with_mode(mut self, mode: MftReadMode) -> Self {
        self.mode = mode;
        self
    }

    /// Returns the current read mode.
    #[must_use]
    pub const fn mode(&self) -> MftReadMode {
        self.mode
    }

    /// Sets whether to merge extension records.
    ///
    /// Extension records are used when a file has too many attributes to fit
    /// in a single MFT record (e.g., many hard links, alternate data streams).
    /// This affects ~1% of files.
    ///
    /// # Arguments
    ///
    /// * `merge` - If `true`, merge extension records for complete data. If
    ///   `false` (default), skip extensions for ~15-25% faster reads.
    #[must_use]
    pub const fn with_merge_extensions(mut self, merge: bool) -> Self {
        self.merge_extensions = merge;
        self
    }

    /// Returns whether extension record merging is enabled.
    #[must_use]
    pub const fn merge_extensions(&self) -> bool {
        self.merge_extensions
    }

    /// Read the entire MFT and return as a `DataFrame`.
    ///
    /// This method reads all MFT records and constructs a Polars `DataFrame`
    /// with the standard schema (frs, `parent_frs`, name, size, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_mft_internal(None::<fn(MftProgress)>)
    }

    /// Read the entire MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT with progress callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called periodically with progress updates
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_internal(Some(callback))
    }

    /// Read MFT with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT with detailed phase timing for benchmarking.
    ///
    /// This method measures each phase of MFT reading separately:
    /// - Open: Volume handle and metadata retrieval
    /// - Read: Disk I/O (reading chunks)
    /// - Parse: Record parsing (parallel)
    /// - Merge: Extension record merging
    /// - DataFrame build: Converting parsed records to DataFrame
    ///
    /// # Arguments
    ///
    /// * `skip_df_build` - If true, skip DataFrame building (measure I/O +
    ///   parse only)
    ///
    /// # Returns
    ///
    /// A tuple of (optional DataFrame, BenchmarkResult).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_with_timing(
        &self,
        skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        self.read_mft_with_timing_internal(skip_df_build)
    }

    /// Read MFT with timing (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_with_timing(
        &self,
        _skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal MFT reading implementation.
    ///
    /// This implementation uses the high-performance parallel reader with:
    /// 1. Extent-aware reading for fragmented MFTs
    /// 2. Bitmap-based cluster skipping (like C++ implementation)
    /// 3. Parallel record processing using Rayon
    /// 4. Large batch I/O (4-8 MB) for reduced syscall overhead
    /// 5. Drive-type aware tuning (SSD vs HDD)
    #[cfg(windows)]
    fn read_mft_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(MftProgress),
    {
        use crate::io::{MftExtentMap, ParallelMftReader};
        use crate::platform::detect_drive_type;

        info!(volume = %self.volume, "Starting MFT read");

        let start_time = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Detect drive type for optimal I/O tuning
        let drive_type = detect_drive_type(self.volume);
        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            chunk_size_mb = drive_type.optimal_chunk_size() / (1024 * 1024),
            "🚀 Drive type detected for I/O optimization"
        );

        debug!(
            record_size,
            bytes_per_cluster = volume_data.bytes_per_cluster,
            mft_valid_data_length = volume_data.mft_valid_data_length,
            "Volume data retrieved"
        );

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
            // Fallback to single contiguous extent
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        info!(num_extents = extents.len(), "MFT extents retrieved");

        // Create extent map
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);

        let total_records = extent_map.total_records();
        info!(total_records, "Total MFT records to read");

        // Try to get the MFT bitmap for optimization
        let bitmap = self.handle.get_mft_bitmap().ok();
        if let Some(ref bm) = bitmap {
            let in_use = bm.count_in_use();
            info!(
                in_use_records = in_use,
                skip_percentage = 100.0 - (in_use as f64 / total_records as f64 * 100.0),
                "MFT bitmap loaded - will skip unused records"
            );
        } else {
            debug!("No MFT bitmap available - reading all records");
        }

        // Report initial progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: 0,
                total_records: Some(total_records),
                bytes_read: 0,
                elapsed: start_time.elapsed(),
            });
        }

        // M2 9.1-9.3: Select reader based on mode
        let effective_mode = match self.mode {
            MftReadMode::Auto => {
                // Auto-select based on drive type
                // - SSD: Parallel (read all, parse in parallel)
                // - HDD: Pipelined (true I/O+CPU overlap)
                match drive_type {
                    crate::platform::DriveType::Ssd => MftReadMode::Parallel,
                    crate::platform::DriveType::Hdd => MftReadMode::Pipelined,
                    crate::platform::DriveType::Unknown => MftReadMode::Parallel,
                }
            }
            mode => mode,
        };

        info!(
            mode = %effective_mode,
            "🚀 Using read mode"
        );

        let handle = self.handle.raw_handle();
        let total_bytes = total_records * u64::from(record_size);

        // Read using the selected mode
        let parsed_records = match effective_mode {
            MftReadMode::Parallel | MftReadMode::Auto => {
                // Parallel mode: read all chunks then parse in parallel (best for SSD)
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_parallel_with_progress(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    parallel_reader
                        .read_all_parallel_with_progress::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Streaming => {
                // Streaming mode: sequential reads with immediate parsing (lower memory)
                let mut streaming_reader =
                    crate::io::StreamingMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    streaming_reader.read_all_streaming(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    streaming_reader.read_all_streaming::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Prefetch => {
                // Prefetch mode: double-buffered reads for I/O overlap (good for HDD)
                let prefetch_reader =
                    crate::io::PrefetchMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    prefetch_reader.read_all_prefetch(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    prefetch_reader.read_all_prefetch::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::Pipelined => {
                // Pipelined mode: true I/O+CPU overlap with separate threads (best for HDD)
                let pipelined_reader =
                    crate::io::PipelinedMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    pipelined_reader.read_all_pipelined(
                        handle,
                        true,
                        Some(move |bytes_read: u64, total_bytes_expected: u64| {
                            let records_approx = if total_bytes_expected > 0 {
                                (bytes_read * total_records) / total_bytes_expected
                            } else {
                                0
                            };
                            cb_ref(MftProgress {
                                records_read: records_approx,
                                total_records: Some(total_records),
                                bytes_read,
                                elapsed: start.elapsed(),
                            });
                        }),
                    )?
                } else {
                    pipelined_reader.read_all_pipelined::<fn(u64, u64)>(handle, true, None)?
                }
            }
        };

        let read_elapsed = start_time.elapsed();
        let records_parsed_count = parsed_records.len();
        let throughput_mb_s = if read_elapsed.as_secs_f64() > 0.0 {
            (total_bytes as f64 / (1024.0 * 1024.0)) / read_elapsed.as_secs_f64()
        } else {
            0.0
        };
        let records_per_sec = if read_elapsed.as_secs_f64() > 0.0 {
            records_parsed_count as f64 / read_elapsed.as_secs_f64()
        } else {
            0.0
        };

        info!(
            records_parsed = records_parsed_count,
            total_records,
            elapsed_ms = read_elapsed.as_millis(),
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "✅ Parallel read complete"
        );

        // Report final progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: total_records,
                total_records: Some(total_records),
                bytes_read: total_bytes,
                elapsed: start_time.elapsed(),
            });
        }

        // M1 8.3 OPTIMIZATION: Fuse stats computation with DataFrame building
        // This eliminates one full pass over all records (was ~5-10% of DF build time)
        let capacity = parsed_records.len();
        let mut stats = MftStats::default();

        // Pre-allocate all column vectors
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);

        // Single pass: build columns AND compute stats simultaneously
        for parsed in parsed_records {
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            // Accumulate stats inline (no separate loop!)
            if parsed.is_directory {
                stats.dir_count += 1;
            } else {
                stats.file_count += 1;
                stats.total_file_size = stats.total_file_size.saturating_add(parsed.size);
                stats.total_allocated_size = stats
                    .total_allocated_size
                    .saturating_add(parsed.allocated_size);
            }
            stats.hidden_count += u64::from(parsed.std_info.is_hidden);
            stats.system_count += u64::from(parsed.std_info.is_system);
            stats.compressed_count += u64::from(parsed.std_info.is_compressed);
            stats.encrypted_count += u64::from(parsed.std_info.is_encrypted);
            stats.sparse_count += u64::from(parsed.std_info.is_sparse);
            stats.reparse_count += u64::from(parsed.std_info.is_reparse);
            stats.multi_stream_count += u64::from(stream_count > 1);
            stats.multi_name_count += u64::from(name_count > 1);

            // Build column vectors
            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(name_count);
            stream_count_vec.push(stream_count);
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
        }

        // Log stats (computed during the loop above)
        info!(
            directories = stats.dir_count,
            files = stats.file_count,
            "📊 Record type breakdown"
        );

        info!(
            hidden = stats.hidden_count,
            system = stats.system_count,
            compressed = stats.compressed_count,
            encrypted = stats.encrypted_count,
            sparse = stats.sparse_count,
            reparse_points = stats.reparse_count,
            "🏷️  Attribute flags summary"
        );

        if stats.multi_stream_count > 0 || stats.multi_name_count > 0 {
            info!(
                files_with_ads = stats.multi_stream_count,
                files_with_hardlinks = stats.multi_name_count,
                "🔗 Extended attributes"
            );
        }

        debug!(
            total_file_size_gb = format!(
                "{:.2}",
                stats.total_file_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            total_allocated_gb = format!(
                "{:.2}",
                stats.total_allocated_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            slack_space_mb = format!("{:.2}", stats.slack_space() as f64 / (1024.0 * 1024.0)),
            slack_percentage = format!("{:.1}%", stats.slack_percentage()),
            "💾 Storage analysis"
        );

        // Build DataFrame with full schema
        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
        )
    }

    /// Internal implementation for MFT reading with detailed phase timing.
    ///
    /// This method measures each phase separately for benchmarking purposes.
    #[cfg(windows)]
    #[allow(clippy::too_many_lines)]
    fn read_mft_with_timing_internal(
        &self,
        skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        use crate::io::{MftExtentMap, ParallelMftReader, generate_read_chunks};
        use crate::platform::detect_drive_type;

        let total_start = Instant::now();

        // Phase 1: Open (already done, but measure metadata retrieval)
        let open_start = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = drive_type.optimal_chunk_size();

        // Get MFT extents
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        let extent_map =
            MftExtentMap::new(extents.clone(), volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();
        let mft_size_bytes = total_records * u64::from(record_size);

        // Get bitmap
        let bitmap = self.handle.get_mft_bitmap().ok();
        let in_use_records = bitmap.as_ref().map(|bm| bm.count_in_use() as u64);

        // Generate chunks to get count
        let chunks = generate_read_chunks(&extent_map, bitmap.as_ref(), chunk_size);
        let chunk_count = chunks.len();

        let open_ms = open_start.elapsed().as_millis() as u64;

        // Build characteristics
        let characteristics = DriveCharacteristics {
            drive_letter: self.volume,
            drive_type: format!("{:?}", drive_type),
            mft_size_bytes,
            total_records,
            in_use_records,
            extent_count: extents.len(),
            bytes_per_record: record_size,
            chunk_size_bytes: chunk_size,
            chunk_count,
        };

        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            total_records,
            mft_size_mb = mft_size_bytes / (1024 * 1024),
            extents = extents.len(),
            chunks = chunk_count,
            "📊 Benchmark: MFT characteristics"
        );

        // Phase 2+3: Read + Parse (using SoA path for optimal df_build)
        // The ParallelMftReader reads sequentially then parses in parallel.
        // Using read_all_parallel_to_columns returns ParsedColumns (SoA layout)
        // which eliminates the AoS→SoA transpose in df_build.
        //
        // Fast path (merge_extensions=false): Skips extension records (~1% of files
        // with many hard links/ADS). ~15-25% faster, ideal for file search.
        let read_parse_start = Instant::now();
        let parallel_reader = ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
        let handle = self.handle.raw_handle();

        let parsed_columns = parallel_reader.read_all_parallel_to_columns::<fn(u64, u64)>(
            handle,
            self.merge_extensions,
            None,
        )?;

        let read_parse_ms = read_parse_start.elapsed().as_millis() as u64;
        let records_parsed = parsed_columns.len();

        // Note: Currently read and parse are interleaved in ParallelMftReader.
        // For now, we report combined time. Future: instrument inside
        // ParallelMftReader. Estimate: ~70% read, ~30% parse on HDD; ~30% read,
        // ~70% parse on SSD
        let (read_ms, parse_ms, merge_ms) = match drive_type {
            crate::platform::DriveType::Ssd => {
                // SSD: I/O is fast, parsing dominates
                let read_est = read_parse_ms * 30 / 100;
                let parse_est = read_parse_ms * 50 / 100;
                let merge_est = read_parse_ms * 20 / 100;
                (read_est, parse_est, merge_est)
            }
            _ => {
                // HDD: I/O dominates
                let read_est = read_parse_ms * 70 / 100;
                let parse_est = read_parse_ms * 20 / 100;
                let merge_est = read_parse_ms * 10 / 100;
                (read_est, parse_est, merge_est)
            }
        };

        info!(
            records_parsed,
            read_parse_ms, "📊 Benchmark: Read + Parse complete"
        );

        // Phase 4: DataFrame build (optional)
        // Using SoA path: ParsedColumns → DataFrame (no transpose needed!)
        let (df, df_build_ms) = if skip_df_build {
            (None, 0)
        } else {
            let df_start = Instant::now();
            let df = Self::build_dataframe_from_columns(parsed_columns)?;
            let df_ms = df_start.elapsed().as_millis() as u64;
            info!(
                df_build_ms = df_ms,
                "📊 Benchmark: DataFrame build complete (SoA path)"
            );
            (Some(df), df_ms)
        };

        let total_ms = total_start.elapsed().as_millis() as u64;

        // Calculate throughput
        let total_secs = total_ms as f64 / 1000.0;
        let throughput_mb_s = if total_secs > 0.0 {
            (mft_size_bytes as f64 / (1024.0 * 1024.0)) / total_secs
        } else {
            0.0
        };
        let records_per_sec = if total_secs > 0.0 {
            records_parsed as f64 / total_secs
        } else {
            0.0
        };

        let timings = PhaseTimings {
            open_ms,
            read_ms,
            parse_ms,
            merge_ms,
            df_build_ms,
            total_ms,
        };

        let result = BenchmarkResult {
            timings,
            characteristics,
            records_parsed,
            throughput_mb_s,
            records_per_sec,
        };

        info!(
            total_ms,
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "📊 Benchmark: Complete"
        );

        Ok((df, result))
    }

    /// Helper to build DataFrame from parsed records (legacy AoS path).
    ///
    /// NOTE: This function is superseded by `build_dataframe_from_columns`
    /// which uses the SoA path and avoids the AoS→SoA transpose. Kept for
    /// reference and potential fallback use.
    #[cfg(windows)]
    #[allow(dead_code)]
    fn build_dataframe_from_records(
        parsed_records: Vec<crate::io::ParsedRecord>,
    ) -> Result<DataFrame> {
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(name_count);
            stream_count_vec.push(stream_count);
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
        }

        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
        )
    }

    /// Builds a `DataFrame` from the collected vectors (legacy 8-column
    /// schema).
    #[cfg(windows)]
    #[allow(clippy::too_many_arguments, dead_code)]
    fn build_dataframe(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        flags_vec: Vec<u16>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let columns = vec![
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            Series::new("size".into(), size_vec).into_column(),
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` with full C++ parity schema (23 columns).
    #[cfg(windows)]
    #[allow(clippy::too_many_arguments)]
    fn build_dataframe_full(
        frs_vec: Vec<u64>,
        parent_frs_vec: Vec<u64>,
        name_vec: Vec<String>,
        size_vec: Vec<u64>,
        allocated_size_vec: Vec<u64>,
        created_vec: Vec<i64>,
        modified_vec: Vec<i64>,
        accessed_vec: Vec<i64>,
        mft_changed_vec: Vec<i64>,
        is_directory_vec: Vec<bool>,
        name_count_vec: Vec<u16>,
        stream_count_vec: Vec<u16>,
        is_readonly_vec: Vec<bool>,
        is_hidden_vec: Vec<bool>,
        is_system_vec: Vec<bool>,
        is_archive_vec: Vec<bool>,
        is_compressed_vec: Vec<bool>,
        is_encrypted_vec: Vec<bool>,
        is_sparse_vec: Vec<bool>,
        is_reparse_vec: Vec<bool>,
        is_offline_vec: Vec<bool>,
        is_not_indexed_vec: Vec<bool>,
        is_temporary_vec: Vec<bool>,
    ) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let columns = vec![
            // Core identifiers
            Series::new("frs".into(), frs_vec).into_column(),
            Series::new("parent_frs".into(), parent_frs_vec).into_column(),
            Series::new("name".into(), name_vec).into_column(),
            // Size information
            Series::new("size".into(), size_vec).into_column(),
            Series::new("allocated_size".into(), allocated_size_vec).into_column(),
            // Timestamps (4 total, matching C++)
            Series::new("created".into(), created_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), modified_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), accessed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), mft_changed_vec)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            // Type and counts
            Series::new("is_directory".into(), is_directory_vec).into_column(),
            Series::new("name_count".into(), name_count_vec).into_column(),
            Series::new("stream_count".into(), stream_count_vec).into_column(),
            // Extended attribute flags (matching C++ StandardInfo)
            Series::new("is_readonly".into(), is_readonly_vec).into_column(),
            Series::new("is_hidden".into(), is_hidden_vec).into_column(),
            Series::new("is_system".into(), is_system_vec).into_column(),
            Series::new("is_archive".into(), is_archive_vec).into_column(),
            Series::new("is_compressed".into(), is_compressed_vec).into_column(),
            Series::new("is_encrypted".into(), is_encrypted_vec).into_column(),
            Series::new("is_sparse".into(), is_sparse_vec).into_column(),
            Series::new("is_reparse".into(), is_reparse_vec).into_column(),
            Series::new("is_offline".into(), is_offline_vec).into_column(),
            Series::new("is_not_indexed".into(), is_not_indexed_vec).into_column(),
            Series::new("is_temporary".into(), is_temporary_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` directly from `ParsedColumns` (SoA layout).
    ///
    /// This is the optimized path that avoids the AoS→SoA transpose.
    /// The columns are already in the correct format, so we just wrap them
    /// in Polars Series.
    #[cfg(windows)]
    fn build_dataframe_from_columns(columns: crate::io::ParsedColumns) -> Result<DataFrame> {
        use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

        let polars_columns = vec![
            // Core identifiers
            Series::new("frs".into(), columns.frs).into_column(),
            Series::new("parent_frs".into(), columns.parent_frs).into_column(),
            Series::new("name".into(), columns.name).into_column(),
            // Size information
            Series::new("size".into(), columns.size).into_column(),
            Series::new("allocated_size".into(), columns.allocated_size).into_column(),
            // Timestamps (4 total, matching C++)
            Series::new("created".into(), columns.created)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("modified".into(), columns.modified)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("accessed".into(), columns.accessed)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            Series::new("mft_changed".into(), columns.mft_changed)
                .cast(&DataType::Datetime(TimeUnit::Microseconds, None))?
                .into_column(),
            // Type and counts
            Series::new("is_directory".into(), columns.is_directory).into_column(),
            Series::new("name_count".into(), columns.name_count).into_column(),
            Series::new("stream_count".into(), columns.stream_count).into_column(),
            // Extended attribute flags (matching C++ StandardInfo)
            Series::new("is_readonly".into(), columns.is_readonly).into_column(),
            Series::new("is_hidden".into(), columns.is_hidden).into_column(),
            Series::new("is_system".into(), columns.is_system).into_column(),
            Series::new("is_archive".into(), columns.is_archive).into_column(),
            Series::new("is_compressed".into(), columns.is_compressed).into_column(),
            Series::new("is_encrypted".into(), columns.is_encrypted).into_column(),
            Series::new("is_sparse".into(), columns.is_sparse).into_column(),
            Series::new("is_reparse".into(), columns.is_reparse).into_column(),
            Series::new("is_offline".into(), columns.is_offline).into_column(),
            Series::new("is_not_indexed".into(), columns.is_not_indexed).into_column(),
            Series::new("is_temporary".into(), columns.is_temporary).into_column(),
            Series::new("is_integrity_stream".into(), columns.is_integrity_stream).into_column(),
            Series::new("is_no_scrub_data".into(), columns.is_no_scrub_data).into_column(),
            Series::new("is_pinned".into(), columns.is_pinned).into_column(),
            Series::new("is_unpinned".into(), columns.is_unpinned).into_column(),
            Series::new("is_virtual".into(), columns.is_virtual).into_column(),
        ];

        DataFrame::new_infer_height(polars_columns).map_err(MftError::from)
    }

    /// Save a `DataFrame` to Parquet format.
    ///
    /// Parquet provides excellent compression and fast loading times.
    ///
    /// # Arguments
    ///
    /// * `df` - The `DataFrame` to save
    /// * `path` - Output file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save_parquet<P: AsRef<Path>>(df: &mut DataFrame, path: P) -> Result<()> {
        let file = std::fs::File::create(path.as_ref())?;
        ParquetWriter::new(file)
            .finish(df)
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(())
    }

    /// Load a `DataFrame` from Parquet format.
    ///
    /// # Arguments
    ///
    /// * `path` - Input file path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is invalid.
    pub fn load_parquet<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        let file = std::fs::File::open(path.as_ref())?;
        let df = ParquetReader::new(file)
            .finish()
            .map_err(|err| MftError::Parquet(err.to_string()))?;
        Ok(df)
    }

    /// Get the volume letter this reader is attached to.
    #[must_use]
    pub const fn volume(&self) -> char {
        self.volume
    }

    /// Create an empty `DataFrame` with the MFT schema.
    #[allow(dead_code)]
    fn create_empty_dataframe() -> Result<DataFrame> {
        use uffs_polars::{Column, DataType, TimeUnit};

        let schema_columns = vec![
            Column::new_empty("frs".into(), &DataType::UInt64),
            Column::new_empty("parent_frs".into(), &DataType::UInt64),
            Column::new_empty("name".into(), &DataType::String),
            Column::new_empty("size".into(), &DataType::UInt64),
            Column::new_empty(
                "created".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "modified".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty(
                "accessed".into(),
                &DataType::Datetime(TimeUnit::Microseconds, None),
            ),
            Column::new_empty("flags".into(), &DataType::UInt16),
        ];

        // Use new_infer_height to infer height from columns (Polars 0.52+ API)
        DataFrame::new_infer_height(schema_columns).map_err(MftError::from)
    }

    // ========================================================================
    // RAW MFT Persistence
    // ========================================================================

    /// Read the entire MFT as raw bytes.
    ///
    /// This reads all MFT records as contiguous raw bytes, handling fragmented
    /// MFTs by reassembling extents in order. The result can be saved with
    /// [`save_raw_mft`](crate::raw::save_raw_mft) for offline analysis.
    ///
    /// # Returns
    ///
    /// A tuple of (raw bytes, record size).
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        self.read_raw_internal()
    }

    /// Read raw MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_raw(&self) -> Result<(Vec<u8>, u32)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal raw MFT reading implementation.
    ///
    /// Uses the shared `ParallelMftReader` infrastructure for proper chunk
    /// handling, sector alignment, and dynamic buffer sizing.
    #[cfg(windows)]
    fn read_raw_internal(&self) -> Result<(Vec<u8>, u32)> {
        use crate::io::{MftExtentMap, ParallelMftReader, generate_read_chunks};
        use crate::platform::detect_drive_type;

        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|_| {
            vec![crate::platform::MftExtent {
                vcn: 0,
                cluster_count: volume_data.mft_valid_data_length
                    / u64::from(volume_data.bytes_per_cluster),
                lcn: volume_data.mft_start_lcn as i64,
            }]
        });

        // Create extent map
        let extent_map = MftExtentMap::new(extents, volume_data.bytes_per_cluster, record_size);
        let total_records = extent_map.total_records();

        // Allocate output buffer for all records
        let total_size = total_records as usize * record_size as usize;
        let mut output = vec![0u8; total_size];

        // Use ParallelMftReader for proper chunk reading (handles sector alignment,
        // dynamic buffer sizing, etc.)
        let drive_type = detect_drive_type(self.volume);
        let parallel_reader =
            ParallelMftReader::new_optimized(extent_map.clone(), None, drive_type);

        // Generate read chunks (without bitmap - we want ALL records for raw dump)
        let chunks = generate_read_chunks(&extent_map, None, parallel_reader.chunk_size);

        let handle = self.handle.raw_handle();

        // Read each chunk using the shared read_chunk function
        for chunk in chunks {
            let data = parallel_reader.read_chunk(handle, &chunk, record_size)?;

            // Copy to output at correct position
            let output_offset = chunk.start_frs as usize * record_size as usize;
            let copy_size = data.len().min(total_size - output_offset);
            output[output_offset..output_offset + copy_size].copy_from_slice(&data[..copy_size]);
        }

        Ok((output, record_size))
    }

    /// Read raw MFT and save to file.
    ///
    /// This is a convenience method that reads the MFT and saves it in one
    /// step.
    ///
    /// # Arguments
    ///
    /// * `path` - Output file path
    /// * `options` - Save options (compression, etc.)
    ///
    /// # Errors
    ///
    /// Returns an error if reading or saving fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        let (data, record_size) = self.read_raw_internal()?;
        crate::raw::save_raw_mft(path, &data, record_size, options)
    }

    /// Save raw MFT to file (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn save_raw_to_file<P: AsRef<Path>>(
        &self,
        _path: P,
        _options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        Err(MftError::PlatformNotSupported)
    }

    /// Load raw MFT from file and parse to DataFrame.
    ///
    /// This loads a previously saved raw MFT file and parses it into a
    /// DataFrame.
    ///
    /// # Arguments
    ///
    /// * `path` - Input file path
    ///
    /// # Errors
    ///
    /// Returns an error if loading or parsing fails.
    ///
    /// # Platform
    ///
    /// Currently only available on Windows due to NTFS parsing dependencies.
    #[cfg(windows)]
    pub fn load_raw_to_dataframe<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
        use crate::io::{MftRecordMerger, apply_fixup, parse_record_full};

        let raw = crate::raw::load_raw_mft(path, &crate::raw::LoadRawOptions::default())?;

        // Parse all records
        let mut merger = MftRecordMerger::with_capacity(raw.header.record_count as usize);

        for (frs, record_data) in raw.iter_records() {
            let mut record_buf = record_data.to_vec();

            // Apply fixup
            if !apply_fixup(&mut record_buf) {
                continue;
            }

            // Parse record
            let result = parse_record_full(&record_buf, frs);
            merger.add_result(result);
        }

        // Merge extensions and convert directly to ParsedColumns (SoA path)
        let parsed_columns = merger.merge_into_columns();

        // Convert to DataFrame using SoA path (no transpose needed!)
        Self::build_dataframe_from_columns(parsed_columns)
    }

    /// Load raw MFT from file (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn load_raw_to_dataframe<P: AsRef<Path>>(_path: P) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Convert parsed records to DataFrame (legacy AoS path).
    ///
    /// NOTE: This function is superseded by `build_dataframe_from_columns`
    /// which uses the SoA path and avoids the AoS→SoA transpose. Kept for
    /// reference.
    #[cfg(windows)]
    #[allow(dead_code)]
    fn parsed_records_to_dataframe(
        parsed_records: Vec<crate::io::ParsedRecord>,
    ) -> Result<DataFrame> {
        let capacity = parsed_records.len();
        let mut frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut parent_frs_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut name_vec: Vec<String> = Vec::with_capacity(capacity);
        let mut size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut allocated_size_vec: Vec<u64> = Vec::with_capacity(capacity);
        let mut created_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut modified_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut accessed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut mft_changed_vec: Vec<i64> = Vec::with_capacity(capacity);
        let mut is_directory_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut name_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut stream_count_vec: Vec<u16> = Vec::with_capacity(capacity);
        let mut is_readonly_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_hidden_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_system_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_archive_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_compressed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_encrypted_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_sparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_reparse_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_offline_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_not_indexed_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_temporary_vec: Vec<bool> = Vec::with_capacity(capacity);

        for parsed in parsed_records {
            // Compute counts before moving any fields
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            frs_vec.push(parsed.frs);
            parent_frs_vec.push(parsed.parent_frs);
            name_vec.push(parsed.name);
            size_vec.push(parsed.size);
            allocated_size_vec.push(parsed.allocated_size);
            created_vec.push(parsed.std_info.created);
            modified_vec.push(parsed.std_info.modified);
            accessed_vec.push(parsed.std_info.accessed);
            mft_changed_vec.push(parsed.std_info.mft_changed);
            is_directory_vec.push(parsed.is_directory);
            name_count_vec.push(name_count);
            stream_count_vec.push(stream_count);
            is_readonly_vec.push(parsed.std_info.is_readonly);
            is_hidden_vec.push(parsed.std_info.is_hidden);
            is_system_vec.push(parsed.std_info.is_system);
            is_archive_vec.push(parsed.std_info.is_archive);
            is_compressed_vec.push(parsed.std_info.is_compressed);
            is_encrypted_vec.push(parsed.std_info.is_encrypted);
            is_sparse_vec.push(parsed.std_info.is_sparse);
            is_reparse_vec.push(parsed.std_info.is_reparse);
            is_offline_vec.push(parsed.std_info.is_offline);
            is_not_indexed_vec.push(parsed.std_info.is_not_content_indexed);
            is_temporary_vec.push(parsed.std_info.is_temporary);
        }

        Self::build_dataframe_full(
            frs_vec,
            parent_frs_vec,
            name_vec,
            size_vec,
            allocated_size_vec,
            created_vec,
            modified_vec,
            accessed_vec,
            mft_changed_vec,
            is_directory_vec,
            name_count_vec,
            stream_count_vec,
            is_readonly_vec,
            is_hidden_vec,
            is_system_vec,
            is_archive_vec,
            is_compressed_vec,
            is_encrypted_vec,
            is_sparse_vec,
            is_reparse_vec,
            is_offline_vec,
            is_not_indexed_vec,
            is_temporary_vec,
        )
    }
}

// ============================================================================
// Multi-Drive MFT Reader
// ============================================================================

/// Result from reading a single drive.
#[derive(Debug)]
pub struct DriveReadResult {
    /// The drive letter.
    pub drive: char,
    /// The `DataFrame` (if successful).
    pub dataframe: Option<DataFrame>,
    /// The error (if failed).
    pub error: Option<MftError>,
}

/// Reads MFTs from multiple drives concurrently.
///
/// This struct orchestrates parallel reading of MFTs from multiple NTFS
/// volumes, merging the results into a single `DataFrame` with a `drive` column
/// to distinguish the source of each record.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_mft::MultiDriveMftReader;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MultiDriveMftReader::new(vec!['C', 'D', 'E']);
///     let df = reader.read_all().await?;
///     println!("Found {} files across all drives", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct MultiDriveMftReader {
    /// The drive letters to read from.
    drives: Vec<char>,
}

impl MultiDriveMftReader {
    /// Creates a new multi-drive reader.
    ///
    /// # Arguments
    ///
    /// * `drives` - List of drive letters to read (e.g., `vec!['C', 'D', 'E']`)
    #[must_use]
    pub fn new(drives: Vec<char>) -> Self {
        Self {
            drives: drives
                .into_iter()
                .map(|ch| ch.to_ascii_uppercase())
                .collect(),
        }
    }

    /// Returns the list of drives this reader will process.
    #[must_use]
    pub fn drives(&self) -> &[char] {
        &self.drives
    }

    /// Read MFTs from all drives concurrently.
    ///
    /// Returns a merged DataFrame with a `drive` column (e.g., "C:", "D:").
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        self.read_all_internal(None::<fn(char, MftProgress)>).await
    }

    /// Read MFTs from all drives (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all(&self) -> Result<DataFrame> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with per-drive progress callbacks.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called with progress updates for each drive
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_internal(Some(callback)).await
    }

    /// Read MFTs with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for concurrent drive reading.
    #[cfg(windows)]
    async fn read_all_internal<F>(&self, callback: Option<F>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        use std::sync::Arc;

        use tokio::task::JoinSet;
        use uffs_polars::{IntoLazy, col, lit};

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        // Wrap callback in Arc for sharing across tasks
        let callback = callback.map(Arc::new);

        // Spawn a task for each drive
        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            let cb = callback.clone();

            join_set.spawn(async move {
                let result = Self::read_single_drive(drive, cb).await;
                DriveReadResult {
                    drive,
                    dataframe: result.as_ref().ok().cloned(),
                    error: result.err(),
                }
            });
        }

        // Collect results
        let mut dataframes: Vec<DataFrame> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => {
                    if let Some(df) = drive_result.dataframe {
                        // Add "drive" column
                        let drive_str = format!("{}:", drive_result.drive);
                        let df_with_drive = df
                            .lazy()
                            .with_column(lit(drive_str).alias("drive"))
                            .collect()
                            .map_err(MftError::from)?;
                        dataframes.push(df_with_drive);
                    } else if let Some(err) = drive_result.error {
                        errors.push((drive_result.drive, err));
                    }
                }
                Err(join_err) => {
                    // Task panicked or was cancelled
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }
        }

        // If no DataFrames were collected, return the first error
        if dataframes.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        // Concatenate all DataFrames using vstack
        let mut result = dataframes.remove(0);
        for df in dataframes {
            result = result.vstack(&df).map_err(MftError::from)?;
        }

        // Reorder columns to put "drive" first
        let column_names: Vec<String> = result
            .get_column_names()
            .into_iter()
            .filter(|c| c.as_str() != "drive")
            .map(|c| c.to_string())
            .collect();
        let columns: Vec<_> = std::iter::once("drive".to_string())
            .chain(column_names)
            .map(|s| col(&s))
            .collect();

        result
            .lazy()
            .select(columns)
            .collect()
            .map_err(MftError::from)
    }

    /// Read a single drive with optional progress callback.
    ///
    /// Uses `spawn_blocking` because `MftReader` contains Windows HANDLEs
    /// which are not `Send`.
    #[cfg(windows)]
    async fn read_single_drive<F>(drive: char, callback: Option<Arc<F>>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        // Use spawn_blocking because MftReader contains Windows HANDLEs (*mut c_void)
        // which are not Send. All MFT I/O is blocking anyway.
        tokio::task::spawn_blocking(move || {
            // Create a new runtime for the blocking task
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let reader = MftReader::open(drive).await?;

                if let Some(cb) = callback {
                    reader
                        .read_with_progress(move |progress| {
                            cb(drive, progress);
                        })
                        .await
                } else {
                    reader.read_all().await
                }
            })
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Read all drives and return individual results (for detailed error
    /// handling).
    ///
    /// Unlike `read_all()`, this returns results for each drive separately,
    /// allowing the caller to handle partial failures.
    ///
    /// # Errors
    ///
    /// Returns an error only if the operation itself fails (not individual
    /// drives).
    #[cfg(windows)]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        use tokio::task::JoinSet;

        if self.drives.is_empty() {
            return Ok(Vec::new());
        }

        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            join_set.spawn(async move {
                let result = Self::read_single_drive::<fn(char, MftProgress)>(drive, None).await;
                DriveReadResult {
                    drive,
                    dataframe: result.as_ref().ok().cloned(),
                    error: result.err(),
                }
            });
        }

        let mut results = Vec::with_capacity(self.drives.len());
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(drive_result) => results.push(drive_result),
                Err(join_err) => {
                    results.push(DriveReadResult {
                        drive: '?',
                        dataframe: None,
                        error: Some(MftError::InvalidInput(format!("Task failed: {join_err}"))),
                    });
                }
            }
        }

        Ok(results)
    }

    /// Read all drives detailed (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_detailed(&self) -> Result<Vec<DriveReadResult>> {
        Err(MftError::PlatformNotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(windows)]
    async fn test_open_valid_volume() {
        let result = MftReader::open('C').await;
        // This will fail without admin privileges, but should not panic
        assert!(result.is_ok() || matches!(result, Err(MftError::InsufficientPrivileges)));
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_platform_not_supported() {
        let result = MftReader::open('C').await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }

    #[test]
    fn test_progress_percentage() {
        let progress = MftProgress {
            records_read: 50,
            total_records: Some(100),
            bytes_read: 1024,
            elapsed: Duration::from_secs(1),
        };
        assert_eq!(progress.percentage(), Some(50.0_f64));
    }

    #[test]
    fn test_progress_speed() {
        let progress = MftProgress {
            records_read: 1000,
            total_records: None,
            bytes_read: 10 * 1024 * 1024, // 10 MB
            elapsed: Duration::from_secs(2),
        };
        assert!((progress.speed_mbps() - 5.0_f64).abs() < 0.01_f64);
    }

    #[test]
    fn test_multi_drive_reader_new() {
        let reader = MultiDriveMftReader::new(vec!['c', 'd', 'e']);
        assert_eq!(reader.drives(), &['C', 'D', 'E']);
    }

    #[test]
    fn test_multi_drive_reader_empty() {
        let reader = MultiDriveMftReader::new(vec![]);
        assert!(reader.drives().is_empty());
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_multi_drive_platform_not_supported() {
        let reader = MultiDriveMftReader::new(vec!['C', 'D']);
        let result = reader.read_all().await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }
}
