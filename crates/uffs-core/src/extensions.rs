//! Extension filtering, collection aliases, and extension indexing.
//!
//! Provides extension-based file filtering with support for:
//! - Individual extensions: `jpg`, `png`, `txt`
//! - Collection aliases: `pictures`, `documents`, `videos`, `music`
//! - Extension index for fast `*.ext` queries
//!
//! # Examples
//!
//! ```
//! use uffs_core::extensions::ExtensionFilter;
//!
//! // Parse extension list with collections
//! let filter = ExtensionFilter::parse("jpg,mp4,documents").unwrap();
//! assert!(filter.matches("photo.jpg"));
//! assert!(filter.matches("video.mp4"));
//! assert!(filter.matches("report.pdf")); // from documents collection
//! ```

#![allow(clippy::single_call_fn)]

use std::collections::{HashMap, HashSet};

use uffs_polars::{DataFrame, Expr, IntoLazy, col, lit};

use crate::error::Result;

/// Predefined extension collections.
pub mod collections {
    /// Picture file extensions.
    pub const PICTURES: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "svg", "ico", "raw", "heic",
    ];

    /// Document file extensions.
    pub const DOCUMENTS: &[&str] = &[
        "doc", "docx", "pdf", "txt", "rtf", "odt", "xls", "xlsx", "ppt", "pptx", "csv", "md",
    ];

    /// Video file extensions.
    pub const VIDEOS: &[&str] = &[
        "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "mpeg", "mpg", "m4v", "3gp",
    ];

    /// Music/audio file extensions.
    pub const MUSIC: &[&str] = &[
        "mp3", "wav", "flac", "aac", "ogg", "wma", "m4a", "opus", "aiff",
    ];

    /// Archive file extensions.
    pub const ARCHIVES: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso"];

    /// Code/source file extensions.
    pub const CODE: &[&str] = &[
        "rs", "py", "js", "ts", "java", "c", "cpp", "h", "hpp", "go", "rb", "php", "swift", "kt",
    ];
}

/// Extension filter for matching files by extension.
#[derive(Debug, Clone)]
pub struct ExtensionFilter {
    /// Set of lowercase extensions (without leading dot).
    extensions: HashSet<String>,
}

impl ExtensionFilter {
    /// Create a new empty extension filter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            extensions: HashSet::new(),
        }
    }

    /// Parse a comma-separated list of extensions and collection names.
    ///
    /// Supports:
    /// - Individual extensions: `jpg`, `png`, `.txt` (dot is stripped)
    /// - Collection aliases: `pictures`, `documents`, `videos`, `music`,
    ///   `archives`, `code`
    ///
    /// # Errors
    ///
    /// Returns an error if the input is empty.
    #[allow(clippy::shadow_reuse)]
    pub fn parse(input: &str) -> core::result::Result<Self, &'static str> {
        let trimmed_input = input.trim();
        if trimmed_input.is_empty() {
            return Err("Extension filter cannot be empty");
        }

        let mut filter = Self::new();

        for raw_part in trimmed_input.split(',') {
            let part = raw_part.trim().to_lowercase();
            if part.is_empty() {
                continue;
            }

            // Check if it's a collection alias
            match part.as_str() {
                "pictures" | "images" => filter.add_collection(collections::PICTURES),
                "documents" | "docs" => filter.add_collection(collections::DOCUMENTS),
                "videos" | "video" => filter.add_collection(collections::VIDEOS),
                "music" | "audio" => filter.add_collection(collections::MUSIC),
                "archives" | "compressed" => filter.add_collection(collections::ARCHIVES),
                "code" | "source" => filter.add_collection(collections::CODE),
                _ => {
                    // Individual extension - strip leading dot if present
                    let ext = part.strip_prefix('.').unwrap_or(&part);
                    filter.extensions.insert(ext.to_owned());
                }
            }
        }

        Ok(filter)
    }

    /// Add a collection of extensions.
    fn add_collection(&mut self, exts: &[&str]) {
        for ext in exts {
            self.extensions.insert((*ext).to_owned());
        }
    }

    /// Check if a filename matches any of the extensions.
    #[must_use]
    pub fn matches(&self, filename: &str) -> bool {
        // Extract extension from filename using split to avoid string slicing
        if let Some(ext_part) = filename.rsplit('.').next() {
            // Only match if there was actually a dot (not just the filename itself)
            if ext_part.len() < filename.len() {
                let ext = ext_part.to_lowercase();
                return self.extensions.contains(&ext);
            }
        }
        false
    }

    /// Get the set of extensions.
    #[must_use]
    pub const fn extensions(&self) -> &HashSet<String> {
        &self.extensions
    }

    /// Convert to a regex pattern for Polars filtering.
    ///
    /// Returns a regex that matches filenames ending with any of the
    /// extensions.
    #[must_use]
    pub fn to_regex(&self) -> String {
        if self.extensions.is_empty() {
            return String::new();
        }

        let exts: Vec<&str> = self.extensions.iter().map(String::as_str).collect();
        format!(r"\.({})$", exts.join("|"))
    }
}

