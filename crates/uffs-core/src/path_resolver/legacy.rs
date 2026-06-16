// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Legacy path resolver implementations retained for backward compatibility.
//!
//! Contains the original `HashMap`-based [`PathResolver`] and the deprecated
//! multi-drive helper.

use std::collections::{HashMap, HashSet};

use uffs_polars::{Column, DataFrame};

use crate::error::{CoreError, Result};

/// Resolves full paths from FRS (File Record Segment) numbers.
///
/// The MFT stores files with parent references (FRS numbers), not full paths.
/// This resolver builds a lookup table to reconstruct full paths efficiently.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_core::PathResolver;
///
/// let resolver = PathResolver::build(&df)?;
/// let path = resolver.resolve(12345)?;
/// println!("Full path: {}", path);
/// ```
pub struct PathResolver {
    /// Map from FRS to (`parent_frs`, name)
    entries: HashMap<u64, (u64, String)>,
    /// Cache of resolved paths
    cache: HashMap<u64, String>,
    /// Volume letter (e.g., 'C')
    volume: uffs_mft::platform::DriveLetter,
}

impl PathResolver {
    /// Build a path resolver from a `DataFrame`.
    ///
    /// # Arguments
    ///
    /// * `df` - `DataFrame` with columns: frs, `parent_frs`, name
    /// * `volume` - Drive letter (e.g., 'C')
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn build(df: &DataFrame, volume: uffs_mft::platform::DriveLetter) -> Result<Self> {
        let frs_col = df.column("frs")?.u64()?;
        let parent_col = df.column("parent_frs")?.u64()?;
        let name_col = df.column("name")?.str()?;

        let mut entries = HashMap::with_capacity(df.height());

        for i in 0..df.height() {
            if let (Some(frs), Some(parent), Some(name)) =
                (frs_col.get(i), parent_col.get(i), name_col.get(i))
            {
                entries.insert(frs, (parent, name.to_owned()));
            }
        }

        Ok(Self {
            entries,
            cache: HashMap::new(),
            volume,
        })
    }

    /// Resolve the full path for a given FRS.
    ///
    /// # Errors
    ///
    /// Returns an error if the FRS is not found or a circular reference is
    /// detected.
    pub fn resolve(&mut self, frs: u64) -> Result<String> {
        // Check cache first
        if let Some(path) = self.cache.get(&frs) {
            return Ok(path.clone());
        }

        // Build path by walking up the tree
        let mut components = Vec::new();
        let mut current = frs;
        let mut visited = HashSet::new();

        while current != 0 && current != 5 {
            // 5 is root directory FRS
            if !visited.insert(current) {
                return Err(CoreError::CircularReference(current));
            }

            if let Some((parent, name)) = self.entries.get(&current) {
                components.push(name.clone());
                current = *parent;
            } else {
                return Err(CoreError::PathResolution(current));
            }
        }

        // Build path from components (reverse order, uppercase drive letter)
        components.reverse();
        let path = format!("{}:\\{}", self.volume.as_char(), components.join("\\"));

        // Cache the result
        self.cache.insert(frs, path.clone());

        Ok(path)
    }

    /// Add a "path" column to the `DataFrame` with resolved paths.
    ///
    /// # Errors
    ///
    /// Returns an error if path resolution fails.
    pub fn add_path_column(&mut self, df: &DataFrame) -> Result<DataFrame> {
        let frs_col = df.column("frs")?.u64()?;

        let paths: Vec<String> = frs_col
            .iter()
            .map(|frs| {
                frs.map_or_else(
                    || "<null>".to_owned(),
                    |frs_val| {
                        self.resolve(frs_val)
                            .unwrap_or_else(|_| "<unknown>".to_owned())
                    },
                )
            })
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }
}

/// Add a "path" column to a multi-drive `DataFrame`.
///
/// **WARNING:** This function builds the resolver from the passed `DataFrame`.
/// If the `DataFrame` is filtered (e.g., only matching files), parent
/// directories may be missing, causing `<unknown>` paths.
///
/// For correct path resolution, use [`super::add_paths_from_full_data`]
/// instead, which builds the resolver from full MFT data before filtering.
///
/// # Errors
///
/// Returns an error if required columns are missing or path resolution fails.
#[deprecated(
    since = "0.2.13",
    note = "Use add_paths_from_full_data() for correct path resolution"
)]
pub fn add_path_column_multi_drive(df: &DataFrame) -> Result<DataFrame> {
    // Check if we have a drive column
    let has_drive_col = df.column("drive").is_ok();

    if !has_drive_col {
        // Single drive - need to infer from context or fail
        return Err(CoreError::MissingColumn("drive".to_owned()));
    }

    let drive_col = df.column("drive")?.str()?;
    let frs_col = df.column("frs")?.u64()?;
    let parent_col = df.column("parent_frs")?.u64()?;
    let name_col = df.column("name")?.str()?;

    // Build per-drive resolvers
    let mut resolvers: HashMap<uffs_mft::platform::DriveLetter, PathResolver> = HashMap::new();

    // First pass: build entries for each drive
    for i in 0..df.height() {
        if let (Some(drive_str), Some(frs), Some(parent), Some(name)) = (
            drive_col.get(i),
            frs_col.get(i),
            parent_col.get(i),
            name_col.get(i),
        ) {
            // Extract drive letter from "C:" format; fall back to
            // `DriveLetter::X` (matches the previous `'?'` sentinel
            // semantics — any record whose drive prefix isn't ASCII
            // A–Z gets bucketed under the "unknown" letter).
            let drive_letter = drive_str
                .chars()
                .next()
                .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
                .unwrap_or(uffs_mft::platform::DriveLetter::X);

            let resolver = resolvers
                .entry(drive_letter)
                .or_insert_with(|| PathResolver {
                    entries: HashMap::new(),
                    cache: HashMap::new(),
                    volume: drive_letter,
                });

            resolver.entries.insert(frs, (parent, name.to_owned()));
        }
    }

    // Second pass: resolve paths
    let paths: Vec<String> = (0..df.height())
        .map(|i| {
            let drive_str = drive_col.get(i);
            let frs = frs_col.get(i);

            match (drive_str, frs) {
                (Some(drive), Some(frs_val)) => {
                    let drive_letter = drive
                        .chars()
                        .next()
                        .and_then(|ch| uffs_mft::platform::DriveLetter::parse(ch).ok())
                        .unwrap_or(uffs_mft::platform::DriveLetter::X);
                    resolvers.get_mut(&drive_letter).map_or_else(
                        || "<unknown>".to_owned(),
                        |resolver| {
                            resolver
                                .resolve(frs_val)
                                .unwrap_or_else(|_| "<unknown>".to_owned())
                        },
                    )
                }
                _ => "<null>".to_owned(),
            }
        })
        .collect();

    let path_series = Column::new("path".into(), paths);
    let mut result = df.clone();
    result.with_column(path_series)?;

    Ok(result)
}
