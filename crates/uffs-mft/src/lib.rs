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
#[cfg(not(windows))]
use rayon as _;
#[cfg(not(windows))]
use thiserror as _;
#[cfg(not(windows))]
use uffs_polars as _;
use {anyhow as _, clap as _, indicatif as _, tokio as _, tracing as _, tracing_subscriber as _};

// ============================================================================
// Module declarations
// ============================================================================

pub mod error;
pub mod flags;
pub mod raw;

#[cfg(windows)]
pub mod ntfs;

#[cfg(windows)]
pub mod io;

#[cfg(windows)]
pub mod platform;

mod reader;

// ============================================================================
// Public API re-exports
// ============================================================================

pub use error::{MftError, Result};
pub use flags::FileFlags;
// Re-export I/O types for advanced usage
#[cfg(windows)]
pub use io::{
    AlignedBuffer, BatchMftReader, ExtensionAttributes, MftExtentMap, MftRecordMerger,
    MftRecordReader, ParallelMftReader, ParseResult, ParsedRecord, ReadChunk, apply_fixup,
    generate_read_chunks, parse_record_full,
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
    MftBitmap, MftExtent, NtfsVolumeData, VolumeHandle, detect_ntfs_drives, is_elevated,
};
pub use raw::{
    LoadRawOptions, RawMftData, RawMftHeader, SaveRawOptions, load_raw_mft, load_raw_mft_header,
    save_raw_mft,
};
pub use reader::{DriveReadResult, MftProgress, MftReader, MultiDriveMftReader};
// Re-export Polars types for convenience
pub use uffs_polars::{DataFrame, IntoLazy, LazyFrame, col, lit};
