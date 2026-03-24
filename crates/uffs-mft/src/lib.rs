//! # uffs-mft: NTFS Master File Table Reading Library
//!
//! This crate provides high-performance direct MFT reading capabilities,
//! outputting data as Polars `DataFrame`s for efficient querying.
//!
//! ## Features
//!
//! - **Direct MFT Access**: Bypasses Windows file enumeration APIs for speed
//! - **Async I/O**: Uses tokio for high-throughput disk reading
//! - **Polars Integration**: Returns `DataFrame`s for powerful data
//!   manipulation
//! - **Parquet Persistence**: Save/load indexes in compressed Parquet format
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use uffs_mft::MftReader;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Read MFT from C: drive (requires admin privileges)
//!     let df = MftReader::open('C')?.read_all()?;
//!
//!     println!("Found {} files", df.height());
//!
//!     // Save for later use
//!     MftReader::save_parquet(&df, "c_drive.parquet")?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## `DataFrame` Schema
//!
//! The returned `DataFrame` has the following columns:
//!
//! | Column       | Type           | Description                    |
//! |--------------|----------------|--------------------------------|
//! | `frs`        | `UInt64`       | File Record Segment number     |
//! | `parent_frs` | `UInt64`       | Parent directory FRS           |
//! | `name`       | `String`       | File/directory name            |
//! | `size`       | `UInt64`       | File size in bytes             |
//! | `created`    | `Datetime[μs]` | Creation timestamp             |
//! | `modified`   | `Datetime[μs]` | Modification timestamp         |
//! | `accessed`   | `Datetime[μs]` | Access timestamp               |
//! | `flags`      | `UInt16`       | Bit-packed attributes          |

#![warn(clippy::all, clippy::pedantic)]
#![expect(
    clippy::module_name_repetitions,
    reason = "re-exports use crate-prefixed names for clarity"
)]

extern crate alloc;

// ============================================================================
// Suppress unused crate warnings
// ============================================================================
// These dependencies are used by the uffs_mft binary (src/main.rs), not the
// library. Cargo doesn't support per-binary dependencies, so we suppress the
// warnings here. The binary uses these for CLI, logging, and async runtime.
// Platform-specific dependencies (used on Windows only)
#[cfg(not(windows))]
use bitflags as _;
// Dev-dependencies (used in benchmarks and tests only)
#[cfg(test)]
use criterion as _;
#[cfg(test)]
use proptest as _;
#[cfg(test)]
use rand as _;
#[cfg(test)]
use rand_chacha as _;
// Pipelining dependencies (used in io.rs PipelinedMftReader on Windows)
#[cfg(not(windows))]
use rayon as _;
// FxHash for fast hashing (used in io.rs on Windows)
#[cfg(not(windows))]
use rustc_hash as _;
#[cfg(not(windows))]
use thiserror as _;
#[cfg(not(windows))]
use uffs_polars as _;
#[cfg(windows)]
use windows as _;
// Binary dependencies (used by src/main.rs)
use {
    anyhow as _, chrono as _, clap as _, dirs_next as _, hostname as _, indicatif as _,
    smallvec as _, tokio as _, tracing as _, tracing_appender as _, tracing_subscriber as _,
};

// ============================================================================
// Module declarations
// ============================================================================

pub mod error;
pub mod flags;
pub mod index;
pub mod raw;
pub mod raw_iocp;
pub mod tree_metrics;

// Cross-platform modules (NTFS structures and parsing)
pub mod ntfs; // NTFS structure definitions - cross-platform
pub mod parse; // MFT record parsing - cross-platform

// I/O operations module
// Available on all platforms for offline MFT processing (chaos mode, testing)
// Live MFT access (via HANDLE) is still Windows-only and gated per-function
pub mod io;

// Platform module needed by io module
// Available on all platforms (with Windows-specific HANDLE types cfg-gated
// internally)
pub mod platform;

pub mod usn;

pub mod cache;

mod reader;

// ============================================================================
// Public API re-exports
// ============================================================================

