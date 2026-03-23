//! Fast Vec-backed path resolution.

use core::mem::size_of;

use rayon::prelude::*;
use uffs_polars::{Column, DataFrame};

use super::arena::NameArena;
use crate::error::Result;

/// Entry in the fast path resolver.
/// Packed for memory efficiency (16 bytes per entry).
#[derive(Debug, Clone, Copy, Default)]
struct FastEntry {
    /// Parent FRS (0 = no parent, 5 = root).
    parent_frs: u64,
    /// Offset into the name arena.
    name_offset: u32,
    /// Length of the name.
    name_len: u16,
    /// Flags (reserved for future use).
    _flags: u16,
}

/// Fast path resolver using Vec-based O(1) lookup.
///
/// Optimized for MFT data where FRS values are typically dense (0 to
/// `max_frs`). Uses a Vec indexed by FRS for O(1) lookup instead of `HashMap`'s
/// O(1) amortized.
///
/// # Memory Layout
///
/// - `entries`: `Vec<FastEntry>` indexed by FRS (16 bytes per entry)
/// - `names`: `NameArena` holding all file names contiguously
/// - `path_cache`: Pre-computed paths for frequently accessed entries
///
/// # Example
///
/// ```rust,ignore
/// use uffs_core::FastPathResolver;
///
/// let resolver = FastPathResolver::build(&full_mft_df, 'C')?;
/// let path = resolver.resolve(12345);
/// println!("Full path: {path}");
/// ```
#[derive(Debug, Clone)]
pub struct FastPathResolver {
    /// Entries indexed by FRS. None = FRS not present.
    entries: Vec<Option<FastEntry>>,
    /// Arena holding all file names.
    names: NameArena,
    /// Volume letter (e.g., 'C').
    volume: char,
    /// Pre-computed paths for caching.
    path_cache: Vec<Option<String>>,
    /// Maximum FRS value (for bounds checking).
    max_frs: u64,
}

impl FastPathResolver {
    /// Build a fast path resolver from a `DataFrame`.
    ///
    /// **IMPORTANT:** Pass the FULL MFT `DataFrame`, not a filtered subset.
    /// This ensures all parent directories are available for path resolution.
    ///
    /// # Arguments
    ///
    /// * `df` - Full MFT `DataFrame` with columns: frs, `parent_frs`, name
    /// * `volume` - Drive letter (e.g., 'C')
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn build(df: &DataFrame, volume: char) -> Result<Self> {
        let frs_col = df.column("frs")?.u64()?;
        let parent_col = df.column("parent_frs")?.u64()?;
        let name_col = df.column("name")?.str()?;

        // Find max FRS to size the Vec
        let max_frs = frs_col.into_iter().flatten().max().unwrap_or(0);

        // Estimate name arena size (average 20 bytes per name)
        let estimated_name_bytes = df.height() * 20;
        let mut names = NameArena::with_capacity(estimated_name_bytes);

        // Pre-allocate entries Vec (u64 to usize is safe for practical MFT sizes)
        let entries_len = uffs_mft::frs_to_usize(max_frs + 1);
        let mut entries: Vec<Option<FastEntry>> = vec![None; entries_len];

        // Build entries
        for row_idx in 0..df.height() {
            if let (Some(frs), Some(parent), Some(name)) = (
                frs_col.get(row_idx),
                parent_col.get(row_idx),
                name_col.get(row_idx),
            ) {
                let (name_offset, name_len) = names.add(name);
                // Use safe get_mut to avoid indexing panic
                if let Some(slot) = entries.get_mut(uffs_mft::frs_to_usize(frs)) {
                    *slot = Some(FastEntry {
                        parent_frs: parent,
                        name_offset,
                        name_len,
                        _flags: 0,
                    });
                }
            }
        }

        // Pre-allocate path cache (empty initially)
        let path_cache = vec![None; entries.len()];

        Ok(Self {
            entries,
            names,
            volume,
            path_cache,
            max_frs,
        })
    }

    /// Resolve the full path for a given FRS.
    ///
    /// Returns the resolved path, or a fallback string if resolution fails.
    /// This method never fails - it returns `<unknown>` for unresolvable paths.
    #[must_use]
    pub fn resolve(&self, frs: u64) -> String {
        // Check cache first
        if let Some(cached) = self.get_cached(frs) {
            return cached.to_owned();
        }

        // Build path by walking up the tree
        self.build_path(frs)
    }

    /// Resolve path with mutable caching.
    ///
    /// Caches the result for future lookups.
    pub fn resolve_cached(&mut self, frs: u64) -> String {
        // Check cache first
        let frs_idx = uffs_mft::frs_to_usize(frs);
        if let Some(Some(cached)) = self.path_cache.get(frs_idx) {
            return cached.clone();
        }

        // Build path
        let path = self.build_path(frs);

        // Cache it using safe get_mut
        if let Some(slot) = self.path_cache.get_mut(frs_idx) {
            *slot = Some(path.clone());
        }

        path
    }

