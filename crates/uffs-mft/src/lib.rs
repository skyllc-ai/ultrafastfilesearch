// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

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
//! fn main() -> Result<(), Box<dyn core::error::Error>> {
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
//!
//! ## API hygiene policy (Phase 3b §3.4 / §3.6 / §3.7)
//!
//! The vast majority of `pub struct` declarations in this crate fall
//! into one of three categories, each with a **uniform decision**:
//!
//! 1. **NTFS on-disk zerocopy types** (`#[repr(C, packed)]` +
//!    `#[derive(FromBytes, Immutable, KnownLayout)]`): `MultiSectorHeader`,
//!    `AttributeRecordHeader`, `ResidentAttributeData`,
//!    `NonResidentAttributeData`, `FileRecordSegmentHeader`, `IndexHeader`,
//!    `IndexRoot`, `StandardInformation`, `FileNameAttribute`,
//!    `ExtendedStandardInfo`, `AttributeListEntry`, `ReparsePointHeader`,
//!    `ReparseMountPointBuffer`, `NtfsBootSector`, IOCP capture headers, etc.
//!    The field layout **is** the NTFS specification (or the IOCP capture file
//!    contract); `pub` fields are non-negotiable.  `#[non_exhaustive]` would
//!    forbid the very `MyHeader { … }` literals that the zerocopy decoder
//!    helpers build by hand.  **Kept exhaustive.**
//!
//! 2. **Index / record DTOs** (`MftIndex`, `FileRecord`, `ChildInfo`,
//!    `LinkInfo`, `SizeInfo`, `StandardInfo`, `IndexStreamInfo`,
//!    `IndexNameRef`, `UsnApplyStats`, `IndexBuildTiming`, `ParseResult`,
//!    `ParsedColumns`, `ParsedRecord`, `ReadChunk`, `RawMftHeader`,
//!    `RawMftData`, etc.):  read-mostly value types consumed by `uffs-core` and
//!    `uffs-daemon`.  Hundreds of struct-literal construction sites;
//!    `#[non_exhaustive]` would require migrating each to a builder for
//!    marginal benefit while the crate is Polars-blocked from publishing.
//!    **Kept exhaustive.**
//!
//! 3. **Configuration option structs** (`LoadRawOptions`, `SaveRawOptions`,
//!    `IocpCaptureOptions`):  `pub` fields are config knobs with
//!    `Default::default()`; struct-literal construction is the natural API.
//!    **Kept exhaustive** for the same migration-cost reason; revisit when the
//!    crate publishes.
//!
//! The few `pub enum` declarations (`AttributeType`, `ReparseTag`,
//! `DriveType`, `FileFlags`, etc.) are **closed type-code enums**
//! defined by the NTFS specification — new variants only appear when
//! Microsoft extends NTFS — and **state-machine / dispatch enums**
//! that consumers exhaustively match.  Either category falls under
//! the playbook §3.6 "keep exhaustive" rule.  **Kept exhaustive.**
//!
//! No `pub trait` declarations live in this crate, so the
//! sealed-trait decision (§3.7) is **N/A**.

#![warn(clippy::all, clippy::pedantic)]
#![expect(
    clippy::module_name_repetitions,
    reason = "re-exports use crate-prefixed names for clarity"
)]
// Windows-only because every `std::io::Error::new` site lives behind
// `#[cfg(windows)]` (IO-reader, bitmap, volume paths).  On non-Windows builds
// the lint never fires, and an unconditional `#[expect]` would itself trip
// `unfulfilled_lint_expectations` under `-D warnings`.
#![cfg_attr(
    windows,
    expect(
        clippy::std_instead_of_core,
        reason = "core::io::Error is not yet stable (feature `core_io`, tracking issue rust-lang/rust#154046). Our IO-reader, bitmap, and volume paths construct std::io::Error::new(ErrorKind::_, msg); swapping to core::io::ErrorKind alone would force a split std/core import at every site. Revisit once core::io::Error stabilizes."
    )
)]

extern crate alloc;

// Dev-dependencies used in tests but not by the library itself.
// Binary dependencies (used by src/main.rs only, or by #[cfg(windows)]
// modules). Listed here to prevent unused_crate_dependencies false positives on
// non-Windows.
use anyhow as _;
// ============================================================================
// Suppress unused crate warnings
// ============================================================================
// These dependencies are used by the uffs_mft binary (src/main.rs), not the
// library. Cargo doesn't support per-binary dependencies, so we suppress the
// warnings here. The binary uses these for CLI, logging, and async runtime.
// Platform-specific dependencies (used on Windows only)
#[cfg(not(windows))]
use bitflags as _;
use chrono as _;
use clap as _;
// Dev-dependencies (used in benchmarks and tests only)
#[cfg(test)]
use criterion as _;
use dirs_next as _;
#[cfg(test)]
use hex as _;
use hostname as _;
use indicatif as _;
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
#[cfg(test)]
use sha2 as _;
use smallvec as _;
#[cfg(test)]
use tempfile as _;
#[cfg(not(windows))]
use thiserror as _;
use tokio as _;
use tracing as _;
use tracing_appender as _;
use tracing_subscriber as _;
#[cfg(not(windows))]
use uffs_polars as _;
use uffs_text as _;
#[cfg(windows)]
use windows as _;