// Re-export cache types
#[cfg(windows)]
pub use cache::load_or_build_dataframe_cached;
pub use cache::{
    CacheStatus, INDEX_TTL_SECONDS, MultiDriveCacheStatus, cache_age_seconds, cache_dir,
    cache_file_path, check_cache_status, check_multi_drive_cache, cleanup_expired_cache,
    is_cache_fresh, load_cached_index, remove_all_cached_indices, remove_cached_index,
    save_to_cache,
};
pub use error::{MftError, Result};
pub use flags::FileFlags;
// Re-export lean index types
pub use index::{
    ChildInfo, FileRecord, IndexBuildTiming, IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex,
    NO_ENTRY, ROOT_FRS, SizeInfo, StandardInfo, UsnApplyStats, frs_to_usize, len_to_u16,
    len_to_u32,
};
// Re-export I/O types for advanced usage
#[cfg(windows)]
pub use io::{
    AlignedBuffer, BatchMftReader, ExtensionAttributes, MftExtentMap, MftRecordMerger,
    MftRecordReader, ParallelMftReader, ParseResult, ParsedColumns, ParsedRecord,
    PipelinedMftReader, PrefetchMftReader, ReadChunk, ReadParseTiming, StreamingMftReader,
    apply_fixup, generate_read_chunks, parse_record_full, parse_record_zero_alloc,
};
// Re-export NTFS constants and types (pure Rust data structures, cross-platform)
pub use ntfs::SECTOR_SIZE;
pub use ntfs::{
    AttributeIterator, AttributeListEntry, AttributeRecordHeader, AttributeRef, AttributeType,
    DataRun, ExtendedStandardInfo, FileNameAttribute, FileRecordSegmentHeader, IndexHeader,
    IndexRoot, NameInfo, NonResidentAttributeData, NtfsBootSector, ReparseMountPointBuffer,
    ReparsePointHeader, ReparseTag, ResidentAttributeData, StandardInformation, StreamInfo,
    apply_usa_fixup, extract_data_runs_from_attribute, fixup_file_record, parse_data_runs,
};
// Re-export platform types
// Core types (DriveType, MftBitmap, MftExtent) are pure data — available on all platforms
// Windows-specific types and functions (VolumeHandle, detect_ntfs_drives, etc.) only on
// Windows
pub use platform::{DriveType, MftBitmap, MftExtent};
#[cfg(windows)]
pub use platform::{
    NtfsVolumeData, VolumeHandle, detect_drive_type, detect_ntfs_drives, infer_drive_from_path,
    is_elevated, is_volume_read_only,
};
pub use raw::{
    LoadRawOptions, RawMftData, RawMftHeader, SaveRawOptions, load_raw_mft, load_raw_mft_header,
    save_raw_mft,
};
pub use raw_iocp::{
    CapturedChunk, IocpCaptureData, IocpCaptureHeader, IocpCaptureOptions, IocpCaptureWriter,
    is_iocp_capture, load_iocp_capture, load_iocp_to_index,
};
pub use reader::{
    BenchmarkResult, DriveCharacteristics, DriveReadResult, MftProgress, MftReadMode, MftReader,
    MftStats, MultiDriveMftReader, PhaseTimings,
};
// Re-export Polars types for convenience
pub use uffs_polars::{DataFrame, IntoLazy, LazyFrame, col, lit};
// Re-export USN Journal types
pub use usn::{
    ChangeType, FileChange, UsnJournalInfo, UsnRecord, aggregate_changes, query_usn_journal,
    read_usn_journal, reason,
};

// ============================================================================
// Shared utility functions
// ============================================================================

/// Formats a number with comma separators for readability.
///
/// Examples: `1234567` → `"1,234,567"`, `1000` → `"1,000"`
#[must_use]
pub fn format_number_commas(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, ch) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Formats a byte count in human-readable form based on magnitude.
///
/// - < 1 KB: `1234 B`
/// - < 1 MB: `123.45 KB`
/// - < 1 GB: `123.45 MB`
/// - < 1 TB: `123.45 GB`
/// - >= 1 TB: `123.45 TB`
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "precision loss acceptable for display"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "floating-point arithmetic required for human-readable byte formatting"
)]
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes:>4} B")
    } else if bytes < 1024 * 1024 {
        format!("{:>7.2} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:>7.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes < 1024 * 1024 * 1024 * 1024 {
        format!("{:>7.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!(
            "{:>7.2} TB",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
        )
    }
}

/// Formats a Unix-microsecond timestamp as `YYYY-MM-DD HH:MM:SS`.
///
/// Returns `"—"` for zero/invalid timestamps.
///
/// Uses Howard Hinnant's civil calendar algorithm (same as the CLI's
/// `append_datetime`). No external crate dependency.
#[must_use]
#[expect(
    clippy::cast_sign_loss,
    reason = "rem_euclid always returns non-negative value"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "day_secs and doe are mathematically bounded within u32 range"
)]
pub fn format_timestamp(unix_micros: i64) -> String {
    if unix_micros == 0 {
        return "—".to_owned();
    }
    let adjusted_secs = unix_micros.div_euclid(1_000_000);

    // Civil time decomposition (no leap seconds — matches chrono behavior).
    let day_secs = adjusted_secs.rem_euclid(86_400) as u32;
    let days = adjusted_secs.div_euclid(86_400) + 719_468; // shift to 0000-03-01 epoch

    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year_offset = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_proxy = (5 * doy + 2) / 153;
    let day = doy - (153 * month_proxy + 2) / 5 + 1;
    let month = if month_proxy < 10 {
        month_proxy + 3
    } else {
        month_proxy - 9
    };
    let year = if month <= 2 {
        year_offset + 1
    } else {
        year_offset
    };

    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Formats a boolean as a filled or hollow circle glyph.
///
/// - `true` → `"●"` (full moon / filled circle)
/// - `false` → `"○"` (hollow moon / empty circle)
///
/// Intended for NTFS boolean attribute columns (Read-only, Hidden, etc.)
/// where a compact visual indicator is clearer than `1` / `0`.
#[must_use]
pub const fn format_bool(value: bool) -> &'static str {
    if value { "●" } else { "○" }
}

/// Formats a duration intelligently based on magnitude.
///
/// - Days+: `2d 3h 5m 10s`
/// - Hours+: `3h 5m 10s`
/// - Minutes+: `5 m 10 s`
/// - Seconds+: `10 s 500 ms`
/// - Milliseconds+: `500 ms 250 μs`
/// - Sub-ms: `250 μs 100 ns`
#[must_use]
pub fn format_duration(duration: core::time::Duration) -> String {
    let total_seconds = duration.as_secs();
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3600) % 24;
    let days = total_seconds / 86400;
    let milliseconds = duration.subsec_millis();
    let microseconds = duration.subsec_micros() % 1_000;
    let nanoseconds = duration.subsec_nanos() % 1_000;

    if days > 0 {
        format!("{days:>2}d {hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if hours > 0 {
        format!("{hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if minutes > 0 {
        format!("{minutes:>3} m  {seconds:>3} s ")
    } else if seconds > 0 {
        format!("{seconds:>3} s  {milliseconds:>3} ms")
    } else if milliseconds > 0 {
        format!("{milliseconds:>3} ms {microseconds:>3} μs")
    } else if microseconds > 0 {
        format!("{microseconds:>3} μs {nanoseconds:>3} ns")
    } else {
        format!("{nanoseconds:>3} ns")
    }
}
