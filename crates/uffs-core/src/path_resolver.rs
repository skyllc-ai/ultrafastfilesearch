//! Path resolution from FRS numbers.
//!
//! Reconstructs full file paths from the parent-child FRS relationships.

use std::collections::HashMap;

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
    volume: char,
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
    pub fn build(df: &DataFrame, volume: char) -> Result<Self> {
        let frs_col = df.column("frs")?.u64()?;
        let parent_col = df.column("parent_frs")?.u64()?;
        let name_col = df.column("name")?.str()?;

        let mut entries = HashMap::with_capacity(df.height());

        for i in 0..df.height() {
            if let (Some(frs), Some(parent), Some(name)) =
                (frs_col.get(i), parent_col.get(i), name_col.get(i))
            {
                entries.insert(frs, (parent, name.to_string()));
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
    /// Returns an error if the FRS is not found or a circular reference is detected.
    pub fn resolve(&mut self, frs: u64) -> Result<String> {
        // Check cache first
        if let Some(path) = self.cache.get(&frs) {
            return Ok(path.clone());
        }

        // Build path by walking up the tree
        let mut components = Vec::new();
        let mut current = frs;
        let mut visited = std::collections::HashSet::new();

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

        // Build path from components (reverse order)
        components.reverse();
        let path = format!("{}:\\{}", self.volume, components.join("\\"));

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
            .into_iter()
            .map(|frs| {
                frs.map_or_else(
                    || "<null>".to_string(),
                    |f| self.resolve(f).unwrap_or_else(|_| "<unknown>".to_string()),
                )
            })
            .collect();

        let path_series = Column::new("path".into(), paths);
        let mut result = df.clone();
        result.with_column(path_series)?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_df() -> DataFrame {
        DataFrame::new(vec![
            Column::new("frs".into(), &[5u64, 100, 101, 102]),
            Column::new("parent_frs".into(), &[0u64, 5, 100, 101]),
            Column::new("name".into(), &["", "Users", "john", "Documents"]),
        ])
        .unwrap()
    }

    #[test]
    fn test_resolve_path() {
        let df = create_test_df();
        let mut resolver = PathResolver::build(&df, 'C').unwrap();

        let path = resolver.resolve(102).unwrap();
        assert_eq!(path, "C:\\Users\\john\\Documents");
    }

    #[test]
    fn test_path_caching() {
        let df = create_test_df();
        let mut resolver = PathResolver::build(&df, 'C').unwrap();

        // First resolution
        let path1 = resolver.resolve(102).unwrap();
        // Second resolution (should use cache)
        let path2 = resolver.resolve(102).unwrap();

        assert_eq!(path1, path2);
    }
}

