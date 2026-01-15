//! # uffs-mft: NTFS Master File Table Reading Library
//!
//! This crate provides high-performance direct MFT reading capabilities,
//! outputting data as Polars `DataFrame`s for efficient querying.
//!
//! ## Features
//!
//! - **Direct MFT Access**: Bypasses Windows file enumeration APIs for speed
//! - **Async I/O**: Uses tokio for high-throughput disk reading
//! - **Polars Integration**: Returns `DataFrame`s for powerful data manipulation
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
// Module declarations
// ============================================================================

pub mod error;
pub mod flags;

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
pub use reader::{MftProgress, MftReader};

// Re-export Polars types for convenience
pub use uffs_polars::{DataFrame, LazyFrame};