impl Default for ExtensionFilter {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Column Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Create a Polars expression that extracts the file extension from a name
/// column.
///
/// The extension is extracted as lowercase, without the leading dot.
/// Files without extensions (or hidden files like `.gitignore`) return null.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_core::extensions::ext_expr;
///
/// // Add extension column to `DataFrame`
/// let df = df.lazy()
///     .with_column(ext_expr("name").alias("ext"))
///     .collect()?;
/// ```
pub fn ext_expr(name_column: &str) -> Expr {
    // Extract extension: everything after the last dot, lowercased
    // Returns null for files without extensions or hidden files (starting with dot)
    let name_col = col(name_column);

    // Find the last dot position
    // Use str().extract() with regex to get extension
    // Pattern: match a dot followed by non-dot characters at the end
    // But only if there's something before the dot (not hidden files)
    name_col
        .str()
        .to_lowercase()
        .str()
        .extract(lit(r"[^.]\.([^.]+)$"), 1)
}

/// Add an `ext` column to a `DataFrame` for optimized extension queries.
///
/// The extension column contains lowercase extensions without the leading dot.
/// Files without extensions have null values.
///
/// # Errors
///
/// Returns an error if the `DataFrame` operation fails.
///
/// # Example
///
/// ```rust,ignore
/// use uffs_core::extensions::add_ext_column;
///
/// let df = add_ext_column(df)?;
/// // Now df has an "ext" column for fast extension queries
/// ```
pub fn add_ext_column(df: DataFrame) -> Result<DataFrame> {
    df.lazy()
        .with_column(ext_expr("name").alias("ext"))
        .collect()
        .map_err(crate::error::CoreError::from)
}

/// Check if a `DataFrame` has an `ext` column.
#[must_use]
pub fn has_ext_column(df: &DataFrame) -> bool {
    df.get_column_names()
        .iter()
        .any(|name| name.as_str() == "ext")
}

// ═══════════════════════════════════════════════════════════════════════════
// ExtensionIndex - Pre-built extension → FRS mapping
// ═══════════════════════════════════════════════════════════════════════════

/// Pre-built index mapping file extensions to FRS values.
///
/// This enables O(1) lookup for `*.ext` queries instead of scanning all files.
///
/// # Example
///
/// ```ignore
/// let index = ExtensionIndex::build(&df)?;
/// let txt_files = index.get("txt"); // Returns &[u64] of FRS values
/// ```
#[derive(Debug, Clone)]
pub struct ExtensionIndex {
    /// Extension (lowercase, no dot) → list of FRS values
    index: HashMap<String, Vec<u64>>,
    /// Total number of files indexed
    total_files: usize,
}

impl ExtensionIndex {
    /// Build an extension index from a `DataFrame`.
    ///
    /// Requires `frs` and `name` columns.
    ///
    /// # Errors
    ///
    /// Returns an error if required columns are missing.
    pub fn build(df: &DataFrame) -> Result<Self> {
        let frs_col = df.column("frs")?.u64()?;
        let name_col = df.column("name")?.str()?;

        let mut index: HashMap<String, Vec<u64>> = HashMap::new();
        let mut total_files = 0;

        for (frs_opt, name_opt) in frs_col.into_iter().zip(name_col.into_iter()) {
            let Some(frs) = frs_opt else { continue };
            let Some(name) = name_opt else { continue };

            total_files += 1;

            // Extract extension
            if let Some(ext) = Self::extract_extension(name) {
                index.entry(ext).or_default().push(frs);
            }
        }

        Ok(Self { index, total_files })
    }

    /// Extract lowercase extension from a filename.
    fn extract_extension(name: &str) -> Option<String> {
        // Find the last dot
        let dot_pos = name.rfind('.')?;

        // Must have something after the dot
        if dot_pos + 1 >= name.len() {
            return None;
        }

        // Must not be at the start (hidden files like .gitignore)
        if dot_pos == 0 {
            return None;
        }

        // Use get() for safe slicing - avoids panic on UTF-8 boundary issues
        // dot_pos is a valid char boundary since rfind returns char positions
        name.get(dot_pos + 1..).map(str::to_lowercase)
    }

