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
use crate::ntfs::StreamInfo;
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
/// - `Pipelined`: True I/O and CPU overlap with separate threads
/// - `PipelinedParallel`: Pipelined I/O with multi-core parallel parsing
/// - `Auto`: Automatically selects based on detected drive type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MftReadMode {
    /// Automatic mode selection based on drive type (default).
    /// - SSD → `Parallel`
    /// - HDD → `PipelinedParallel`
    /// - Unknown → `Parallel`
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
    /// Note: Parsing is single-threaded. Use `PipelinedParallel` for
    /// multi-core.
    Pipelined,
    /// Pipelined parallel mode: Pipelined I/O with multi-core parallel parsing.
    /// Best for HDDs with multi-core CPUs - combines I/O overlap with Rayon
    /// parallel parsing for maximum throughput.
    PipelinedParallel,
    /// IOCP parallel mode: Windows I/O Completion Ports with multiple
    /// concurrent reads in flight. Mirrors the C++ implementation for
    /// maximum I/O overlap. Best for HDDs where multiple outstanding reads
    /// can hide latency.
    IocpParallel,
    /// Bulk mode: C++ style "read all, then parse".
    /// Pre-allocates single buffer for entire MFT, reads all extents
    /// directly into it (zero copies), then parses in parallel.
    /// Uses bitmap skip optimization to reduce I/O.
    /// Best for HDDs with sufficient RAM (~12GB for large drives).
    Bulk,
    /// Bulk IOCP mode: True C++ style - queues ALL reads to IOCP at once.
    /// Combines bulk buffer allocation with IOCP for maximum I/O overlap.
    /// Windows I/O manager optimizes disk head scheduling across all reads.
    /// Best for HDDs - lets the OS schedule reads optimally.
    BulkIocp,
    /// Sliding window IOCP mode: C++ style with 2 reads in flight.
    /// Only 2 reads queued at a time (not thousands!), with per-read buffer
    /// recycling. This matches the actual C++ implementation which uses a
    /// sliding window, not bulk queuing.
    /// Best for HDDs - minimal I/O scheduler overhead, maximum throughput.
    SlidingIocp,
    /// Sliding window IOCP with inline parsing: Full C++ parity.
    /// Parses each 1MB chunk as it completes (no buffering), builds index
    /// incrementally during I/O, creates parent placeholders on-demand.
    /// Eliminates separate parse and index build phases.
    /// Best for HDDs - overlaps CPU work with I/O for maximum throughput.
    SlidingIocpInline,
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
            Self::PipelinedParallel => "pipelined-parallel",
            Self::IocpParallel => "iocp-parallel",
            Self::Bulk => "bulk",
            Self::BulkIocp => "bulk-iocp",
            Self::SlidingIocp => "sliding-iocp",
            Self::SlidingIocpInline => "sliding-iocp-inline",
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
            "pipelined-parallel" | "pipelinedparallel" => Ok(Self::PipelinedParallel),
            "iocp-parallel" | "iocpparallel" | "iocp" => Ok(Self::IocpParallel),
            "bulk" => Ok(Self::Bulk),
            "bulk-iocp" | "bulkiocp" => Ok(Self::BulkIocp),
            "sliding-iocp" | "slidingiocp" | "sliding" => Ok(Self::SlidingIocp),
            "sliding-iocp-inline" | "slidingiocpinline" | "inline" => Ok(Self::SlidingIocpInline),
            _ => Err(format!(
                "Invalid read mode '{s}'. Valid options: auto, parallel, streaming, prefetch, pipelined, pipelined-parallel, iocp-parallel, bulk, bulk-iocp, sliding-iocp, sliding-iocp-inline"
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
#[allow(clippy::struct_excessive_bools)] // Builder pattern with boolean options
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
    /// Whether to use the MFT bitmap for optimization.
    ///
    /// - `true` (default): Use bitmap to skip unused records (faster).
    /// - `false`: Read all records regardless of bitmap (for debugging).
    use_bitmap: bool,
    /// Whether to expand hard links to separate rows.
    ///
    /// - `true` (default): Each hard link becomes a separate row, matching C++
    ///   behavior and user expectations (what they see in Explorer).
    /// - `false`: One row per unique FRS (power user mode, smaller output).
    expand_links: bool,
    /// Whether to add placeholder records for missing parent directories.
    ///
    /// - `true` (default): Add placeholders for path resolution.
    /// - `false`: Skip placeholders (faster, but path resolution may fail for
    ///   some files whose parents weren't in the MFT).
    ///
    /// # Performance Optimization (2026-01-23)
    ///
    /// C++ team doesn't add placeholders upfront - they resolve paths lazily.
    /// Disabling this saves ~15% of CPU time during indexing.
    add_placeholders: bool,
    /// Number of concurrent I/O operations (reads in flight).
    ///
    /// - Default: 2 for HDD (optimal for sequential reads)
    /// - Higher values (8-32) may help on SSD/NVMe
    ///
    /// For HDDs, more concurrency can cause seeks and hurt performance.
    /// For `NVMe`, high concurrency (16-32) is needed to saturate the device.
    concurrency: Option<usize>,
    /// I/O chunk size in bytes.
    ///
    /// - Default: 1MB (1024 * 1024)
    /// - Larger chunks (2-4MB) reduce syscall overhead but increase latency
    io_size: Option<usize>,
    /// Whether to use parallel parsing (M3 optimization).
    ///
    /// - `None` (default): Auto-detect based on drive type (enabled for `NVMe`)
    /// - `Some(true)`: Force parallel parsing
    /// - `Some(false)`: Force inline parsing
    ///
    /// Parallel parsing uses worker threads to parse MFT records in parallel
    /// with I/O. This is beneficial for `NVMe` drives where I/O is faster than
    /// parsing.
    parallel_parse: Option<bool>,
    /// Number of parsing worker threads (only used with parallel parsing).
    ///
    /// - `None` (default): Use number of CPU cores
    /// - `Some(n)`: Use exactly n worker threads
    parse_workers: Option<usize>,
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
            // Enable extension merging by default for C++ parity.
            // Extension records contain additional attributes for files with
            // many hard links or alternate data streams. Without merging,
            // ~1% of files may have incomplete attribute information.
            // The performance impact is ~10-15% slower, but correctness is
            // more important for file search accuracy.
            merge_extensions: true,
            use_bitmap: true,       // Use bitmap optimization by default
            expand_links: true,     // Expand hard links by default (C++ parity)
            add_placeholders: true, // Add placeholders by default for path resolution
            concurrency: None,      // Use default (2 for HDD)
            io_size: None,          // Use default (1MB)
            parallel_parse: None,   // Auto-detect based on drive type
            parse_workers: None,    // Use num_cpus
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

    /// Sets whether to use the MFT bitmap for optimization.
    ///
    /// The bitmap indicates which MFT records are "in use" vs "free".
    /// When enabled (default), unused records are skipped for faster reads.
    /// Disable this for debugging if you suspect the bitmap is causing
    /// records to be incorrectly skipped.
    ///
    /// # Arguments
    ///
    /// * `use_bitmap` - If `true` (default), use bitmap optimization. If
    ///   `false`, read all records regardless of bitmap.
    #[must_use]
    pub const fn with_use_bitmap(mut self, use_bitmap: bool) -> Self {
        self.use_bitmap = use_bitmap;
        self
    }

    /// Returns whether bitmap optimization is enabled.
    #[must_use]
    pub const fn use_bitmap(&self) -> bool {
        self.use_bitmap
    }

    /// Sets whether to expand hard links to separate rows.
    ///
    /// When enabled (default), each hard link becomes a separate row in the
    /// output. This matches C++ behavior and user expectations - if a file
    /// has 3 hard links, users see 3 entries in Explorer, so they expect
    /// 3 entries in search results.
    ///
    /// When disabled (`--unique` mode), only one row per unique FRS is output.
    /// This is useful for power users who want to count unique files, not
    /// paths.
    ///
    /// # Arguments
    ///
    /// * `expand` - If `true` (default), expand hard links. If `false`, output
    ///   one row per unique FRS.
    #[must_use]
    pub const fn with_expand_links(mut self, expand: bool) -> Self {
        self.expand_links = expand;
        self
    }

    /// Returns whether hard link expansion is enabled.
    #[must_use]
    pub const fn expand_links(&self) -> bool {
        self.expand_links
    }

    /// Sets whether to add placeholder records for missing parent directories.
    ///
    /// When enabled (default), placeholder records are created for parent
    /// directories that are referenced but not present in the MFT. This ensures
    /// path resolution works for all files.
    ///
    /// When disabled, placeholder creation is skipped. This is faster (~15%
    /// improvement) but path resolution may fail for some files whose parents
    /// weren't in the MFT.
    ///
    /// # Performance Optimization (2026-01-23)
    ///
    /// C++ team doesn't add placeholders upfront - they resolve paths lazily.
    /// Disabling this matches C++ behavior and saves ~15% of CPU time.
    ///
    /// # Arguments
    ///
    /// * `add` - If `true` (default), add placeholders. If `false`, skip.
    #[must_use]
    pub const fn with_add_placeholders(mut self, add: bool) -> Self {
        self.add_placeholders = add;
        self
    }

    /// Returns whether placeholder creation is enabled.
    #[must_use]
    pub const fn add_placeholders(&self) -> bool {
        self.add_placeholders
    }

    /// Sets the number of concurrent I/O operations (reads in flight).
    ///
    /// This controls how many async read operations are queued simultaneously.
    /// The optimal value depends on the drive type:
    ///
    /// - **HDD**: 2 (default) - More concurrency causes seeks and hurts
    ///   performance
    /// - **SSD**: 4-8 - Can benefit from moderate parallelism
    /// - **`NVMe`**: 16-32 - High concurrency needed to saturate the device
    ///
    /// # Arguments
    ///
    /// * `concurrency` - Number of I/O operations in flight (1-64)
    #[must_use]
    pub const fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = Some(concurrency);
        self
    }

    /// Returns the configured concurrency, or None for default.
    #[must_use]
    pub const fn concurrency(&self) -> Option<usize> {
        self.concurrency
    }

    /// Sets the I/O chunk size in bytes.
    ///
    /// This controls the size of each async read operation. Larger chunks
    /// reduce syscall overhead but increase latency per completion.
    ///
    /// - **Default**: 1MB (1024 * 1024)
    /// - **SSD**: 2MB may be slightly better
    /// - **`NVMe`**: 4MB can reduce overhead
    ///
    /// # Arguments
    ///
    /// * `io_size` - I/O chunk size in bytes (e.g., 1024*1024 for 1MB)
    #[must_use]
    pub const fn with_io_size(mut self, io_size: usize) -> Self {
        self.io_size = Some(io_size);
        self
    }

    /// Returns the configured I/O size, or None for default.
    #[must_use]
    pub const fn io_size(&self) -> Option<usize> {
        self.io_size
    }

    /// Sets whether to use parallel parsing (M3 optimization).
    ///
    /// Parallel parsing uses worker threads to parse MFT records in parallel
    /// with I/O. This is beneficial for `NVMe` drives where I/O is faster than
    /// parsing.
    ///
    /// # Arguments
    ///
    /// * `parallel` - If `true`, use parallel parsing. If `false`, use inline
    ///   parsing.
    #[must_use]
    pub const fn with_parallel_parse(mut self, parallel: bool) -> Self {
        self.parallel_parse = Some(parallel);
        self
    }

    /// Returns whether parallel parsing is enabled.
    #[must_use]
    pub const fn parallel_parse(&self) -> Option<bool> {
        self.parallel_parse
    }

    /// Sets the number of parsing worker threads.
    ///
    /// Only used when parallel parsing is enabled.
    ///
    /// # Arguments
    ///
    /// * `workers` - Number of worker threads (None = use `num_cpus`)
    #[must_use]
    pub const fn with_parse_workers(mut self, workers: Option<usize>) -> Self {
        self.parse_workers = workers;
        self
    }

    /// Returns the configured number of parse workers.
    #[must_use]
    pub const fn parse_workers(&self) -> Option<usize> {
        self.parse_workers
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

    /// Read the entire MFT into a lean `MftIndex` (fast path).
    ///
    /// This method builds a compact `MftIndex` structure instead of a Polars
    /// DataFrame. It's significantly faster because it avoids the DataFrame
    /// building overhead (~15-20s on large drives).
    ///
    /// Use this when you need fast indexing and searching. Convert to DataFrame
    /// later with `MftIndex::to_dataframe()` if you need Polars analytics.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    #[allow(clippy::unused_async)]
    pub async fn read_all_index(&self) -> Result<crate::index::MftIndex> {
        self.read_mft_index_internal(None::<fn(MftProgress)>)
    }

    /// Read MFT into lean index (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_index(&self) -> Result<crate::index::MftIndex> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT into lean index with progress callback.
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
    pub async fn read_index_with_progress<F>(&self, callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_index_internal(Some(callback))
    }

    /// Read MFT into lean index with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_index_with_progress<F>(&self, _callback: F) -> Result<crate::index::MftIndex>
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

        // Try to get the MFT bitmap for optimization (if enabled)
        let bitmap = if self.use_bitmap {
            let bm = self.handle.get_mft_bitmap().ok();
            if let Some(ref b) = bm {
                let in_use = b.count_in_use();
                info!(
                    in_use_records = in_use,
                    skip_percentage = 100.0 - (in_use as f64 / total_records as f64 * 100.0),
                    "MFT bitmap loaded - will skip unused records"
                );
            } else {
                debug!("No MFT bitmap available - reading all records");
            }
            bm
        } else {
            info!("Bitmap optimization DISABLED (--no-bitmap) - reading ALL records");
            None
        };

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
        // C++ team insight: "read all then parse" is faster than pipelining even on HDD
        // because: no context switching, CPU cache stays hot, no channel overhead,
        // OS can optimize continuous sequential reads better.
        // For read_all() (returns Vec<ParsedRecord>), use SlidingIocp for IOCP-based
        // I/O.
        let effective_mode = match self.mode {
            MftReadMode::Auto => {
                // Auto-select based on drive type - use IOCP for all drive types
                // The concurrency is automatically adjusted per drive type
                match drive_type {
                    // NVMe: IOCP with 32 concurrent reads
                    crate::platform::DriveType::Nvme => MftReadMode::SlidingIocp,
                    // SSD: IOCP with 8 concurrent reads
                    crate::platform::DriveType::Ssd => MftReadMode::SlidingIocp,
                    // HDD: IOCP with 2 concurrent reads (sequential is optimal)
                    crate::platform::DriveType::Hdd => MftReadMode::SlidingIocp,
                    // Unknown: Conservative IOCP approach
                    crate::platform::DriveType::Unknown => MftReadMode::SlidingIocp,
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
            MftReadMode::PipelinedParallel => {
                // Pipelined parallel mode: I/O overlap + multi-core parsing (best for HDD)
                let pipelined_reader =
                    crate::io::PipelinedMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    pipelined_reader.read_all_pipelined_parallel(
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
                    pipelined_reader
                        .read_all_pipelined_parallel::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::IocpParallel => {
                // IOCP parallel mode: Multiple overlapped reads in flight (best for HDD)
                // IOCP requires FILE_FLAG_OVERLAPPED, so we open a separate handle
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let iocp_reader = crate::io::IocpMftReader::new(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    iocp_reader.read_all_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    iocp_reader.read_all_iocp::<fn(u64, u64)>(overlapped_handle, true, None)
                };

                // Close the overlapped handle
                // SAFETY: overlapped_handle is a valid handle opened by open_overlapped_handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::Bulk => {
                // Bulk mode: C++ style "read all, then parse"
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk(
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
                    parallel_reader.read_all_bulk::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::BulkIocp => {
                // Bulk IOCP mode: True C++ style - queues ALL reads to IOCP at once
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    parallel_reader.read_all_bulk_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocp => {
                // Sliding window IOCP mode: C++ style with 2 reads in flight
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_sliding_window_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocpInline => {
                // SlidingIocpInline is designed for direct index building.
                // For read_mft_internal (which returns Vec<ParsedRecord>), fall back to
                // SlidingIocp.
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_sliding_window_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
        };

        // Add placeholder records for missing parent directories.
        // This matches C++ behavior where `at()` creates placeholder records
        // for any referenced FRS that hasn't been seen yet.
        // Can be disabled with `with_add_placeholders(false)` for ~15% speedup.
        let mut parsed_records = parsed_records;
        if self.add_placeholders {
            let placeholders_added =
                crate::io::add_missing_parent_placeholders_to_vec(&mut parsed_records);
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

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
        //
        // With expand_links=true (default), we expand hard links to separate rows.
        // Stats are computed per unique FRS (before expansion).
        let expand_links = self.expand_links;
        let base_capacity = parsed_records.len();
        // If expanding links, estimate ~20% more rows for hard links
        let capacity = if expand_links {
            (base_capacity as f64 * 1.2) as usize
        } else {
            base_capacity
        };
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
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);
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
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);

        // Single pass: build columns AND compute stats simultaneously
        for parsed in parsed_records {
            let name_count = parsed.name_count();
            let stream_count = parsed.stream_count();

            // Accumulate stats inline (no separate loop!)
            // Stats are per unique FRS, not per expanded link
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

            // Build column vectors - expand (names × streams) if enabled
            if expand_links {
                // Get names to iterate over
                let names: Vec<_> = if parsed.names.is_empty() {
                    vec![crate::ntfs::NameInfo {
                        name: parsed.name.clone(),
                        parent_frs: parsed.parent_frs,
                        namespace: 3,
                    }]
                } else {
                    parsed.names.clone()
                };

                // Get streams to iterate over
                let streams: Vec<_> = if parsed.streams.is_empty() {
                    vec![StreamInfo {
                        name: String::new(),
                        size: parsed.size,
                        allocated_size: parsed.allocated_size,
                        is_sparse: false,
                        is_compressed: false,
                    }]
                } else {
                    parsed.streams.clone()
                };

                // Expand: one row per (name × stream) combination
                for name_info in &names {
                    for stream_info in &streams {
                        frs_vec.push(parsed.frs);
                        parent_frs_vec.push(name_info.parent_frs);
                        name_vec.push(name_info.name.clone());
                        // Use stream-specific size for ADS
                        let (size, alloc) = if stream_info.name.is_empty() {
                            (parsed.size, parsed.allocated_size)
                        } else {
                            (stream_info.size, stream_info.allocated_size)
                        };
                        size_vec.push(size);
                        allocated_size_vec.push(alloc);
                        created_vec.push(parsed.std_info.created);
                        modified_vec.push(parsed.std_info.modified);
                        accessed_vec.push(parsed.std_info.accessed);
                        mft_changed_vec.push(parsed.std_info.mft_changed);
                        is_directory_vec.push(parsed.is_directory);
                        name_count_vec.push(1);
                        stream_count_vec.push(1);
                        stream_name_vec.push(stream_info.name.clone());
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
                        is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
                        is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
                        is_pinned_vec.push(parsed.std_info.is_pinned);
                        is_unpinned_vec.push(parsed.std_info.is_unpinned);
                        is_virtual_vec.push(parsed.std_info.is_virtual);
                        flags_vec.push(parsed.std_info.to_raw_flags());
                    }
                }
            } else {
                // No expansion: one row per FRS (use primary name)
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
                stream_name_vec.push(String::new()); // Default stream
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
                is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
                is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
                is_pinned_vec.push(parsed.std_info.is_pinned);
                is_unpinned_vec.push(parsed.std_info.is_unpinned);
                is_virtual_vec.push(parsed.std_info.is_virtual);
                flags_vec.push(parsed.std_info.to_raw_flags());
            }
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
            stream_name_vec,
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
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
        )
    }

    /// Internal implementation for building lean `MftIndex`.
    ///
    /// This is the fast path that avoids DataFrame building overhead.
    /// Uses the same I/O and parsing as `read_mft_internal`, but builds
    /// a compact `MftIndex` instead of a Polars DataFrame.
    #[cfg(windows)]
    #[allow(clippy::too_many_lines)]
    fn read_mft_index_internal<F>(&self, callback: Option<F>) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress),
    {
        use crate::index::MftIndex;
        use crate::io::{MftExtentMap, ParallelMftReader};
        use crate::platform::detect_drive_type;

        info!(volume = %self.volume, "Starting MFT read (lean index)");

        let start_time = Instant::now();
        let record_size = self.handle.file_record_size();
        let volume_data = self.handle.volume_data();

        // Detect drive type for optimal I/O tuning
        let drive_type = detect_drive_type(self.volume);
        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            "🚀 Drive type detected for I/O optimization (lean index)"
        );

        // Get MFT extents for fragmented MFT support
        let extents = self.handle.get_mft_extents().unwrap_or_else(|e| {
            warn!(error = ?e, "Failed to get MFT extents, using fallback");
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
        let bitmap = if self.use_bitmap {
            let bm = self.handle.get_mft_bitmap().ok();
            if let Some(ref b) = bm {
                let in_use = b.count_in_use();
                info!(
                    in_use_records = in_use,
                    skip_percentage = 100.0 - (in_use as f64 / total_records as f64 * 100.0),
                    "MFT bitmap loaded - will skip unused records"
                );
            }
            bm
        } else {
            info!("Bitmap optimization DISABLED - reading ALL records");
            None
        };

        // Report initial progress
        if let Some(ref cb) = callback {
            cb(MftProgress {
                records_read: 0,
                total_records: Some(total_records),
                bytes_read: 0,
                elapsed: start_time.elapsed(),
            });
        }

        // Select reader based on mode
        // C++ team insight: "read all then parse" is faster than pipelining even on HDD
        // because: no context switching, CPU cache stays hot, no channel overhead,
        // OS can optimize continuous sequential reads better.
        // For lean index (MftIndex), use SlidingIocpInline for NVMe/SSD - this uses
        // IOCP with multiple reads in flight and inline parsing, matching C++
        // performance.
        let effective_mode = match self.mode {
            MftReadMode::Auto => match drive_type {
                // NVMe: Use IOCP with 32 concurrent reads + parallel parsing (2024 MB/s)
                crate::platform::DriveType::Nvme => MftReadMode::SlidingIocpInline,
                // SSD: Use IOCP with 8 concurrent reads + parallel parsing
                crate::platform::DriveType::Ssd => MftReadMode::SlidingIocpInline,
                // HDD: Use IOCP with 2 concurrent reads (sequential is optimal)
                crate::platform::DriveType::Hdd => MftReadMode::SlidingIocpInline,
                // Unknown: Conservative IOCP approach
                crate::platform::DriveType::Unknown => MftReadMode::SlidingIocpInline,
            },
            mode => mode,
        };

        info!(mode = %effective_mode, "🚀 Using read mode (lean index)");

        let handle = self.handle.raw_handle();
        let total_bytes = total_records * u64::from(record_size);

        // Read using the selected mode (same as read_mft_internal)
        let parsed_records = match effective_mode {
            MftReadMode::Parallel | MftReadMode::Auto => {
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
            MftReadMode::Pipelined => {
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
            MftReadMode::PipelinedParallel => {
                let pipelined_reader =
                    crate::io::PipelinedMftReader::new(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    pipelined_reader.read_all_pipelined_parallel(
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
                    pipelined_reader
                        .read_all_pipelined_parallel::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::IocpParallel => {
                // IOCP parallel mode: Multiple overlapped reads in flight
                // IOCP requires FILE_FLAG_OVERLAPPED, so we open a separate handle
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let iocp_reader = crate::io::IocpMftReader::new(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    iocp_reader.read_all_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    iocp_reader.read_all_iocp::<fn(u64, u64)>(overlapped_handle, true, None)
                };

                // Close the overlapped handle
                // SAFETY: overlapped_handle is a valid handle opened by open_overlapped_handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::Bulk => {
                // Bulk mode: C++ style "read all, then parse"
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk(
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
                    parallel_reader.read_all_bulk::<fn(u64, u64)>(handle, true, None)?
                }
            }
            MftReadMode::BulkIocp => {
                // Bulk IOCP mode: True C++ style - queues ALL reads to IOCP at once
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_bulk_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    parallel_reader.read_all_bulk_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocp => {
                // Sliding window IOCP mode: C++ style with 2 reads in flight
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = if let Some(ref cb) = callback {
                    let cb_ref = cb;
                    let start = start_time;
                    parallel_reader.read_all_sliding_window_iocp(
                        overlapped_handle,
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
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp::<fn(u64, u64)>(
                        overlapped_handle,
                        true,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocpInline => {
                // Sliding window IOCP with inline parsing: Full C++ parity
                // This mode returns MftIndex directly, skipping the intermediate
                // Vec<ParsedRecord>
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                // Determine if we should use parallel parsing (M3 optimization)
                let use_parallel = self.parallel_parse.unwrap_or_else(|| {
                    // Auto-detect: enable for NVMe where I/O is faster than parsing
                    drive_type.benefits_from_parallel_parsing()
                });

                let result = if use_parallel {
                    info!("🚀 Using PARALLEL parsing (M3 optimization)");
                    parallel_reader.read_all_sliding_window_iocp_to_index_parallel::<fn(u64, u64)>(
                        overlapped_handle,
                        self.volume,
                        self.concurrency,
                        self.io_size,
                        self.parse_workers,
                        None,
                    )
                } else {
                    parallel_reader.read_all_sliding_window_iocp_to_index::<fn(u64, u64)>(
                        overlapped_handle,
                        self.volume,
                        self.concurrency,
                        self.io_size,
                        None,
                    )
                };

                // Close the overlapped handle
                #[allow(unsafe_code)]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                let index = result?;

                // Report final progress
                if let Some(ref cb) = callback {
                    cb(MftProgress {
                        records_read: total_records,
                        total_records: Some(total_records),
                        bytes_read: total_bytes,
                        elapsed: start_time.elapsed(),
                    });
                }

                // Return early - we already have the index, no need for placeholder/build
                // phases
                return Ok(index);
            }
            MftReadMode::Streaming | MftReadMode::Prefetch => {
                // Fallback to parallel for streaming/prefetch modes in lean index
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
                parallel_reader
                    .read_all_parallel_with_progress::<fn(u64, u64)>(handle, true, None)?
            }
        };

        // Add placeholder records for missing parent directories.
        // Can be disabled with `with_add_placeholders(false)` for ~15% speedup.
        let mut parsed_records = parsed_records;
        if self.add_placeholders {
            let placeholders_added =
                crate::io::add_missing_parent_placeholders_to_vec(&mut parsed_records);
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

        let read_elapsed = start_time.elapsed();
        let records_parsed_count = parsed_records.len();
        let throughput_mb_s = if read_elapsed.as_secs_f64() > 0.0 {
            (total_bytes as f64 / (1024.0 * 1024.0)) / read_elapsed.as_secs_f64()
        } else {
            0.0
        };

        info!(
            records_parsed = records_parsed_count,
            elapsed_ms = read_elapsed.as_millis(),
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            "✅ MFT read complete, building lean index"
        );

        // Build lean MftIndex (fast path - no DataFrame overhead)
        let index_start = Instant::now();
        let index = MftIndex::from_parsed_records(self.volume, parsed_records);
        let index_elapsed = index_start.elapsed();

        info!(
            records = index.records.len(),
            names_buffer_kb = index.names.len() / 1024,
            index_build_ms = index_elapsed.as_millis(),
            "✅ Lean index built"
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

        Ok(index)
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

        let mut parsed_columns = parallel_reader.read_all_parallel_to_columns::<fn(u64, u64)>(
            handle,
            self.merge_extensions,
            self.expand_links,
            None,
        )?;

        // Add placeholder records for missing parent directories.
        // This matches C++ behavior where `at()` creates placeholder records
        // for any referenced FRS that hasn't been seen yet.
        // Can be disabled with `with_add_placeholders(false)` for ~15% speedup.
        if self.add_placeholders {
            let placeholders_added = parsed_columns.add_missing_parent_placeholders();
            if placeholders_added > 0 {
                debug!(
                    placeholders_added,
                    "Added placeholder records for path resolution"
                );
            }
        }

        let read_parse_ms = read_parse_start.elapsed().as_millis() as u64;
        let records_parsed = parsed_columns.len();

        // Note: Currently read and parse are interleaved in ParallelMftReader.
        // For now, we report combined time. Future: instrument inside
        // ParallelMftReader. Estimate: ~70% read, ~30% parse on HDD; ~30% read,
        // ~70% parse on SSD
        let (read_ms, parse_ms, merge_ms) = match drive_type {
            crate::platform::DriveType::Nvme => {
                // NVMe: I/O is extremely fast, parsing dominates
                let read_est = read_parse_ms * 20 / 100;
                let parse_est = read_parse_ms * 60 / 100;
                let merge_est = read_parse_ms * 20 / 100;
                (read_est, parse_est, merge_est)
            }
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
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);

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
            stream_name_vec.push(String::new()); // No expansion, use empty stream name
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
            is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
            is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
            is_pinned_vec.push(parsed.std_info.is_pinned);
            is_unpinned_vec.push(parsed.std_info.is_unpinned);
            is_virtual_vec.push(parsed.std_info.is_virtual);
            flags_vec.push(parsed.std_info.to_raw_flags());
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
            stream_name_vec,
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
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
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
        stream_name_vec: Vec<String>,
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
        is_integrity_stream_vec: Vec<bool>,
        is_no_scrub_data_vec: Vec<bool>,
        is_pinned_vec: Vec<bool>,
        is_unpinned_vec: Vec<bool>,
        is_virtual_vec: Vec<bool>,
        flags_vec: Vec<u32>,
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
            Series::new("stream_name".into(), stream_name_vec).into_column(),
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
            // Additional flags for C++ parity
            Series::new("is_integrity_stream".into(), is_integrity_stream_vec).into_column(),
            Series::new("is_no_scrub_data".into(), is_no_scrub_data_vec).into_column(),
            Series::new("is_pinned".into(), is_pinned_vec).into_column(),
            Series::new("is_unpinned".into(), is_unpinned_vec).into_column(),
            Series::new("is_virtual".into(), is_virtual_vec).into_column(),
            // Raw attribute flags (combined value for C++ parity)
            Series::new("flags".into(), flags_vec).into_column(),
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
            Series::new("stream_name".into(), columns.stream_name).into_column(),
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
            // Raw attribute flags (combined value for C++ parity)
            Series::new("flags".into(), columns.flags).into_column(),
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

    /// Read raw MFT and save to file using streaming I/O.
    ///
    /// This method uses streaming I/O to avoid buffering the entire MFT in
    /// memory. Each chunk is read from disk and immediately written to the
    /// output file, enabling efficient saves of large MFTs (10+ GB).
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
        self.save_raw_streaming(path, options)
    }

    /// Internal streaming save implementation.
    ///
    /// Uses double-buffering with a dedicated reader thread to overlap
    /// disk reads with file writes for maximum throughput.
    #[cfg(windows)]
    #[allow(unsafe_code)] // Required: Windows FFI (ReadFile, SetFilePointerEx)
    fn save_raw_streaming<P: AsRef<Path>>(
        &self,
        path: P,
        options: &crate::raw::SaveRawOptions,
    ) -> Result<crate::raw::RawMftHeader> {
        use std::thread;

        use crossbeam_channel::{Receiver, Sender, bounded};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};

        use crate::io::{AlignedBuffer, MftExtentMap, SECTOR_SIZE, generate_read_chunks};
        use crate::platform::detect_drive_type;
        use crate::raw::StreamingRawMftWriter;

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

        // Determine chunk size based on drive type
        // Use larger chunks (4-8 MB) for streaming to reduce syscall overhead
        let drive_type = detect_drive_type(self.volume);
        let chunk_size = match drive_type {
            crate::platform::DriveType::Nvme => 8 * 1024 * 1024, // 8 MB for NVMe
            crate::platform::DriveType::Ssd => 8 * 1024 * 1024,  // 8 MB for SSD
            crate::platform::DriveType::Hdd => 4 * 1024 * 1024,  // 4 MB for HDD
            crate::platform::DriveType::Unknown => 4 * 1024 * 1024,
        };

        // Generate read chunks
        let chunks = generate_read_chunks(&extent_map, None, chunk_size);
        let total_chunks = chunks.len();

        info!(
            "Streaming save: {} chunks, {} MB each, drive type: {:?}",
            total_chunks,
            chunk_size / (1024 * 1024),
            drive_type
        );

        // Create streaming writer
        let mut writer = StreamingRawMftWriter::new(path, record_size, options)?;

        // Use double-buffering with a reader thread for I/O overlap
        // Channel capacity of 2 gives us double-buffering
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = bounded(2);

        // Convert HANDLE to usize for thread transfer (HANDLE is just a pointer)
        // SAFETY: Windows file handles are thread-safe kernel objects. We convert
        // to usize to avoid Send issues with the raw pointer inside HANDLE.
        let handle_ptr = self.handle.raw_handle().0 as usize;
        let record_size_copy = record_size;

        // Spawn reader thread
        let reader_handle = thread::spawn(move || -> Result<()> {
            // Reconstruct HANDLE from usize
            let handle = HANDLE(handle_ptr as *mut std::ffi::c_void);
            let mut buffer = AlignedBuffer::new(chunk_size + SECTOR_SIZE);

            for chunk in chunks {
                // Read chunk with sector alignment
                let read_size = chunk.record_count * u64::from(record_size_copy);
                let aligned_offset = (chunk.disk_offset / SECTOR_SIZE as u64) * SECTOR_SIZE as u64;
                let offset_adjustment = (chunk.disk_offset - aligned_offset) as usize;
                let aligned_size = ((read_size as usize + offset_adjustment + SECTOR_SIZE - 1)
                    / SECTOR_SIZE)
                    * SECTOR_SIZE;

                // Resize buffer if needed
                if buffer.len() < aligned_size {
                    buffer = AlignedBuffer::new(aligned_size);
                }

                // Seek to position
                let mut new_pos: i64 = 0;
                let seek_result = unsafe {
                    SetFilePointerEx(
                        handle,
                        aligned_offset as i64,
                        Some(&mut new_pos),
                        FILE_BEGIN,
                    )
                };

                if seek_result.is_err() {
                    return Err(MftError::Io(std::io::Error::last_os_error()));
                }

                // Read data
                let mut bytes_read: u32 = 0;
                let read_result = unsafe {
                    ReadFile(
                        handle,
                        Some(&mut buffer.as_mut_slice()[..aligned_size]),
                        Some(&mut bytes_read),
                        None,
                    )
                };

                if read_result.is_err() {
                    return Err(MftError::Io(std::io::Error::last_os_error()));
                }

                // Extract the actual data (skip alignment padding)
                let actual_size = read_size as usize;
                let data =
                    buffer.as_slice()[offset_adjustment..offset_adjustment + actual_size].to_vec();

                // Send to writer (blocks if channel is full - double-buffering)
                if tx.send(data).is_err() {
                    break; // Writer closed, stop reading
                }
            }

            Ok(())
        });

        // Writer loop - receive chunks and write them
        let mut chunks_written = 0;
        for data in rx {
            writer.write_chunk(&data)?;
            chunks_written += 1;

            if chunks_written % 100 == 0 {
                debug!(
                    "Streaming save progress: {}/{} chunks",
                    chunks_written, total_chunks
                );
            }
        }

        // Wait for reader thread to finish
        match reader_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(MftError::Io(std::io::Error::other(
                    "Reader thread panicked",
                )));
            }
        }

        // Finish writing and get final header
        let header = writer.finish()?;

        info!(
            "Streaming save complete: {} records, {} bytes",
            header.record_count, header.original_size
        );

        Ok(header)
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
        // Note: load_raw_to_dataframe doesn't have access to expand_links setting,
        // so we default to true (C++ parity)
        let parsed_columns = merger.merge_into_columns(true);

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
        let mut is_integrity_stream_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_no_scrub_data_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_pinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_unpinned_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut is_virtual_vec: Vec<bool> = Vec::with_capacity(capacity);
        let mut flags_vec: Vec<u32> = Vec::with_capacity(capacity);
        let mut stream_name_vec: Vec<String> = Vec::with_capacity(capacity);

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
            stream_name_vec.push(String::new()); // No expansion, use empty stream name
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
            is_integrity_stream_vec.push(parsed.std_info.is_integrity_stream);
            is_no_scrub_data_vec.push(parsed.std_info.is_no_scrub_data);
            is_pinned_vec.push(parsed.std_info.is_pinned);
            is_unpinned_vec.push(parsed.std_info.is_unpinned);
            is_virtual_vec.push(parsed.std_info.is_virtual);
            flags_vec.push(parsed.std_info.to_raw_flags());
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
            stream_name_vec,
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
            is_integrity_stream_vec,
            is_no_scrub_data_vec,
            is_pinned_vec,
            is_unpinned_vec,
            is_virtual_vec,
            flags_vec,
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

    // =========================================================================
    // Lean Index Methods (Optimized Path)
    // =========================================================================

    /// Read MFTs from all drives concurrently into lean `MftIndex` structures.
    ///
    /// This is the optimized path that uses `SlidingIocpInline` with parallel
    /// parsing for maximum performance. Returns a vector of `MftIndex` objects,
    /// one per drive.
    ///
    /// If some drives fail, the successful ones are still returned.
    /// Only fails if ALL drives fail.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index(&self) -> Result<Vec<crate::index::MftIndex>> {
        self.read_all_index_internal(None::<fn(char, MftProgress)>)
            .await
    }

    /// Read MFTs from all drives into lean index (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_index(&self) -> Result<Vec<crate::index::MftIndex>> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with progress callbacks into lean index.
    ///
    /// The callback receives `(drive_letter, progress)` for each drive.
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index_with_progress<F>(
        &self,
        callback: F,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        self.read_all_index_internal(Some(callback)).await
    }

    /// Read MFTs with progress into lean index (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_index_with_progress<F>(
        &self,
        _callback: F,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFTs from all drives with cache support.
    ///
    /// For each drive:
    /// - If cache is fresh (within TTL), use cached index
    /// - If cache is stale or missing, read from disk and update cache
    ///
    /// This provides the best of both worlds: fast startup when cache is valid,
    /// and automatic refresh when needed.
    ///
    /// # Arguments
    ///
    /// * `ttl_seconds` - Time-to-live for cache entries (use
    ///   `INDEX_TTL_SECONDS` for default)
    ///
    /// # Errors
    ///
    /// Returns an error if all drives fail to read.
    #[cfg(windows)]
    pub async fn read_all_index_cached(
        &self,
        ttl_seconds: u64,
    ) -> Result<Vec<crate::index::MftIndex>> {
        use tokio::task::JoinSet;
        use tracing::info;

        use crate::cache::{CacheStatus, check_cache_status};

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            let ttl = ttl_seconds;

            join_set.spawn(async move {
                // Check cache first
                let cache_result = check_cache_status(drive, ttl);

                match cache_result {
                    CacheStatus::Fresh {
                        index,
                        header: _,
                        age_seconds,
                    } => {
                        info!(
                            drive = %drive,
                            age_seconds,
                            records = index.len(),
                            "📦 Cache HIT - using cached index"
                        );
                        Ok(index)
                    }
                    CacheStatus::Stale { age_seconds } => {
                        info!(
                            drive = %drive,
                            age_seconds = ?age_seconds,
                            "🔄 Cache STALE - rebuilding index"
                        );
                        Self::read_and_cache_single_drive(drive).await
                    }
                    CacheStatus::Missing => {
                        info!(drive = %drive, "🆕 Cache MISS - building index");
                        Self::read_and_cache_single_drive(drive).await
                    }
                }
            });
        }

        // Collect results
        let mut indices: Vec<crate::index::MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => {
                    indices.push(index);
                }
                Ok(Err(e)) => {
                    errors.push(('?', e));
                }
                Err(join_err) => {
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }
        }

        // If no indices were collected, return the first error
        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        Ok(indices)
    }

    /// Read MFTs with cache support (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[allow(clippy::unused_async)]
    pub async fn read_all_index_cached(
        &self,
        _ttl_seconds: u64,
    ) -> Result<Vec<crate::index::MftIndex>> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal implementation for concurrent lean index reading.
    #[cfg(windows)]
    async fn read_all_index_internal<F>(
        &self,
        callback: Option<F>,
    ) -> Result<Vec<crate::index::MftIndex>>
    where
        F: Fn(char, MftProgress) + Send + Sync + Clone + 'static,
    {
        use std::sync::Arc;

        use tokio::task::JoinSet;

        if self.drives.is_empty() {
            return Err(MftError::InvalidInput("No drives specified".into()));
        }

        // Wrap callback in Arc for sharing across tasks
        let callback = callback.map(Arc::new);

        // Spawn a task for each drive
        let mut join_set = JoinSet::new();

        for &drive in &self.drives {
            let cb = callback.clone();

            join_set.spawn(async move { Self::read_single_drive_index(drive, cb).await });
        }

        // Collect results
        let mut indices: Vec<crate::index::MftIndex> = Vec::new();
        let mut errors: Vec<(char, MftError)> = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(index)) => {
                    indices.push(index);
                }
                Ok(Err(e)) => {
                    errors.push(('?', e));
                }
                Err(join_err) => {
                    errors.push((
                        '?',
                        MftError::InvalidInput(format!("Task failed: {join_err}")),
                    ));
                }
            }
        }

        // If no indices were collected, return the first error
        if indices.is_empty() {
            return Err(errors
                .into_iter()
                .next()
                .map(|(_, e)| e)
                .unwrap_or(MftError::InvalidInput("No drives could be read".into())));
        }

        Ok(indices)
    }

    /// Read a single drive into lean index with optional progress callback.
    #[cfg(windows)]
    async fn read_single_drive_index<F>(
        drive: char,
        callback: Option<Arc<F>>,
    ) -> Result<crate::index::MftIndex>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let reader = MftReader::open(drive).await?;

                if let Some(cb) = callback {
                    reader
                        .read_index_with_progress(move |progress| {
                            cb(drive, progress);
                        })
                        .await
                } else {
                    reader.read_all_index().await
                }
            })
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Read a single drive and save to cache.
    #[cfg(windows)]
    async fn read_and_cache_single_drive(drive: char) -> Result<crate::index::MftIndex> {
        use tracing::info;

        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::query_usn_journal;

        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let reader = MftReader::open(drive).await?;
                let index = reader.read_all_index().await?;

                // Get volume info for caching
                let handle = VolumeHandle::open(drive)?;
                let volume_data = handle.volume_data();
                let volume_serial = volume_data.volume_serial_number;

                let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
                    Ok(info) => (info.journal_id, info.next_usn),
                    Err(_) => (0, 0),
                };

                // Save to cache
                if let Err(e) =
                    save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn)
                {
                    // Log but don't fail - caching is optional
                    info!(drive = %drive, error = %e, "⚠️ Failed to save to cache");
                } else {
                    info!(drive = %drive, records = index.len(), "💾 Saved to cache");
                }

                Ok(index)
            })
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
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

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_multi_drive_index_platform_not_supported() {
        let reader = MultiDriveMftReader::new(vec!['C', 'D']);
        let result = reader.read_all_index().await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }

    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_multi_drive_index_cached_platform_not_supported() {
        let reader = MultiDriveMftReader::new(vec!['C', 'D']);
        let result = reader.read_all_index_cached(3600).await;
        assert!(matches!(result, Err(MftError::PlatformNotSupported)));
    }

    // =========================================================================
    // Tests for MftReader optimal defaults
    // =========================================================================

    /// Test that MftReader stores None for concurrency/io_size by default,
    /// allowing the I/O layer to use optimal settings based on drive type.
    #[tokio::test]
    #[cfg(windows)]
    async fn test_mft_reader_uses_none_defaults() {
        // This test requires admin privileges, so we check if we can open
        let reader = match MftReader::open('C').await {
            Ok(r) => r,
            Err(MftError::InsufficientPrivileges) => {
                // Skip test if not running as admin
                return;
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        };

        // Verify that concurrency and io_size are None (will use optimal defaults)
        assert!(
            reader.concurrency.is_none(),
            "MftReader should default to None for concurrency"
        );
        assert!(
            reader.io_size.is_none(),
            "MftReader should default to None for io_size"
        );
        assert!(
            reader.parallel_parse.is_none(),
            "MftReader should default to None for parallel_parse"
        );
        assert!(
            reader.parse_workers.is_none(),
            "MftReader should default to None for parse_workers"
        );
    }

    /// Test that MftReader builder methods correctly set values
    #[tokio::test]
    #[cfg(windows)]
    async fn test_mft_reader_builder_overrides() {
        let reader = match MftReader::open('C').await {
            Ok(r) => r,
            Err(MftError::InsufficientPrivileges) => return,
            Err(e) => panic!("Unexpected error: {:?}", e),
        };

        // Apply builder methods
        let reader = reader
            .with_concurrency(16)
            .with_io_size(2 * 1024 * 1024)
            .with_parallel_parse(true)
            .with_parse_workers(4);

        // Verify values are set
        assert_eq!(reader.concurrency, Some(16));
        assert_eq!(reader.io_size, Some(2 * 1024 * 1024));
        assert_eq!(reader.parallel_parse, Some(true));
        assert_eq!(reader.parse_workers, Some(4));
    }

    /// Test that MftReadMode::Auto is the default
    #[tokio::test]
    #[cfg(windows)]
    async fn test_mft_reader_default_mode_is_auto() {
        let reader = match MftReader::open('C').await {
            Ok(r) => r,
            Err(MftError::InsufficientPrivileges) => return,
            Err(e) => panic!("Unexpected error: {:?}", e),
        };

        assert_eq!(
            reader.mode,
            MftReadMode::Auto,
            "MftReader should default to Auto mode"
        );
    }

    /// Test that all boolean defaults are set for optimal performance
    #[tokio::test]
    #[cfg(windows)]
    async fn test_mft_reader_boolean_defaults() {
        let reader = match MftReader::open('C').await {
            Ok(r) => r,
            Err(MftError::InsufficientPrivileges) => return,
            Err(e) => panic!("Unexpected error: {:?}", e),
        };

        // These defaults are set for optimal performance and C++ parity
        assert!(reader.use_bitmap, "use_bitmap should default to true");
        assert!(reader.expand_links, "expand_links should default to true");
        assert!(
            reader.add_placeholders,
            "add_placeholders should default to true"
        );
        assert!(
            reader.merge_extensions,
            "merge_extensions should default to true"
        );
    }
}
