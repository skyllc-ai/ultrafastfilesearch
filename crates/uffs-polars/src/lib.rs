// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! # uffs-polars: Polars Facade for UFFS (Ultra Fast File Search)
//!
//! This crate provides a pre-compiled Polars wrapper for the UFFS project.
//! It exists solely for **compilation time isolation**.
//!
//! ## Why This Crate Exists
//!
//! Polars is a powerful but heavy dependency (~4 minutes to compile).
//! By isolating it in this facade crate:
//!
//! - Polars compiles **once** and is cached
//! - Changes to `uffs-mft`, `uffs-core`, etc. don't trigger Polars
//!   recompilation
//! - Development iteration time drops from ~4 min to ~25 seconds
//!
//! ## Usage
//!
//! All other crates in the workspace depend on `uffs-polars` instead of
//! `polars` directly:
//!
//! ```toml
//! [dependencies]
//! uffs-polars = { workspace = true }
//! ```
//!
//! Then import from this crate:
//!
//! ```rust,ignore
//! use uffs_polars::*;
//!
//! let df = DataFrame::new_infer_height(vec![
//!     Column::new("name".into(), &["file1.txt", "file2.rs"]),
//!     Column::new("size".into(), &[1024u64, 2048]),
//! ])?;
//! ```
//!
//! ## Re-exported Types
//!
//! This crate re-exports everything from `polars::prelude` plus commonly
//! used types for convenience.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

// ============================================================================
// Re-export polars prelude (primary API)
// ============================================================================
pub use polars::prelude::*;
// ============================================================================
// Re-export specific modules for advanced usage
// ============================================================================
pub use polars::{chunked_array, datatypes, error, frame, lazy, series};

// ============================================================================
// Convenience type aliases for UFFS
// ============================================================================

/// A `DataFrame` containing MFT (Master File Table) data.
///
/// Schema:
/// - `frs`: `UInt64` - File Record Segment number
/// - `parent_frs`: `UInt64` - Parent directory FRS
/// - `name`: `String` - File/directory name
/// - `size`: `UInt64` - File size in bytes
/// - `created`: `Datetime[μs]` - Creation timestamp
/// - `modified`: `Datetime[μs]` - Modification timestamp
/// - `accessed`: `Datetime[μs]` - Access timestamp
/// - `flags`: `UInt16` - Bit-packed file attributes
// NOTE: Reserved schema alias retained for Phase 3 type-safety wave.
// Phase 2.5 audit shows 0 current consumers, but this is part of the
// documented MFT DataFrame contract surface.  Targeted allow is
// preferred over deletion — the alias is a public API anchor.
pub type MftDataFrame = DataFrame;

/// A `LazyFrame` for deferred MFT query execution.
// NOTE: Reserved schema alias retained for Phase 3 lazy-query wave.
// Same rationale as `MftDataFrame` above.
pub type MftLazyFrame = LazyFrame;

// ============================================================================
// MFT Column Names (constants for type safety)
// ============================================================================

/// Column names used in MFT `DataFrame`s
pub mod columns {
    /// File Record Segment number (primary key)
    pub const FRS: &str = "frs";
    /// Parent directory FRS (foreign key)
    // NOTE: Reserved column name retained for Phase 3 schema work.
    // Currently 0 consumers but part of the documented MFT schema.
    pub const PARENT_FRS: &str = "parent_frs";
    /// File or directory name
    pub const NAME: &str = "name";
    /// File size in bytes
    pub const SIZE: &str = "size";
    /// Creation timestamp
    pub const CREATED: &str = "created";
    /// Modification timestamp
    pub const MODIFIED: &str = "modified";
    /// Access timestamp
    pub const ACCESSED: &str = "accessed";
    /// Bit-packed file attributes (see `uffs_mft::flags`)
    pub const FLAGS: &str = "flags";
    /// Full resolved path (computed column)
    pub const PATH: &str = "path";
    /// File extension (computed column)
    // NOTE: Reserved column name retained for Phase 3 schema work.
    // Currently 0 consumers but part of the documented MFT schema.
    pub const EXTENSION: &str = "extension";
}