    /// Get FRS values for a specific extension.
    ///
    /// Extension should be lowercase without the leading dot.
    #[must_use]
    pub fn get(&self, ext: &str) -> Option<&[u64]> {
        self.index.get(&ext.to_lowercase()).map(Vec::as_slice)
    }

    /// Get the number of files with a specific extension.
    #[must_use]
    pub fn count(&self, ext: &str) -> usize {
        self.get(ext).map_or(0, <[u64]>::len)
    }

    /// Get all indexed extensions.
    #[must_use]
    pub fn extensions(&self) -> Vec<&str> {
        self.index.keys().map(String::as_str).collect()
    }

    /// Get the total number of files indexed.
    #[must_use]
    pub const fn total_files(&self) -> usize {
        self.total_files
    }

    /// Get the number of unique extensions.
    #[must_use]
    pub fn unique_extensions(&self) -> usize {
        self.index.len()
    }

    /// Check if an extension exists in the index.
    #[must_use]
    pub fn has_extension(&self, ext: &str) -> bool {
        self.index.contains_key(&ext.to_lowercase())
    }

    /// Get statistics about the index.
    #[must_use]
    pub fn stats(&self) -> ExtensionIndexStats {
        let total_indexed: usize = self.index.values().map(Vec::len).sum();
        let max_count = self.index.values().map(Vec::len).max().unwrap_or(0);

        ExtensionIndexStats {
            total_files: self.total_files,
            files_with_extension: total_indexed,
            unique_extensions: self.index.len(),
            max_extension_count: max_count,
        }
    }
}

/// Statistics about an extension index.
#[derive(Debug, Clone)]
pub struct ExtensionIndexStats {
    /// Total files scanned.
    pub total_files: usize,
    /// Files that have an extension.
    pub files_with_extension: usize,
    /// Number of unique extensions.
    pub unique_extensions: usize,
    /// Maximum files for any single extension.
    pub max_extension_count: usize,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::std_instead_of_core)]
mod tests {
    use uffs_polars::Column;

    use super::*;

    // Use a test-specific Result type that works with CoreError
    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn test_parse_single_extension() {
        let filter = ExtensionFilter::parse("jpg").unwrap();
        assert!(filter.matches("photo.jpg"));
        assert!(filter.matches("PHOTO.JPG"));
        assert!(!filter.matches("photo.png"));
    }

    #[test]
    fn test_parse_multiple_extensions() {
        let filter = ExtensionFilter::parse("jpg,png,gif").unwrap();
        assert!(filter.matches("photo.jpg"));
        assert!(filter.matches("image.png"));
        assert!(filter.matches("anim.gif"));
        assert!(!filter.matches("doc.pdf"));
    }

    #[test]
    fn test_parse_with_dots() {
        let filter = ExtensionFilter::parse(".jpg,.png").unwrap();
        assert!(filter.matches("photo.jpg"));
        assert!(filter.matches("image.png"));
    }

    #[test]
    fn test_pictures_collection() {
        let filter = ExtensionFilter::parse("pictures").unwrap();
        assert!(filter.matches("photo.jpg"));
        assert!(filter.matches("image.png"));
        assert!(filter.matches("icon.ico"));
        assert!(!filter.matches("video.mp4"));
    }

    #[test]
    fn test_documents_collection() {
        let filter = ExtensionFilter::parse("documents").unwrap();
        assert!(filter.matches("report.pdf"));
        assert!(filter.matches("letter.docx"));
        assert!(filter.matches("data.xlsx"));
        assert!(!filter.matches("photo.jpg"));
    }

    #[test]
    fn test_mixed_collection_and_extensions() {
        let filter = ExtensionFilter::parse("pictures,mp4,pdf").unwrap();
        assert!(filter.matches("photo.jpg"));
        assert!(filter.matches("video.mp4"));
        assert!(filter.matches("doc.pdf"));
        assert!(!filter.matches("song.mp3"));
    }

    #[test]
    fn test_empty_error() {
        assert!(ExtensionFilter::parse("").is_err());
        assert!(ExtensionFilter::parse("   ").is_err());
    }

    #[test]
    fn test_to_regex() {
        let filter = ExtensionFilter::parse("jpg,png").unwrap();
        let regex = filter.to_regex();
        assert!(regex.contains("jpg"));
        assert!(regex.contains("png"));
        assert!(regex.starts_with(r"\."));
        assert!(regex.ends_with(")$"));
    }