    /// Get a cached path if available.
    fn get_cached(&self, frs: u64) -> Option<&str> {
        self.path_cache
            .get(uffs_mft::frs_to_usize(frs))
            .and_then(|opt| opt.as_deref())
    }

    /// Build path by walking up the tree.
    fn build_path(&self, frs: u64) -> String {
        // Maximum depth to prevent infinite loops
        const MAX_DEPTH: usize = 256;

        // Pre-allocate path buffer (typical path is ~100 chars)
        let mut path_buf = String::with_capacity(128);

        // Collect components in reverse order
        let mut components: Vec<&str> = Vec::with_capacity(16);
        let mut current = frs;
        let mut depth = 0;

        while current != 0 && current != 5 && depth < MAX_DEPTH {
            if let Some(entry) = self.get_entry(current) {
                let name = self.names.get(entry.name_offset, entry.name_len);
                if !name.is_empty() {
                    components.push(name);
                }
                current = entry.parent_frs;
                depth += 1;
            } else {
                // Entry not found - return partial path with marker
                return Self::format_partial_path(&components, current);
            }
        }

        // Build final path (uppercase drive letter for legacy-output parity)
        path_buf.push(self.volume.to_ascii_uppercase());
        path_buf.push_str(":\\");

        // Append components in reverse order
        for (idx, component) in components.iter().rev().enumerate() {
            if idx > 0 {
                path_buf.push('\\');
            }
            path_buf.push_str(component);
        }

        path_buf
    }

