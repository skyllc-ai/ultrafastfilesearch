// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Multi-drive path resolution helpers.

use std::collections::HashMap;

use uffs_polars::{Column, DataFrame};

use super::fast::{FastPathResolver, FastPathResolverStats};
use crate::error::Result;

/// Multi-drive path resolver using `FastPathResolver` per drive.
///
/// This is the recommended way to resolve paths for multi-drive searches.
/// Build it from FULL MFT data, then use it to add paths to filtered results.
///
/// # Example
///
/// ```rust,ignore
/// // Build resolver from FULL MFT data (before filtering)
/// let mut resolver = FastPathResolverMultiDrive::new();
/// resolver.add_drive(uffs_mft::platform::DriveLetter::C, &full_c_drive_df)?;
/// resolver.add_drive(uffs_mft::platform::DriveLetter::D, &full_d_drive_df)?;
///
/// // Apply filters to get search results
/// let filtered = apply_filters(&full_df)?;
///
/// // Add paths using the pre-built resolver
/// let with_paths = resolver.add_path_column(&filtered)?;
/// ```
#[derive(Debug, Clone, Default)]
pub struct FastPathResolverMultiDrive {
    /// Per-drive resolvers.
    resolvers: HashMap<uffs_mft::platform::DriveLetter, FastPathResolver>,
}

impl FastPathResolverMultiDrive {
    /// Create a new empty multi-drive resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a drive's MFT data to the resolver.
    ///
    /// **IMPORTANT:** Pass the FULL MFT `DataFrame`, not filtered data.
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn add_drive(
        &mut self,
        drive: uffs_mft::platform::DriveLetter,
        full_mft_df: &DataFrame,
    ) -> Result<()> {
        let resolver = FastPathResolver::build(full_mft_df, drive)?;
        self.resolvers.insert(drive, resolver);
        Ok(())
    }

    /// Get a resolver for a specific drive.
    #[must_use]
    pub fn get(&self, drive: uffs_mft::platform::DriveLetter) -> Option<&FastPathResolver> {
        self.resolvers.get(&drive)
    }

    /// Get a mutable resolver for a specific drive.
    pub fn get_mut(
        &mut self,
        drive: uffs_mft::platform::DriveLetter,
    ) -> Option<&mut FastPathResolver> {
        self.resolvers.get_mut(&drive)
    }

    /// Add a "path" column to a filtered `DataFrame`.
    ///
    /// The `DataFrame` must have a "drive" column (e.g., "C:") and "frs"
    /// column. Paths are resolved using the pre-built resolvers.
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn add_path_column(&mut self, filtered_df: &DataFrame) -> Result<DataFrame> {
        let drive_col = filtered_df.column("drive")?.str()?;
        let frs_col = filtered_df.column("frs")?.u64()?;

        let paths: Vec<String> = (0..filtered_df.height())
            .map(|i| {
                let drive_str = drive_col.get(i);
                let frs = frs_col.get(i);

                match (drive_str, frs) {
                    (Some(drive), Some(frs_val)) => {
                        // Fall back to `DriveLetter::X` for malformed
                        // drive prefixes — see legacy.rs for rationale.
                        let drive_letter = drive
                            .chars()
                            .next()
                            .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
                            .unwrap_or(uffs_mft::platform::DriveLetter::X);
                        self.resolvers.get_mut(&drive_letter).map_or_else(
                            || format!("<no resolver for {drive_letter}>"),
                            |resolver| resolver.resolve_cached(frs_val),
                        )
                    }
                    _ => "<null>".to_owned(),
                }
            })
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = filtered_df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }

    /// Get statistics for all drives.
    #[must_use]
    pub fn stats(&self) -> Vec<(uffs_mft::platform::DriveLetter, FastPathResolverStats)> {
        self.resolvers
            .iter()
            .map(|(&drive, resolver)| (drive, resolver.stats()))
            .collect()
    }

    /// Number of drives in the resolver.
    #[must_use]
    pub fn drive_count(&self) -> usize {
        self.resolvers.len()
    }
}

/// Add paths to a single-drive filtered `DataFrame`.
///
/// This is the correct way to add paths when you have:
/// 1. Full MFT data (for building the resolver)
/// 2. Filtered results (to add paths to)
///
/// # Arguments
///
/// * `full_mft_df` - The FULL MFT `DataFrame` (before filtering)
/// * `filtered_df` - The filtered search results
/// * `volume` - Drive letter (e.g., 'C')
///
/// # Errors
///
/// Returns an error if required columns are missing.
pub fn add_paths_from_full_data(
    full_mft_df: &DataFrame,
    filtered_df: &DataFrame,
    volume: uffs_mft::platform::DriveLetter,
) -> Result<DataFrame> {
    let mut resolver = FastPathResolver::build(full_mft_df, volume)?;
    resolver.add_path_column(filtered_df)
}
