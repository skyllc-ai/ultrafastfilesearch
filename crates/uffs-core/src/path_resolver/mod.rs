//! Path resolution from FRS numbers.
//!
//! Reconstructs full file paths from the parent-child FRS relationships.
//!
//! This module provides two implementations:
//! - [`PathResolver`]: HashMap-based, flexible but slower
//! - [`FastPathResolver`]: Vec-based O(1) lookup, optimized for MFT data
//!
//! # Performance
//!
//! For typical MFT data with millions of entries:
//! - `FastPathResolver` is 3-5x faster than `PathResolver`
//! - Uses ~50% less memory due to `NameArena`
//! - `add_path_column_parallel` uses Rayon for multi-threaded resolution

mod arena;
mod fast;
mod legacy;
mod multi_drive;

pub use arena::NameArena;
pub use fast::{FastPathResolver, FastPathResolverStats, add_path_only_column};
#[expect(
    deprecated,
    reason = "re-exporting deprecated function for backward compatibility"
)]
pub use legacy::{PathResolver, add_path_column_multi_drive};
pub use multi_drive::{FastPathResolverMultiDrive, add_paths_from_full_data};

#[cfg(test)]
mod tests;