    /// Format a partial path when resolution fails midway.
    #[expect(
        clippy::single_call_fn,
        reason = "extracted for clarity from build_path"
    )]
    fn format_partial_path(components: &[&str], missing_frs: u64) -> String {
        if components.is_empty() {
            return format!("<unknown:{missing_frs}>");
        }

        let mut path = format!("<unknown:{missing_frs}>\\");
        for (idx, component) in components.iter().rev().enumerate() {
            if idx > 0 {
                path.push('\\');
            }
            path.push_str(component);
        }
        path
    }

    /// Get an entry by FRS (O(1) lookup).
    #[inline]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "u64 FRS fits in usize on 64-bit platforms"
    )]
    fn get_entry(&self, frs: u64) -> Option<&FastEntry> {
        self.entries.get(frs as usize).and_then(Option::as_ref)
    }

    /// Add a "path" column to a `DataFrame` using this resolver (sequential).
    ///
    /// For large `DataFrames`, consider using
    /// [`FastPathResolver::add_path_column_parallel`] instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the frs column is missing.
    pub fn add_path_column(&mut self, df: &DataFrame) -> Result<DataFrame> {
        let frs_col = df.column("frs")?.u64()?;

        let paths: Vec<String> = frs_col
            .into_iter()
            .map(|frs| {
                frs.map_or_else(
                    || "<null>".to_owned(),
                    |frs_val| self.resolve_cached(frs_val),
                )
            })
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }

    /// Add a "path" column to a `DataFrame` using parallel resolution.
    ///
    /// Uses Rayon to resolve paths in parallel across multiple threads.
    /// This is faster for large `DataFrames` (>10K rows) but has overhead
    /// for small `DataFrames`.
    ///
    /// Note: This uses the non-caching `resolve()` method since caching
    /// would require synchronization overhead.
    ///
    /// # Errors
    ///
    /// Returns an error if the frs column is missing.
    pub fn add_path_column_parallel(&self, df: &DataFrame) -> Result<DataFrame> {
        let frs_col = df.column("frs")?.u64()?;

        // Collect FRS values to a Vec for parallel iteration
        let frs_values: Vec<Option<u64>> = frs_col.into_iter().collect();

        // Resolve paths in parallel
        let paths: Vec<String> = frs_values
            .par_iter()
            .map(|frs| frs.map_or_else(|| "<null>".to_owned(), |frs_val| self.resolve(frs_val)))
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }

    /// Add a "path" column, choosing sequential or parallel based on size.
    ///
    /// Uses parallel resolution for `DataFrames` with more than 10,000 rows.
    ///
    /// # Errors
    ///
    /// Returns an error if the frs column is missing.
    pub fn add_path_column_auto(&mut self, df: &DataFrame) -> Result<DataFrame> {
        const PARALLEL_THRESHOLD: usize = 10_000;

        if df.height() > PARALLEL_THRESHOLD {
            self.add_path_column_parallel(df)
        } else {
            self.add_path_column(df)
        }
    }

    /// Add a "path" column with trailing slashes for directories (legacy-output
    /// parity).
    ///
    /// Builds paths correctly for hard links by using `parent_frs` + `name`
    /// instead of just resolving `frs`. Each hard link gets its correct
    /// path based on its parent.
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn add_path_column_with_dir_suffix(&self, df: &DataFrame) -> Result<DataFrame> {
        let parent_frs_col = df.column("parent_frs")?.u64()?;
        let name_col = df.column("name")?.str()?;
        let is_dir_col = df.column("is_directory")?.bool()?;
        let stream_name_col = df.column("stream_name").ok().and_then(|col| col.str().ok());

        // Collect values for parallel iteration
        let parent_frs_values: Vec<Option<u64>> = parent_frs_col.into_iter().collect();
        let name_values: Vec<Option<&str>> = name_col.into_iter().collect();
        let is_dir_values: Vec<Option<bool>> = is_dir_col.into_iter().collect();
        let stream_names: Vec<Option<&str>> = stream_name_col.map_or_else(
            || vec![None; parent_frs_values.len()],
            |col| col.into_iter().collect(),
        );

        // Resolve paths in parallel: parent_path + name + optional stream
        let paths: Vec<String> = parent_frs_values
            .par_iter()
            .zip(name_values.par_iter())
            .zip(is_dir_values.par_iter())
            .zip(stream_names.par_iter())
            .map(|(((parent_frs, name), is_dir), stream_name)| {
                // Resolve parent directory path
                let parent_path =
                    parent_frs.map_or_else(|| "<null>".to_owned(), |frs_val| self.resolve(frs_val));

                // Build full path: parent + backslash + name
                let file_name = name.unwrap_or("<unnamed>");

                // Special case: root directory has name "." - just use parent path
                let mut path = if file_name == "." {
                    parent_path
                } else if parent_path.ends_with('\\') {
                    format!("{parent_path}{file_name}")
                } else {
                    format!("{parent_path}\\{file_name}")
                };

                // Check if this entry has an ADS stream name
                let has_ads = stream_name.is_some_and(|sn| !sn.is_empty());

                // Add trailing backslash for directories, but NOT if they have an ADS
                // (ADS paths should be "dir:stream" not "dir\\:stream")
                if is_dir.unwrap_or(false) && !path.ends_with('\\') && !has_ads {
                    path.push('\\');
                }

                // Append stream name for ADS (e.g., "file.txt:Zone.Identifier")
                if let Some(stream) = stream_name {
                    if !stream.is_empty() {
                        path.push(':');
                        path.push_str(stream);
                    }
                }
                path
            })
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }

    /// Get statistics about the resolver.
    #[must_use]
    pub fn stats(&self) -> FastPathResolverStats {
        let entry_count = self.entries.iter().filter(|entry| entry.is_some()).count();
        let cached_count = self
            .path_cache
            .iter()
            .filter(|entry| entry.is_some())
            .count();

        FastPathResolverStats {
            max_frs: self.max_frs,
            entry_count,
            name_arena_bytes: self.names.len(),
            entries_vec_bytes: self.entries.len() * size_of::<Option<FastEntry>>(),
            cached_paths: cached_count,
        }
    }
}

/// Add a `path_only` column to a `DataFrame` that has a "path" column.
///
/// The `path_only` column contains the directory portion of the path
/// (everything except the filename), with a trailing backslash.
///
/// # Example
///
/// - `C:\Users\john\file.txt` → `C:\Users\john\`
/// - `C:\file.txt` → `C:\`
///
/// # Errors
///
/// Returns an error if the "path" column is missing.
pub fn add_path_only_column(df: &DataFrame) -> Result<DataFrame> {
    let path_col = df.column("path")?.str()?;

    let path_only: Vec<String> = path_col
        .into_iter()
        .map(|path_opt| {
            path_opt.map_or_else(String::new, |path| {
                // Find the last backslash - use get() for safe UTF-8 slicing
                path.rfind('\\').map_or_else(String::new, |last_sep| {
                    // Use get() to safely slice, avoiding panic on UTF-8 boundary
                    path.get(..=last_sep)
                        .map_or_else(String::new, str::to_owned)
                })
            })
        })
        .collect();

    let path_only_series = Column::new("path_only".into(), path_only);
    let mut result = df.clone();
    result.with_column(path_only_series)?;

    Ok(result)
}

/// Statistics about a `FastPathResolver` instance.
#[derive(Debug, Clone)]
pub struct FastPathResolverStats {
    /// Maximum FRS value.
    pub max_frs: u64,
    /// Number of entries (files/directories).
    pub entry_count: usize,
    /// Bytes used by the name arena.
    pub name_arena_bytes: usize,
    /// Bytes used by the entries Vec.
    pub entries_vec_bytes: usize,
    /// Number of cached paths.
    pub cached_paths: usize,
}
