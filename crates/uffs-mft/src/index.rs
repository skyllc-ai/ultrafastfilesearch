//! # Lean MFT Index
//!
//! This module provides a compact, cache-friendly in-memory index for parsed
//! NTFS metadata.
//!
//! ## Design Philosophy
//!
//! - **No Polars overhead**: Build index directly from parsed MFT records
//! - **Compact memory layout**: Bit-packed attributes, contiguous names buffer
//! - **O(1) FRS lookup**: Direct indexing via `frs_to_idx` table
//! - **Optional `DataFrame`**: Convert to Polars only when needed for analytics
//!
//! ## Memory Layout
//!
//! ```text
//! MftIndex
//! ├── records: Vec<FileRecord>     // Core file metadata
//! ├── frs_to_idx: Vec<u32>         // FRS → record index (O(1) lookup)
//! ├── names: String                // All filenames concatenated
//! ├── links: Vec<LinkInfo>         // Hard link chain (overflow)
//! ├── streams: Vec<IndexStreamInfo>     // ADS chain (overflow)
//! └── children: Vec<ChildInfo>     // Directory contents
//! ```

mod base;
mod builder;
mod child_order;
mod dataframe;
mod extensions;
mod fragment;
mod merge;
mod model;
mod path_resolver;
mod paths;
mod standard_info;
mod stats;
mod storage;
mod tree;
mod types;
mod usn;

pub use self::extensions::{ExtensionIndex, ExtensionTable};
pub use self::fragment::MftIndexFragment;
pub use self::model::{ChildInfo, MftIndex};
pub use self::path_resolver::{CachedPath, PathCache, PathResolver};
pub use self::standard_info::StandardInfo;
pub use self::stats::{IndexBuildTiming, MftStats};
pub use self::storage::IndexHeader;
#[cfg(test)]
pub(crate) use self::types::cmp_ascii_case_insensitive;
pub use self::types::{
    FileRecord, IndexNameRef, IndexStreamInfo, InternalStreamInfo, LinkInfo, NO_ENTRY, ROOT_FRS,
    SizeInfo, frs_to_usize, len_to_u16, len_to_u32,
};
pub use self::usn::UsnApplyStats;

#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "test constants fit in target types"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "test code with known non-negative values"
)]
#[expect(
    clippy::collection_is_never_read,
    reason = "test assertions verify internal state"
)]
#[expect(
    clippy::default_numeric_fallback,
    reason = "test code — explicit types not needed"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code with known valid indices"
)]
#[expect(clippy::print_stdout, reason = "test diagnostics output")]
#[expect(
    clippy::shadow_unrelated,
    reason = "test code — variable reuse for setup"
)]
#[expect(clippy::std_instead_of_core, reason = "test code uses std types")]
#[expect(
    clippy::str_to_string,
    reason = "test code — String conversion is fine"
)]
#[expect(clippy::uninlined_format_args, reason = "test code readability")]
#[expect(clippy::use_debug, reason = "test code uses Debug for assertions")]
#[expect(
    clippy::unwrap_used,
    reason = "test code — panicking on failure is acceptable"
)]
#[expect(
    clippy::expect_used,
    reason = "test code — panicking on failure is acceptable"
)]
mod tests;
