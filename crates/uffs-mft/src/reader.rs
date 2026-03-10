//! MFT Reader implementation.
//!
//! This module provides the main entry point for reading NTFS MFT data.
//! Exception: This module exceeds 800 lines because reader configuration,
//! orchestration, and benchmarks still share one audited surface; a split is
//! deferred outside Wave 3C.

#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use tracing::{debug, info, trace, warn};
use uffs_polars::DataFrame;

use crate::error::{MftError, Result};
#[cfg(windows)]
use crate::platform::VolumeHandle;

mod benchmark;
mod persistence;
mod read_mode;
mod stats;

pub use self::benchmark::{BenchmarkResult, DriveCharacteristics, PhaseTimings};
#[cfg(windows)]
use self::benchmark::{
    build_benchmark_result, build_drive_characteristics, estimate_combined_phase_timings,
};
pub use self::read_mode::MftReadMode;
#[cfg(windows)]
use self::read_mode::{dataframe_effective_mode, index_effective_mode};
pub use self::stats::{MftProgress, MftStats};

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
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let reader = MftReader::open('C')?;
///     let df = reader.read_all()?;
///     println!("Found {} files", df.height());
///     Ok(())
/// }
/// ```
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "builder pattern with boolean options"
)]
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
    /// - `true` (default): Each hard link becomes a separate row, matching the
    ///   legacy output behavior and user expectations (what they see in
    ///   Explorer).
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
    /// The legacy implementation resolved placeholders lazily instead of adding
    /// them upfront.
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
    /// Whether to include forensic records (deleted, corrupt, extension).
    ///
    /// - `false` (default): Normal mode - skip deleted/corrupt, merge
    ///   extensions.
    /// - `true`: Forensic mode - include all records with `is_deleted`,
    ///   `is_corrupt`, `is_extension`, `base_frs` columns.
    ///
    /// WARNING: May significantly increase output size (10-50% more rows).
    forensic: bool,
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
    pub fn open(volume: char) -> Result<Self> {
        let handle = VolumeHandle::open(volume)?;

        Ok(Self {
            volume: volume.to_ascii_uppercase(),
            handle,
            mode: MftReadMode::Auto,
            // Enable extension merging by default for baseline-compatible output.
            // Extension records contain additional attributes for files with
            // many hard links or alternate data streams. Without merging,
            // ~1% of files may have incomplete attribute information.
            // The performance impact is ~10-15% slower, but correctness is
            // more important for file search accuracy.
            merge_extensions: true,
            use_bitmap: true,       // Use bitmap optimization by default
            expand_links: true,     // Expand hard links by default for baseline-compatible output
            add_placeholders: true, // Add placeholders by default for path resolution
            concurrency: None,      // Use default (2 for HDD)
            io_size: None,          // Use default (1MB)
            parallel_parse: None,   // Auto-detect based on drive type
            parse_workers: None,    // Use num_cpus
            forensic: false,        // Normal mode by default
        })
    }

    /// Open a volume for MFT reading (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn open(_volume: char) -> Result<Self> {
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
    /// output. This matches the legacy output behavior and user expectations -
    /// if a file has 3 hard links, users see 3 entries in Explorer, so they
    /// expect 3 entries in search results.
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
    /// The legacy implementation resolved placeholders lazily instead of adding
    /// them upfront. Disabling this matches the legacy output behavior and
    /// saves ~15% of CPU time.
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

    /// Enables forensic mode for this reader.
    ///
    /// Forensic mode includes deleted, corrupt, and extension records in the
    /// output, adding `is_deleted`, `is_corrupt`, `is_extension`, `base_frs`
    /// columns.
    ///
    /// # Arguments
    ///
    /// * `forensic` - If `true`, enable forensic mode. If `false` (default),
    ///   use normal mode.
    ///
    /// # Warning
    ///
    /// Forensic mode may significantly increase output size (10-50% more rows).
    #[must_use]
    pub const fn with_forensic(mut self, forensic: bool) -> Self {
        self.forensic = forensic;
        self
    }

    /// Returns whether forensic mode is enabled.
    #[must_use]
    pub const fn forensic(&self) -> bool {
        self.forensic
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
    pub fn read_all(&self) -> Result<DataFrame> {
        self.read_mft_internal(None::<fn(MftProgress)>)
    }

    /// Read the entire MFT (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_all(&self) -> Result<DataFrame> {
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
    pub fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
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
    pub fn read_with_progress<F>(&self, _callback: F) -> Result<DataFrame>
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
    /// # Note
    ///
    /// This function uses `spawn_blocking` internally to run MFT reading on a
    /// dedicated blocking thread. This avoids potential nested tokio runtime
    /// issues that can occur when dependencies (like polars) try to create
    /// their own runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_all_index(&self) -> Result<crate::index::MftIndex> {
        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_all_index ENTER");
        trace!(volume = %self.volume, "read_all_index: ENTER");
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;

        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        let result = tokio::task::spawn_blocking(move || {
            trace!(volume = %volume, "read_all_index: INSIDE spawn_blocking");
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = MftReader {
                volume,
                handle,
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            let idx = reader.read_mft_index_internal(None::<fn(MftProgress)>);
            trace!(volume = %volume, "read_all_index: read_mft_index_internal done");
            idx
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?;
        trace!(volume = %volume, "read_all_index: EXIT");
        tracing::debug!(volume = %volume, "[TRIP] reader::read_all_index EXIT");
        result
    }

    /// Synchronous version of `read_all_index` for use in blocking contexts.
    ///
    /// This is the same as `read_all_index` but without the async wrapper,
    /// for use with `spawn_blocking` or other blocking contexts.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_all_index_sync(&self) -> Result<crate::index::MftIndex> {
        self.read_mft_index_internal(None::<fn(MftProgress)>)
    }

    /// Synchronous version of `read_all_index` (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub const fn read_all_index_sync(&self) -> Result<crate::index::MftIndex> {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT into lean index with detailed timing breakdown.
    ///
    /// This is the benchmarking version of `read_all_index()` that returns
    /// detailed timing for each phase, including tree metrics computation.
    ///
    /// # Returns
    ///
    /// A tuple of (MftIndex, BenchmarkResult) with the index and timing
    /// breakdown.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_all_index_with_timing(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;
        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        let result = tokio::task::spawn_blocking(move || {
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = MftReader {
                volume,
                handle,
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            reader.read_mft_index_with_timing_internal()
        })
        .await
        .map_err(|e| MftError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

        Ok(result)
    }

    /// Read MFT into lean index with timing (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_all_index_with_timing(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Synchronous version of `read_index_with_progress` for use in blocking
    /// contexts.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub fn read_index_with_progress_sync<F>(&self, callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        self.read_mft_index_internal(Some(callback))
    }

    /// Synchronous version of `read_index_with_progress` (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    pub fn read_index_with_progress_sync<F>(&self, _callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT into lean index with progress callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called periodically with progress updates
    ///
    /// # Note
    ///
    /// This function uses `spawn_blocking` internally to run MFT reading on a
    /// dedicated blocking thread. This avoids potential nested tokio runtime
    /// issues.
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_index_with_progress<F>(&self, callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        // Capture configuration to recreate reader in blocking thread
        let volume = self.volume;
        let mode = self.mode;
        let merge_extensions = self.merge_extensions;
        let use_bitmap = self.use_bitmap;
        let expand_links = self.expand_links;
        let add_placeholders = self.add_placeholders;
        let concurrency = self.concurrency;
        let io_size = self.io_size;
        let parallel_parse = self.parallel_parse;
        let parse_workers = self.parse_workers;
        let forensic = self.forensic;

        tokio::task::spawn_blocking(move || {
            // Create a new reader in the blocking thread
            let handle = VolumeHandle::open(volume)?;
            let reader = MftReader {
                volume,
                handle,
                mode,
                merge_extensions,
                use_bitmap,
                expand_links,
                add_placeholders,
                concurrency,
                io_size,
                parallel_parse,
                parse_workers,
                forensic,
            };
            reader.read_mft_index_internal(Some(callback))
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Read MFT into lean index with progress (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_index_with_progress<F>(&self, _callback: F) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress) + Send + 'static,
    {
        Err(MftError::PlatformNotSupported)
    }

    /// Read MFT into lean `MftIndex` with automatic caching.
    ///
    /// This is the **recommended primary method** for CLI usage. It:
    /// 1. Checks if a fresh cache exists (within TTL)
    /// 2. If fresh, loads from cache and applies USN Journal updates
    /// 3. If stale/missing, reads MFT fresh and saves to cache
    ///
    /// Use `read_all_index()` directly only when you need to bypass caching.
    ///
    /// # Arguments
    ///
    /// * `ttl_seconds` - Cache TTL in seconds (use `INDEX_TTL_SECONDS` for
    ///   default)
    ///
    /// # Errors
    ///
    /// Returns an error if MFT reading fails.
    #[cfg(windows)]
    pub async fn read_index_cached(&self, ttl_seconds: u64) -> Result<crate::index::MftIndex> {
        use tracing::{debug, info, warn};

        use crate::cache::{CacheStatus, check_cache_status, save_to_cache};
        use crate::platform::VolumeHandle;
        use crate::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

        let drive = self.volume;
        tracing::debug!(drive = %drive, ttl_seconds, "[TRIP] reader::read_index_cached ENTER");

        // Check cache status
        match check_cache_status(drive, ttl_seconds) {
            CacheStatus::Fresh {
                mut index,
                header,
                age_seconds,
            } => {
                tracing::debug!(drive = %drive, age_seconds, "[TRIP] reader::read_index_cached -> CACHE_HIT path");
                info!(
                    drive = %drive,
                    age_seconds,
                    records = index.len(),
                    "📦 Cache HIT - checking for USN updates"
                );

                // Apply USN Journal updates to bring index up to date
                let current_info = match query_usn_journal(drive) {
                    Ok(info) => info,
                    Err(e) => {
                        warn!(
                            drive = %drive,
                            error = %e,
                            "⚠️ USN Journal unavailable - using cached index as-is"
                        );
                        return Ok(index);
                    }
                };

                // Check if journal ID matches (journal may have been recreated)
                if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
                    info!(
                        drive = %drive,
                        cached_journal_id = header.usn_journal_id,
                        current_journal_id = current_info.journal_id,
                        "🔄 USN Journal ID changed - rebuilding index"
                    );
                    return self.read_and_cache_index().await;
                }

                // Check if our checkpoint is still valid
                let start_usn = header.next_usn;
                if start_usn < current_info.first_usn {
                    info!(
                        drive = %drive,
                        cached_usn = start_usn,
                        first_usn = current_info.first_usn,
                        "🔄 USN Journal wrapped - rebuilding index"
                    );
                    return self.read_and_cache_index().await;
                }

                // If already at latest USN, no changes needed
                if start_usn >= current_info.next_usn {
                    debug!(
                        drive = %drive,
                        usn = start_usn,
                        "✅ Index is already up to date"
                    );
                    return Ok(index);
                }

                // Read USN changes since checkpoint
                let (records, next_usn) =
                    match read_usn_journal(drive, current_info.journal_id, start_usn) {
                        Ok(result) => result,
                        Err(e) => {
                            warn!(
                                drive = %drive,
                                error = %e,
                                "⚠️ Failed to read USN Journal - using cached index as-is"
                            );
                            return Ok(index);
                        }
                    };

                if records.is_empty() {
                    debug!(drive = %drive, "✅ No USN changes since last cache");
                    return Ok(index);
                }

                // Aggregate and apply changes
                let changes_map = aggregate_changes(&records);
                let changes: Vec<_> = changes_map.into_values().collect();
                info!(
                    drive = %drive,
                    usn_records = changes.len(),
                    "📝 Applying USN updates to cached index"
                );

                let stats = index.apply_usn_changes(&changes);
                info!(
                    drive = %drive,
                    created = stats.created,
                    modified = stats.modified,
                    deleted = stats.deleted,
                    skipped = stats.skipped,
                    "✅ USN updates applied"
                );

                // Save updated index back to cache
                let handle = VolumeHandle::open(drive)?;
                let volume_serial = handle.volume_data().volume_serial_number;
                if let Err(e) = save_to_cache(
                    &index,
                    drive,
                    volume_serial,
                    current_info.journal_id,
                    next_usn,
                ) {
                    warn!(drive = %drive, error = %e, "⚠️ Failed to update cache");
                }

                Ok(index)
            }
            CacheStatus::Stale { age_seconds } => {
                tracing::debug!(drive = %drive, "[TRIP] reader::read_index_cached -> CACHE_STALE path");
                info!(
                    drive = %drive,
                    age_seconds = ?age_seconds,
                    "🔄 Cache STALE - rebuilding index"
                );
                self.read_and_cache_index().await
            }
            CacheStatus::Missing => {
                tracing::debug!(drive = %drive, "[TRIP] reader::read_index_cached -> CACHE_MISS path");
                info!(drive = %drive, "🆕 Cache MISS - building index");
                self.read_and_cache_index().await
            }
        }
    }

    /// Read MFT with caching (non-Windows stub).
    ///
    /// # Errors
    ///
    /// Always returns `MftError::PlatformNotSupported` on non-Windows
    /// platforms.
    #[cfg(not(windows))]
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
    pub async fn read_index_cached(&self, _ttl_seconds: u64) -> Result<crate::index::MftIndex> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal helper: read MFT fresh and save to cache.
    #[cfg(windows)]
    async fn read_and_cache_index(&self) -> Result<crate::index::MftIndex> {
        use tracing::info;

        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::query_usn_journal;

        let drive = self.volume;
        tracing::debug!(drive = %drive, "[TRIP] reader::read_and_cache_index ENTER");
        let index = self.read_all_index().await?;
        tracing::debug!(drive = %drive, records = index.len(), "[TRIP] reader::read_and_cache_index -> read_all_index done");

        // Get volume info for caching
        let handle = VolumeHandle::open(drive)?;
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
            Ok(info) => (info.journal_id, info.next_usn),
            Err(_) => (0, 0),
        };

        // Save to cache
        if let Err(e) = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn) {
            info!(drive = %drive, error = %e, "⚠️ Failed to save to cache (non-fatal)");
        } else {
            info!(drive = %drive, records = index.len(), "💾 Saved to cache");
        }

        tracing::debug!(drive = %drive, "[TRIP] reader::read_and_cache_index EXIT");
        Ok(index)
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
    pub fn read_with_timing(
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
    pub const fn read_with_timing(
        &self,
        _skip_df_build: bool,
    ) -> Result<(Option<DataFrame>, BenchmarkResult)> {
        Err(MftError::PlatformNotSupported)
    }

    /// Internal MFT reading implementation.
    ///
    /// This implementation uses the high-performance parallel reader with:
    /// 1. Extent-aware reading for fragmented MFTs
    /// 2. Bitmap-based cluster skipping (matching the historical baseline)
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
        // Historical benchmark insight: "read all then parse" is faster than pipelining
        // even on HDD because: no context switching, CPU cache stays hot, no
        // channel overhead, OS can optimize continuous sequential reads better.
        // For read_all() (returns Vec<ParsedRecord>), use SlidingIocp for IOCP-based
        // I/O.
        let effective_mode = dataframe_effective_mode(self.mode, drive_type);

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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
        };

        // Add placeholder records for missing parent directories.
        // This matches the legacy output behavior where `at()` creates placeholder
        // records for any referenced FRS that hasn't been seen yet.
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
                        fn_created: parsed.fn_created,
                        fn_modified: parsed.fn_modified,
                        fn_accessed: parsed.fn_accessed,
                        fn_mft_changed: parsed.fn_mft_changed,
                        source_frs: parsed.frs,
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
                        is_resident: false,
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
        stats.log_summary();

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
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with mode-specific branches cannot be meaningfully split"
    )]
    fn read_mft_index_internal<F>(&self, callback: Option<F>) -> Result<crate::index::MftIndex>
    where
        F: Fn(MftProgress),
    {
        use crate::index::MftIndex;
        use crate::io::{MftExtentMap, ParallelMftReader};
        use crate::platform::detect_drive_type;

        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_mft_index_internal ENTER");
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
        // Historical benchmark insight: "read all then parse" is faster than pipelining
        // even on HDD because: no context switching, CPU cache stays hot, no
        // channel overhead, OS can optimize continuous sequential reads better.
        // For lean index (MftIndex), use SlidingIocpInline for NVMe/SSD - this uses
        // IOCP with multiple reads in flight and inline parsing, matching C++
        // performance.
        let effective_mode = index_effective_mode(self.mode, drive_type);

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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
                {
                    unsafe { windows::Win32::Foundation::CloseHandle(overlapped_handle) }.ok();
                }

                result?
            }
            MftReadMode::SlidingIocpInline => {
                // Sliding window IOCP with inline parsing and direct index building
                // This mode returns MftIndex directly, skipping the intermediate
                // Vec<ParsedRecord>
                let overlapped_handle = self.handle.open_overlapped_handle()?;
                let parallel_reader =
                    ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);

                let result = parallel_reader.read_all_sliding_window_iocp_to_index::<fn(u64, u64)>(
                    overlapped_handle,
                    self.volume,
                    self.concurrency,
                    self.io_size,
                    None,
                );

                // Close the overlapped handle
                #[expect(unsafe_code, reason = "FFI: CloseHandle on valid overlapped handle")]
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

        tracing::debug!(
            volume = %self.volume,
            records_parsed = records_parsed_count,
            "[TRIP] reader::read_mft_index_internal -> I/O+parse done, calling MftIndex::from_parsed_records"
        );
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

        tracing::debug!(
            volume = %self.volume,
            records = index.records.len(),
            index_build_ms = index_elapsed.as_millis(),
            "[TRIP] reader::read_mft_index_internal -> MftIndex::from_parsed_records done"
        );
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

        tracing::debug!(volume = %self.volume, "[TRIP] reader::read_mft_index_internal EXIT");
        Ok(index)
    }

    /// Internal implementation for MFT lean index reading with detailed phase
    /// timing.
    ///
    /// This method measures each phase separately for benchmarking purposes,
    /// including the tree metrics computation phase which corresponds to
    /// C++ "preprocessing" in `--benchmark-index`.
    ///
    /// # Returns
    ///
    /// A tuple of (MftIndex, BenchmarkResult) with detailed timing breakdown.
    #[cfg(windows)]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with per-phase timing cannot be meaningfully split"
    )]
    fn read_mft_index_with_timing_internal(
        &self,
    ) -> Result<(crate::index::MftIndex, BenchmarkResult)> {
        use crate::index::MftIndex;
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
        let bitmap = if self.use_bitmap {
            self.handle.get_mft_bitmap().ok()
        } else {
            None
        };
        let in_use_records = bitmap.as_ref().map(|bm| bm.count_in_use() as u64);

        // Generate chunks to get count
        let chunks = generate_read_chunks(&extent_map, bitmap.as_ref(), chunk_size);
        let chunk_count = chunks.len();

        let open_ms = open_start.elapsed().as_millis() as u64;

        // Build characteristics
        let characteristics = build_drive_characteristics(
            self.volume,
            drive_type,
            mft_size_bytes,
            total_records,
            in_use_records,
            extents.len(),
            record_size,
            chunk_size,
            chunk_count,
        );

        info!(
            volume = %self.volume,
            drive_type = ?drive_type,
            total_records,
            mft_size_mb = mft_size_bytes / (1024 * 1024),
            extents = extents.len(),
            chunks = chunk_count,
            "📊 Benchmark (lean index): MFT characteristics"
        );

        // Phase 2+3: Read + Parse with accurate timing
        let parallel_reader = ParallelMftReader::new_optimized(extent_map, bitmap, drive_type);
        let handle = self.handle.raw_handle();

        // Use the new timing method for accurate phase breakdown
        let (mut parsed_records, read_parse_timing) =
            parallel_reader.read_all_parallel_with_timing(handle, self.merge_extensions)?;

        // Add placeholder records for missing parent directories
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

        let records_parsed = parsed_records.len();

        // Use accurate timing from instrumented reader (not estimates!)
        let read_ms = read_parse_timing.io_ms();
        let parse_ms = read_parse_timing.parse_ms();
        let merge_ms = read_parse_timing.merge_ms();

        info!(
            records_parsed,
            read_ms,
            parse_ms,
            merge_ms,
            wall_ms = read_parse_timing.wall_ms(),
            overlap_ratio = format!("{:.2}", read_parse_timing.overlap_ratio()),
            "📊 Benchmark (lean index): Read + Parse complete (accurate timing)"
        );

        // Phase 4: Build MftIndex with timing breakdown
        let (index, index_timing) =
            MftIndex::from_parsed_records_with_timing(self.volume, parsed_records);

        info!(
            records = index.records.len(),
            names_buffer_kb = index.names.len() / 1024,
            record_insert_ms = index_timing.record_insert_ms,
            extension_index_ms = index_timing.extension_index_ms,
            sort_children_ms = index_timing.sort_children_ms,
            tree_metrics_ms = index_timing.tree_metrics_ms,
            index_total_ms = index_timing.total_ms,
            "📊 Benchmark (lean index): Index build complete"
        );

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
            df_build_ms: 0, // Not applicable for MftIndex path
            index_build_ms: index_timing.index_only_ms(),
            tree_metrics_ms: index_timing.tree_metrics_ms,
            total_ms,
        };

        let result = build_benchmark_result(
            timings,
            characteristics,
            records_parsed,
            throughput_mb_s,
            records_per_sec,
        );

        info!(
            total_ms,
            index_build_ms = index_timing.index_only_ms(),
            tree_metrics_ms = index_timing.tree_metrics_ms,
            throughput_mb_s = format!("{:.1}", throughput_mb_s),
            records_per_sec = format!("{:.0}", records_per_sec),
            "📊 Benchmark (lean index): Complete"
        );

        Ok((index, result))
    }

    /// Internal implementation for MFT reading with detailed phase timing.
    ///
    /// This method measures each phase separately for benchmarking purposes.
    #[cfg(windows)]
    #[expect(
        clippy::too_many_lines,
        reason = "sequential I/O pipeline with per-phase timing cannot be meaningfully split"
    )]
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
        let characteristics = build_drive_characteristics(
            self.volume,
            drive_type,
            mft_size_bytes,
            total_records,
            in_use_records,
            extents.len(),
            record_size,
            chunk_size,
            chunk_count,
        );

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
        // This matches the legacy output behavior where `at()` creates placeholder
        // records for any referenced FRS that hasn't been seen yet.
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
        let (read_ms, parse_ms, merge_ms) =
            estimate_combined_phase_timings(drive_type, read_parse_ms);

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
            index_build_ms: 0,  // Not applicable for DataFrame path
            tree_metrics_ms: 0, // Not applicable for DataFrame path
            total_ms,
        };

        let result = build_benchmark_result(
            timings,
            characteristics,
            records_parsed,
            throughput_mb_s,
            records_per_sec,
        );

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
    #[expect(
        dead_code,
        reason = "kept as fallback for AoS path; superseded by build_dataframe_from_columns"
    )]
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
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per dataframe column in legacy 8-column schema"
    )]
    #[expect(dead_code, reason = "kept as fallback for legacy 8-column schema")]
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

    /// Builds a `DataFrame` with the full baseline-compatible schema (23
    /// columns).
    #[cfg(windows)]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per dataframe column in full 23-column schema"
    )]
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
            // Additional flags for baseline-compatible output
            Series::new("is_integrity_stream".into(), is_integrity_stream_vec).into_column(),
            Series::new("is_no_scrub_data".into(), is_no_scrub_data_vec).into_column(),
            Series::new("is_pinned".into(), is_pinned_vec).into_column(),
            Series::new("is_unpinned".into(), is_unpinned_vec).into_column(),
            Series::new("is_virtual".into(), is_virtual_vec).into_column(),
            // Raw attribute flags (combined value for baseline-compatible output)
            Series::new("flags".into(), flags_vec).into_column(),
        ];

        DataFrame::new_infer_height(columns).map_err(MftError::from)
    }

    /// Builds a `DataFrame` directly from `ParsedColumns` (`SoA` layout).
    ///
    /// This is the optimized path that avoids the AoS→SoA transpose.
    /// The columns are already in the correct format, so we just wrap them
    /// in Polars Series.
    ///
    /// # Platform
    ///
    /// Cross-platform - works on all platforms.
    #[expect(clippy::single_call_fn, reason = "extracted for clarity")]
    fn build_dataframe_from_columns(columns: crate::parse::ParsedColumns) -> Result<DataFrame> {
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
            // Raw attribute flags (combined value for baseline-compatible output)
            Series::new("flags".into(), columns.flags).into_column(),
        ];

        DataFrame::new_infer_height(polars_columns).map_err(MftError::from)
    }

    /// Get the volume letter this reader is attached to.
    #[must_use]
    pub const fn volume(&self) -> char {
        self.volume
    }

    /// Create an empty `DataFrame` with the MFT schema.
    #[expect(dead_code, reason = "utility for tests and potential future use")]
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

    /// Convert parsed records to DataFrame (legacy AoS path).
    ///
    /// NOTE: This function is superseded by `build_dataframe_from_columns`
    /// which uses the SoA path and avoids the AoS→SoA transpose. Kept for
    /// reference.
    #[cfg(windows)]
    #[expect(
        dead_code,
        reason = "kept as reference for legacy AoS path; superseded by build_dataframe_from_columns"
    )]
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
    /// which are not `Send`, and the MFT reading is blocking I/O.
    #[cfg(windows)]
    async fn read_single_drive<F>(drive: char, callback: Option<Arc<F>>) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static,
    {
        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        // This avoids blocking the async runtime and prevents nested runtime panics.
        tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            if let Some(cb) = callback {
                reader.read_with_progress(move |progress| {
                    cb(drive, progress);
                })
            } else {
                reader.read_all()
            }
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
                        header,
                        age_seconds,
                    } => {
                        info!(
                            drive = %drive,
                            age_seconds,
                            records = index.len(),
                            "📦 Cache HIT - applying USN updates"
                        );
                        // Apply USN changes to bring index up to date
                        Self::apply_usn_updates_to_cached_index(drive, index, header).await
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
    #[expect(clippy::unused_async, reason = "async for API parity with windows")]
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
        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        tokio::task::spawn_blocking(move || {
            let reader = MftReader::open(drive)?;

            if let Some(cb) = callback {
                reader.read_index_with_progress_sync(move |progress| {
                    cb(drive, progress);
                })
            } else {
                reader.read_all_index_sync()
            }
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Read a single drive and save to cache.
    #[cfg(windows)]
    async fn read_and_cache_single_drive(drive: char) -> Result<crate::index::MftIndex> {
        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        tokio::task::spawn_blocking(move || Self::read_and_cache_single_drive_sync(drive))
            .await
            .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Synchronous implementation of read_and_cache_single_drive.
    #[cfg(windows)]
    fn read_and_cache_single_drive_sync(drive: char) -> Result<crate::index::MftIndex> {
        use tracing::info;

        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::query_usn_journal;

        let reader = MftReader::open(drive)?;
        let index = reader.read_all_index_sync()?;

        // Get volume info for caching
        let handle = VolumeHandle::open(drive)?;
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        let (usn_journal_id, next_usn) = match query_usn_journal(drive) {
            Ok(info) => (info.journal_id, info.next_usn),
            Err(_) => (0, 0),
        };

        // Save to cache
        if let Err(e) = save_to_cache(&index, drive, volume_serial, usn_journal_id, next_usn) {
            // Log but don't fail - caching is optional
            info!(drive = %drive, error = %e, "⚠️ Failed to save to cache");
        } else {
            info!(drive = %drive, records = index.len(), "💾 Saved to cache");
        }

        Ok(index)
    }

    /// Apply USN Journal updates to a cached index to bring it up to date.
    ///
    /// This reads changes from the USN Journal since the cached checkpoint,
    /// applies them to the index, and saves the updated index back to cache.
    ///
    /// If USN Journal is unavailable or the journal has wrapped, falls back
    /// to a full rebuild.
    #[cfg(windows)]
    async fn apply_usn_updates_to_cached_index(
        drive: char,
        index: crate::index::MftIndex,
        header: crate::index::IndexHeader,
    ) -> Result<crate::index::MftIndex> {
        // Use spawn_blocking to run blocking I/O on a dedicated thread pool.
        tokio::task::spawn_blocking(move || {
            Self::apply_usn_updates_to_cached_index_sync(drive, index, header)
        })
        .await
        .map_err(|e| MftError::InvalidInput(format!("Task join error: {e}")))?
    }

    /// Synchronous implementation of apply_usn_updates_to_cached_index.
    #[cfg(windows)]
    fn apply_usn_updates_to_cached_index_sync(
        drive: char,
        mut index: crate::index::MftIndex,
        header: crate::index::IndexHeader,
    ) -> Result<crate::index::MftIndex> {
        use tracing::{debug, info, warn};

        use crate::cache::save_to_cache;
        use crate::platform::VolumeHandle;
        use crate::usn::{aggregate_changes, query_usn_journal, read_usn_journal};

        // Query current USN Journal state
        let current_info = match query_usn_journal(drive) {
            Ok(info) => info,
            Err(e) => {
                warn!(
                    drive = %drive,
                    error = %e,
                    "⚠️ USN Journal unavailable - using cached index as-is"
                );
                return Ok(index);
            }
        };

        // Check if journal ID matches (journal may have been recreated)
        if header.usn_journal_id != 0 && current_info.journal_id != header.usn_journal_id {
            info!(
                drive = %drive,
                cached_journal_id = header.usn_journal_id,
                current_journal_id = current_info.journal_id,
                "🔄 USN Journal ID changed - rebuilding index"
            );
            // Journal was recreated, need full rebuild
            return Self::read_and_cache_single_drive_sync(drive);
        }

        // Check if our checkpoint is still valid (not before first_usn)
        let start_usn = header.next_usn;
        if start_usn < current_info.first_usn {
            info!(
                drive = %drive,
                cached_usn = start_usn,
                first_usn = current_info.first_usn,
                "🔄 USN Journal wrapped - rebuilding index"
            );
            // Journal wrapped, need full rebuild
            return Self::read_and_cache_single_drive_sync(drive);
        }

        // If we're already at the latest USN, no changes needed
        if start_usn >= current_info.next_usn {
            debug!(
                drive = %drive,
                usn = start_usn,
                "✅ Index is already up to date"
            );
            return Ok(index);
        }

        // Read USN changes since our checkpoint
        let (records, next_usn) = match read_usn_journal(drive, current_info.journal_id, start_usn)
        {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    drive = %drive,
                    error = %e,
                    "⚠️ Failed to read USN Journal - using cached index as-is"
                );
                return Ok(index);
            }
        };

        if records.is_empty() {
            debug!(
                drive = %drive,
                "✅ No USN changes since last cache"
            );
            return Ok(index);
        }

        // Aggregate changes (deduplicate by FRS)
        let changes_map = aggregate_changes(&records);
        let changes: Vec<_> = changes_map.into_values().collect();
        info!(
            drive = %drive,
            usn_records = changes.len(),
            from_usn = start_usn,
            to_usn = next_usn,
            "🔧 Applying USN changes"
        );

        // Apply changes to index
        let stats = index.apply_usn_changes(&changes);
        debug!(
            drive = %drive,
            created = stats.created,
            deleted = stats.deleted,
            modified = stats.modified,
            skipped = stats.skipped,
            "📊 USN changes applied"
        );

        // Recompute tree metrics after structural changes
        debug!(drive = %drive, "🔨 Recomputing tree metrics after USN updates");
        index.compute_tree_metrics();

        // Save updated index to cache with new checkpoint
        let handle = match VolumeHandle::open(drive) {
            Ok(h) => h,
            Err(e) => {
                warn!(
                    drive = %drive,
                    error = %e,
                    "⚠️ Failed to open volume for cache update"
                );
                return Ok(index);
            }
        };
        let volume_data = handle.volume_data();
        let volume_serial = volume_data.volume_serial_number;

        if let Err(e) = save_to_cache(
            &index,
            drive,
            volume_serial,
            current_info.journal_id,
            next_usn,
        ) {
            warn!(
                drive = %drive,
                error = %e,
                "⚠️ Failed to update cache"
            );
        } else {
            debug!(
                drive = %drive,
                next_usn,
                "💾 Cache updated with new USN checkpoint"
            );
        }

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_open_valid_volume() {
        let result = MftReader::open('C');
        // This will fail without admin privileges, but should not panic
        assert!(result.is_ok() || matches!(result, Err(MftError::InsufficientPrivileges)));
    }

    #[test]
    #[cfg(not(windows))]
    fn test_platform_not_supported() {
        let result = MftReader::open('C');
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
    #[test]
    #[cfg(windows)]
    fn test_mft_reader_uses_none_defaults() {
        // This test requires admin privileges, so we check if we can open
        let reader = match MftReader::open('C') {
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
    #[test]
    #[cfg(windows)]
    fn test_mft_reader_builder_overrides() {
        let reader = match MftReader::open('C') {
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
    #[test]
    #[cfg(windows)]
    fn test_mft_reader_default_mode_is_auto() {
        let reader = match MftReader::open('C') {
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
    #[test]
    #[cfg(windows)]
    fn test_mft_reader_boolean_defaults() {
        let reader = match MftReader::open('C') {
            Ok(r) => r,
            Err(MftError::InsufficientPrivileges) => return,
            Err(e) => panic!("Unexpected error: {:?}", e),
        };

        // These defaults are set for optimal performance and baseline-compatible output
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