// ============================================================================
// Module declarations
// ============================================================================

pub mod discovery;
// Phase 3: error, flags, ntfs have zero external module-path use;
// downstream callers go through the flat `uffs_mft::{MftError, Result,
// FileFlags, SECTOR_SIZE, AttributeIterator, ...}` re-exports below.
pub(crate) mod error;
pub(crate) mod flags;
pub mod index;
pub mod raw;
pub mod raw_iocp;
pub(crate) mod tree_metrics;

// Cross-platform modules (NTFS structures and parsing)
pub(crate) mod ntfs; // NTFS structure definitions - cross-platform
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

// Re-export cache types.  `load_or_build_dataframe_cached` is Windows-only
// (depends on the `uffs_polars::DataFrame` build) and gated below.
#[cfg(windows)]
pub use cache::load_or_build_dataframe_cached;
pub use cache::{
    CacheStatus, FileLock, INDEX_TTL_SECONDS, LockKind, MultiDriveCacheStatus, atomic_write,
    cache_age_seconds, cache_dir, cache_file_path, cache_lock_path, check_cache_status,
    check_multi_drive_cache, cleanup_expired_cache, compress_encrypt_write, compress_zstd_mt,
    create_secure_dir, is_cache_fresh, load_cached_index, migrate_legacy_cache,
    remove_all_cached_indices, remove_cached_index, save_to_cache, save_to_cache_background,
    secure_cache_dir, secure_remove, set_file_permissions_owner_only, with_file_lock,
};
pub use error::{MftError, Result};
pub use flags::FileFlags;
// Re-export lean index types
pub use index::{
    ChildInfo, FileRecord, IndexBuildTiming, IndexNameRef, IndexStreamInfo, LinkInfo, MftIndex,
    NO_ENTRY, ROOT_FRS, SizeInfo, StandardInfo, UsnApplyStats, bytes_to_mb_f64, f64_to_u64,
    f64_to_usize, frs_to_usize, len_to_u16, len_to_u32, micros_to_i64, millis_to_u64, nanos_to_u64,
    nonneg_to_u64, u32_as_usize, u32_to_f64, u64_to_f64, usize_to_f64, usize_to_u64,
};
// Re-export I/O types for advanced usage.  Public-API anchors that have
// no current external consumers but are part of the documented surface are
// kept pub.  The Windows-only reader structs are pub(crate) at their
// definition site and have no `crate::<reader>` consumers (only
// `crate::io::readers::<reader>` paths), so no crate-root re-export is
// needed for them.
#[cfg(windows)]
pub use io::{
    AlignedBuffer, ExtensionAttributes, MftExtentMap, MftRecordMerger, ParseResult, ParsedColumns,
    ParsedRecord, ReadChunk, apply_fixup, generate_read_chunks, parse_record_full,
    parse_record_zero_alloc,
};
// Re-export NTFS constants and types (pure Rust data structures, cross-platform)
pub use ntfs::SECTOR_SIZE;
pub use ntfs::{
    AttributeIterator, AttributeListEntry, AttributeRecordHeader, AttributeRef, AttributeType,
    DataRun, ExtendedStandardInfo, FileNameAttribute, FileRecordSegmentHeader, IndexHeader,
    IndexRoot, MultiSectorHeader, NameInfo, NonResidentAttributeData, NtfsBootSector,
    ReparseMountPointBuffer, ReparsePointHeader, ReparseTag, ResidentAttributeData,
    StandardInformation, StreamInfo, apply_usa_fixup, extract_data_runs_from_attribute,
    fixup_file_record, parse_data_runs,
};
// Re-export platform types
// Core types (DriveType, MftBitmap, MftExtent) are pure data — available on all platforms
// Windows-specific types and functions (VolumeHandle, detect_ntfs_drives, etc.) only on
// Windows
pub use platform::{DriveType, MftBitmap, MftExtent, SystemMemory, query_system_memory};
// External-API anchors with cross-crate consumers.  Other Windows-only
// platform items (NtfsVolumeData, detect_drive_type, infer_drive_from_path,
// is_volume_read_only) are pub(crate) and consumed only via
// `crate::platform::*` paths, so no crate-root re-export is needed.
#[cfg(windows)]
pub use platform::{VolumeHandle, detect_ntfs_drives, is_elevated};
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
// Re-export USN Journal types
pub use usn::{
    ChangeType, FileChange, UsnJournalInfo, UsnRecord, aggregate_changes, query_usn_journal,
    read_usn_journal, reason,
};
