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
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Read MFT from C: drive (requires admin privileges)
//!     let df = MftReader::open('C').await?.read_all().await?;
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
#![allow(clippy::module_name_repetitions)]

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
// Dev-dependencies (used in benchmarks only)
#[cfg(test)]
use criterion as _;
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
    num_cpus as _, smallvec as _, tokio as _, tracing as _, tracing_appender as _,
    tracing_subscriber as _,
};

// ============================================================================
// Module declarations
// ============================================================================

pub mod error;
pub mod flags;
pub mod index;
pub mod raw;

// Cross-platform modules (NTFS structures and parsing)
pub mod ntfs; // NTFS structure definitions - cross-platform
pub mod parse; // MFT record parsing - cross-platform

// Windows-only modules (I/O operations)
#[cfg(windows)]
pub mod io;

#[cfg(windows)]
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
    ChildInfo, FileRecord, IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex, NO_ENTRY, ROOT_FRS,
    SizeInfo, StandardInfo, UsnApplyStats,
};
// Re-export I/O types for advanced usage
#[cfg(windows)]
pub use io::{
    AlignedBuffer, BatchMftReader, ExtensionAttributes, MftExtentMap, MftRecordMerger,
    MftRecordReader, ParallelMftReader, ParseResult, ParsedColumns, ParsedRecord,
    PipelinedMftReader, PrefetchMftReader, ReadChunk, StreamingMftReader, apply_fixup,
    generate_read_chunks, parse_record_full, parse_record_zero_alloc,
};
// Re-export NTFS constants
#[cfg(windows)]
pub use ntfs::SECTOR_SIZE;
#[cfg(windows)]
pub use ntfs::{
    AttributeIterator, AttributeListEntry, AttributeRecordHeader, AttributeRef, AttributeType,
    DataRun, ExtendedStandardInfo, FileNameAttribute, FileRecordSegmentHeader, IndexHeader,
    IndexRoot, NameInfo, NonResidentAttributeData, NtfsBootSector, ReparseMountPointBuffer,
    ReparsePointHeader, ReparseTag, ResidentAttributeData, StandardInformation, StreamInfo,
    apply_usa_fixup, extract_data_runs_from_attribute, fixup_file_record, parse_data_runs,
};
// Re-export platform types
#[cfg(windows)]
pub use platform::{
    DriveType, MftBitmap, MftExtent, NtfsVolumeData, VolumeHandle, detect_drive_type,
    detect_ntfs_drives, infer_drive_from_path, is_elevated, is_volume_read_only,
};
pub use raw::{
    LoadRawOptions, RawMftData, RawMftHeader, SaveRawOptions, load_raw_mft, load_raw_mft_header,
    save_raw_mft,
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
