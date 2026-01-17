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
#![allow(clippy::module_name_repetitions)]

// Suppress unused crate warnings for dev-dependencies (used in benchmarks/tests
// only)
#[cfg(test)]
use criterion as _;
#[cfg(test)]
use tokio as _;

// ============================================================================
// Module declarations
// ============================================================================

mod error;
mod export;
pub mod extensions;
pub mod glob;
pub mod output;
mod path_resolver;
pub mod pattern;
mod query;
pub mod tree;

// ============================================================================
// Public API re-exports
// ============================================================================

pub use error::{CoreError, Result};
pub use export::{export_csv, export_json, export_table};
pub use path_resolver::PathResolver;
pub use query::MftQuery;
// Re-export commonly used types
pub use uffs_mft::{DataFrame, FileFlags, LazyFrame};
pub use uffs_polars::columns;