    #[test]
    fn test_no_extension_file() {
        let filter = ExtensionFilter::parse("txt").unwrap();
        assert!(!filter.matches("README"));
        assert!(!filter.matches("Makefile"));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // ExtensionIndex tests
    // ═══════════════════════════════════════════════════════════════════════

    fn create_ext_test_df() -> DataFrame {
        DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2, 3, 4, 5, 6]),
            Column::new(
                "name".into(),
                &[
                    "photo.jpg",
                    "document.txt",
                    "image.jpg",
                    "README",
                    "script.py",
                    "data.txt",
                ],
            ),
        ])
        .unwrap()
    }

    #[test]
    fn test_extension_index_build() -> TestResult {
        let df = create_ext_test_df();
        let index = ExtensionIndex::build(&df)?;

        assert_eq!(index.total_files(), 6);
        assert_eq!(index.unique_extensions(), 3); // jpg, txt, py
        Ok(())
    }

    #[test]
    fn test_extension_index_get() -> TestResult {
        let df = create_ext_test_df();
        let index = ExtensionIndex::build(&df)?;

        let jpg_files = index.get("jpg").unwrap();
        assert_eq!(jpg_files.len(), 2);
        assert!(jpg_files.contains(&1));
        assert!(jpg_files.contains(&3));

        let txt_files = index.get("txt").unwrap();
        assert_eq!(txt_files.len(), 2);
        assert!(txt_files.contains(&2));
        assert!(txt_files.contains(&6));

        assert!(index.get("pdf").is_none());
        Ok(())
    }

    #[test]
    fn test_extension_index_case_insensitive() -> TestResult {
        let df = create_ext_test_df();
        let index = ExtensionIndex::build(&df)?;

        // Should work with any case
        assert!(index.get("JPG").is_some());
        assert!(index.get("Jpg").is_some());
        assert!(index.get("jpg").is_some());
        Ok(())
    }

    #[test]
    fn test_extension_index_stats() -> TestResult {
        let df = create_ext_test_df();
        let index = ExtensionIndex::build(&df)?;

        let stats = index.stats();
        assert_eq!(stats.total_files, 6);
        assert_eq!(stats.files_with_extension, 5); // README has no extension
        assert_eq!(stats.unique_extensions, 3);
        assert_eq!(stats.max_extension_count, 2); // jpg and txt both have 2
        Ok(())
    }

    #[test]
    fn test_extension_index_hidden_files() -> TestResult {
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2, 3]),
            Column::new("name".into(), &[".gitignore", ".bashrc", "file.txt"]),
        ])?;

        let index = ExtensionIndex::build(&df)?;

        // Hidden files should not be indexed as extensions
        assert!(index.get("gitignore").is_none());
        assert!(index.get("bashrc").is_none());
        assert!(index.get("txt").is_some());
        Ok(())
    }

    // =========================================================================
    // Extension Column Tests
    // =========================================================================

    #[test]
    fn test_add_ext_column() -> TestResult {
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2, 3, 4, 5]),
            Column::new(
                "name".into(),
                &[
                    "photo.jpg",
                    "document.txt",
                    "README",
                    ".gitignore",
                    "archive.tar.gz",
                ],
            ),
        ])?;

        let result = add_ext_column(df)?;

        // Check that ext column was added
        assert!(has_ext_column(&result));

        // Verify the ext column values
        let ext_col = result.column("ext")?.str()?;
        assert_eq!(ext_col.get(0), Some("jpg"));
        assert_eq!(ext_col.get(1), Some("txt"));
        assert!(ext_col.get(2).is_none()); // README has no extension
        assert!(ext_col.get(3).is_none()); // .gitignore is hidden file
        assert_eq!(ext_col.get(4), Some("gz")); // tar.gz -> gz

        Ok(())
    }

    #[test]
    fn test_has_ext_column() -> TestResult {
        let df_without = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64]),
            Column::new("name".into(), &["file.txt"]),
        ])?;

        assert!(!has_ext_column(&df_without));

        let df_with = add_ext_column(df_without)?;
        assert!(has_ext_column(&df_with));

        Ok(())
    }

    #[test]
    fn test_ext_expr_lowercase() -> TestResult {
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[1_u64, 2]),
            Column::new("name".into(), &["Photo.JPG", "Document.TXT"]),
        ])?;

        let result = add_ext_column(df)?;
        let ext_col = result.column("ext")?.str()?;

        // Extensions should be lowercase
        assert_eq!(ext_col.get(0), Some("jpg"));
        assert_eq!(ext_col.get(1), Some("txt"));

        Ok(())
    }
}
