// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Filtering methods for `MftQuery`.

use uffs_polars::{NamedFrom as _, PlSmallStr, Series, col, lit};

use super::MftQuery;

impl MftQuery {
    // =========================================================================
    // Type Filters
    // =========================================================================

    /// Filter to files only (exclude directories).
    #[must_use]
    pub fn files_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_directory").eq(lit(false))),
        }
    }

    /// Filter to directories only.
    #[must_use]
    pub fn directories_only(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_directory").eq(lit(true))),
        }
    }

    /// Exclude hidden files.
    #[must_use]
    pub fn exclude_hidden(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_hidden").eq(lit(false))),
        }
    }

    /// Exclude system files (by `is_system` attribute flag).
    #[must_use]
    pub fn exclude_system(self) -> Self {
        Self {
            lazy: self.lazy.filter(col("is_system").eq(lit(false))),
        }
    }

    /// Hide reserved NTFS metafiles (`$MFT`, `$LogFile`, `$Bitmap`, the
    /// `$Extend` family, …).
    ///
    /// Only the fixed set of reserved names from
    /// [`crate::compact::is_ntfs_metafile_name`] is removed (matched
    /// case-insensitively).  Ordinary `$`-prefixed files — `$Recycle.Bin`,
    /// `$PatchCache`, the `WinSxS` `$$_*.cdf-ms` filemaps — are real
    /// user-visible files (Everything and Explorer show them) and are KEPT.
    /// A plain `name.starts_with('$')` filter wrongly hid all of those.
    #[must_use]
    pub fn hide_system_files(self) -> Self {
        let lower: Vec<String> = crate::compact::NTFS_METAFILE_NAMES
            .iter()
            .map(|name| name.to_lowercase())
            .collect();
        let series = Series::new(PlSmallStr::EMPTY, &lower);
        Self {
            lazy: self.lazy.filter(
                col("name")
                    .str()
                    .to_lowercase()
                    .is_in(lit(series).implode(true), false)
                    .not(),
            ),
        }
    }

    /// Hide NTFS metadata records (FRS < 16, except FRS 5 which is root).
    ///
    /// NTFS reserves the first 16 File Record Segments for system metadata:
    /// - FRS 0: `$MFT` (Master File Table)
    /// - FRS 1: `$MFTMirr` (MFT mirror)
    /// - FRS 2: `$LogFile` (transaction log)
    /// - FRS 3: `$Volume` (volume info)
    /// - FRS 4: `$AttrDef` (attribute definitions)
    /// - FRS 5: `.` (root directory) - **KEPT**
    /// - FRS 6: `$Bitmap` (cluster allocation)
    /// - FRS 7: `$Boot` (boot sector)
    /// - FRS 8: `$BadClus` (bad clusters)
    /// - FRS 9: `$Secure` (security descriptors)
    /// - FRS 10: `$UpCase` (uppercase table)
    /// - FRS 11: `$Extend` (extended metadata)
    /// - FRS 12-15: Reserved
    ///
    /// This matches the legacy UFFS behavior which excludes
    /// these but keeps the root directory.
    #[must_use]
    pub fn hide_metadata_records(self) -> Self {
        // Keep FRS >= 16 OR FRS == 5 (root directory)
        Self {
            lazy: self
                .lazy
                .filter(col("frs").gt_eq(lit(16_u64)).or(col("frs").eq(lit(5_u64)))),
        }
    }

    /// Hide reserved NTFS metafiles, by both record position and name.
    ///
    /// 1. [`hide_metadata_records`](Self::hide_metadata_records) — drops the
    ///    FRS 0-15 reserved records (keeps root FRS 5).
    /// 2. [`hide_system_files`](Self::hide_system_files) — drops any record
    ///    whose name is a reserved metafile (`$MFT`, `$Extend`, …).
    ///
    /// Ordinary `$`-prefixed files (`$Recycle.Bin`, `WinSxS` `$$_*.cdf-ms`) are
    /// NOT hidden by either step.
    #[must_use]
    pub fn hide_system(self) -> Self {
        self.hide_metadata_records().hide_system_files()
    }

    // =========================================================================
    // Size Filters
    // =========================================================================

    /// Filter files with size >= bytes.
    #[must_use]
    pub fn min_size(self, bytes: u64) -> Self {
        Self {
            lazy: self.lazy.filter(col("size").gt_eq(lit(bytes))),
        }
    }

    /// Filter files with size <= bytes.
    #[must_use]
    pub fn max_size(self, bytes: u64) -> Self {
        Self {
            lazy: self.lazy.filter(col("size").lt_eq(lit(bytes))),
        }
    }

    /// Filter files within size range.
    #[must_use]
    pub fn size_between(self, min: u64, max: u64) -> Self {
        Self {
            lazy: self
                .lazy
                .filter(col("size").gt_eq(lit(min)).and(col("size").lt_eq(lit(max)))),
        }
    }

    // =========================================================================
    // Date Filters
    // =========================================================================

    /// Filter files modified after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn modified_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("modified").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files modified before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn modified_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("modified").lt(lit(timestamp_micros))),
        }
    }

    /// Filter files created after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn created_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("created").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files created before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn created_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("created").lt(lit(timestamp_micros))),
        }
    }

    /// Filter files accessed after a given timestamp (Unix microseconds).
    #[must_use]
    pub fn accessed_after(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("accessed").gt(lit(timestamp_micros))),
        }
    }

    /// Filter files accessed before a given timestamp (Unix microseconds).
    #[must_use]
    pub fn accessed_before(self, timestamp_micros: i64) -> Self {
        Self {
            lazy: self.lazy.filter(col("accessed").lt(lit(timestamp_micros))),
        }
    }
}
