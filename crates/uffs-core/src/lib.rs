// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! # uffs-core: Query Engine for UFFS (Ultra Fast File Search)
//!
//! This crate provides a powerful query engine for searching and filtering
//! MFT data using Polars lazy API.
//!
//! ## Features
//!
//! - **Fluent Query API**: Chain filters naturally
//! - **Pattern Matching**: Glob and regex support with SIMD acceleration
//! - **Path Resolution**: Reconstruct full paths from FRS numbers
//! - **Export Formats**: Table, JSON, CSV output
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use uffs_mft::MftReader;
//! use uffs_core::MftQuery;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Load MFT data
//!     let df = MftReader::load_parquet("c_drive.parquet")?;
//!
//!     // Query using fluent API
//!     let results = MftQuery::new(df)
//!         .glob("*.rs")
//!         .files_only()
//!         .min_size(1024)
//!         .sort_by_size(true)
//!         .limit(100)
//!         .collect()?;
//!
//!     // Export results
//!     uffs_core::export_table(&results, std::io::stdout())?;
//!
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

extern crate alloc;

// Suppress unused crate warnings for deps used by sub-modules or reserved
#[cfg(test)]
use criterion as _;
use devicons as _;
use memchr as _;
use tokio as _;

// ============================================================================
// Module declarations
// ============================================================================

pub mod aggregate;
pub mod bloom;
pub mod compact;
pub mod compact_cache;
pub(crate) mod compact_filters;
pub mod compact_loader;
pub mod compact_mmap;
pub mod compact_reader;
pub mod compact_storage;
pub(crate) mod compiled_pattern;
mod error;
mod export;
pub mod extensions;
pub mod format;
pub mod glob;
pub mod index_search;
pub mod output;
mod path_resolver;
pub mod path_trie;
pub mod pattern;
mod query;
pub mod search;
pub(crate) mod slot_pool;
pub mod tree;
pub mod trigram;

// ============================================================================
// Public API re-exports
// ============================================================================

pub use compiled_pattern::{CompiledPattern, GlobKind, classify_glob, compile_pattern};
pub use error::{CoreError, Result};
pub use export::{export_csv, export_json, export_table};
pub use extensions::{
    ExtensionFilter, ExtensionIndex, ExtensionIndexStats, add_ext_column, ext_expr, has_ext_column,
};
pub use index_search::{
    IndexPattern, IndexQuery, QueryComplexity, QueryFeatures, QueryMode, SearchResult, TypeFilter,
    analyze_pattern_complexity, compile_extensions, compile_extensions_with_fold,
    compile_index_pattern, compile_index_pattern_with_fold, compile_parsed_pattern,
    compile_parsed_pattern_with_fold,
};
#[expect(
    deprecated,
    reason = "re-exporting deprecated function for backward compatibility"
)]
pub use path_resolver::add_path_column_multi_drive;
pub use path_resolver::{
    FastPathResolver, FastPathResolverMultiDrive, FastPathResolverStats, NameArena, PathResolver,
    add_path_only_column, add_paths_from_full_data,
};
pub use query::MftQuery;
pub use slot_pool::{DriveLoadEstimate, SlotPool, compute_load_budget, estimate_drive_cost};
pub use tree::{TreeColumn, add_tree_columns, apply_directory_treesize};
// `CaseFold` is the only cross-crate re-export here: it lives on
// `ParkedBody.fold` / `DriveCompactIndex.fold` and is re-exported so
// downstream crates (uffs-daemon, uffs-cli, tests) that touch those
// types don't need a direct `uffs-text` dependency.
//
// Polars types (`DataFrame`, `LazyFrame`, `IntoLazy`, `col`, `lit`,
// `columns`) and `uffs_mft::FileFlags` were re-exported here until
// 2026-05-08 but are NOT anymore — both blocks were dead code (zero
// downstream consumers, verified via repo-wide grep before deletion).
// Consumers that need polars types or `FileFlags` must depend on
// `uffs-polars` / `uffs-mft` directly.  This keeps the polars-tainted
// dep graph explicit and avoids transitive re-publishing of polars
// APIs through `uffs-core` — see `release-automation-plan.md`
// deviation log row "R6 → R8 publishability resolution (Path A)".
pub use uffs_text::case_fold::CaseFold;
