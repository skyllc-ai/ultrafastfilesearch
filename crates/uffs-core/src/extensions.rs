//! Extension filtering and collection aliases.
//!
//! Provides extension-based file filtering with support for:
//! - Individual extensions: `jpg`, `png`, `txt`
//! - Collection aliases: `pictures`, `documents`, `videos`, `music`
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

use std::collections::HashSet;

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
    pub fn parse(input: &str) -> Result<Self, &'static str> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

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
}
