//! Output configuration and formatting.
//!
//! Provides customizable output formatting with:
//! - Column selection
//! - Custom separators
//! - Quote handling
//! - Boolean representation (pos/neg)
//! - Header control

#![allow(clippy::single_call_fn)]

use std::io::Write;

use uffs_polars::{Column, DataFrame, DataType};

use crate::error::Result;

/// Available output columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::missing_docs_in_private_items)]
pub enum OutputColumn {
    /// Full path including filename.
    Path,
    /// Filename only.
    Name,
    /// Directory path without filename.
    PathOnly,
    /// File size in bytes.
    Size,
    /// Allocated size on disk.
    SizeOnDisk,
    /// Creation timestamp.
    Created,
    /// Last modification timestamp.
    Modified,
    /// Last access timestamp.
    Accessed,
    /// File type/extension.
    Type,
    /// File attributes string.
    Attributes,
    /// Raw attribute flags as number.
    AttributeValue,
    /// Hidden attribute.
    Hidden,
    /// System attribute.
    System,
    /// Archive attribute.
    Archive,
    /// Read-only attribute.
    ReadOnly,
    /// Compressed attribute.
    Compressed,
    /// Encrypted attribute.
    Encrypted,
    /// Sparse file attribute.
    Sparse,
    /// Reparse point attribute.
    Reparse,
    /// Offline attribute.
    Offline,
    /// Not content indexed attribute.
    NotIndexed,
    /// Temporary file attribute.
    Temporary,
    /// Virtual file attribute.
    Virtual,
    /// Pinned attribute.
    Pinned,
    /// Unpinned attribute.
    Unpinned,
    /// Descendant count (for directories).
    Descendants,
    /// Sum of logical file sizes under a directory.
    TreeSize,
    /// Sum of allocated sizes under a directory.
    TreeAllocated,
    /// Fragmentation metric: `tree_allocated` / `treesize` ratio.
    Bulkiness,
    /// Integrity stream attribute (`ReFS`).
    Integrity,
    /// No scrub data attribute.
    NoScrub,
    /// Directory flag (boolean, separate from Type).
    DirectoryFlag,
}

/// Default column order matching C++ output exactly.
///
/// This is the order used when `--columns all` is specified.
pub const CPP_COLUMN_ORDER: &[OutputColumn] = &[
    OutputColumn::Path,
    OutputColumn::Name,
    OutputColumn::PathOnly,
    OutputColumn::Size,
    OutputColumn::SizeOnDisk,
    OutputColumn::Created,
    OutputColumn::Modified,
    OutputColumn::Accessed,
    OutputColumn::Descendants,
    OutputColumn::ReadOnly,
    OutputColumn::Archive,
    OutputColumn::System,
    OutputColumn::Hidden,
    OutputColumn::Offline,
    OutputColumn::NotIndexed,
    OutputColumn::NoScrub,
    OutputColumn::Integrity,
    OutputColumn::Pinned,
    OutputColumn::Unpinned,
    OutputColumn::DirectoryFlag,
    OutputColumn::Compressed,
    OutputColumn::Encrypted,
    OutputColumn::Sparse,
    OutputColumn::Reparse,
    OutputColumn::Attributes,
];

impl OutputColumn {
    /// Parse column name from string.
    ///
    /// Supports both full names and short aliases for CPP compatibility:
    /// - `r` → readonly
    /// - `a` → archive
    /// - `s` → system
    /// - `h` → hidden
    /// - `o` → offline
    /// - `directory` → `is_directory` (mapped to Type)
    /// - `notcontent` → notindexed
    /// - `written` → modified
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "path" => Some(Self::Path),
            "name" => Some(Self::Name),
            "pathonly" => Some(Self::PathOnly),
            "size" => Some(Self::Size),
            "sizeondisk" => Some(Self::SizeOnDisk),
            "created" => Some(Self::Created),
            // CPP uses "written", Rust uses "modified" - support both
            "modified" | "written" => Some(Self::Modified),
            "accessed" => Some(Self::Accessed),
            // "directory" maps to Type (which shows file/directory)
            "type" | "directory" => Some(Self::Type),
            "attributes" => Some(Self::Attributes),
            "attributevalue" => Some(Self::AttributeValue),
            // Short aliases for CPP compatibility
            "hidden" | "h" => Some(Self::Hidden),
            "system" | "s" => Some(Self::System),
            "archive" | "a" => Some(Self::Archive),
            "readonly" | "r" => Some(Self::ReadOnly),
            "compressed" => Some(Self::Compressed),
            "encrypted" => Some(Self::Encrypted),
            "sparse" => Some(Self::Sparse),
            "reparse" => Some(Self::Reparse),
            "offline" | "o" => Some(Self::Offline),
            // CPP uses "notcontent", Rust uses "notindexed" - support both
            "notindexed" | "notcontent" => Some(Self::NotIndexed),
            "temporary" => Some(Self::Temporary),
            "virtual" => Some(Self::Virtual),
            "pinned" => Some(Self::Pinned),
            "unpinned" => Some(Self::Unpinned),
            // CPP typo "decendents" supported for compatibility
            "descendants" | "decendents" => Some(Self::Descendants),
            "treesize" | "tree_size" => Some(Self::TreeSize),
            "treeallocated" | "tree_allocated" => Some(Self::TreeAllocated),
            "bulkiness" => Some(Self::Bulkiness),
            // New columns for C++ parity
            "integrity" => Some(Self::Integrity),
            "noscrub" => Some(Self::NoScrub),
            "directoryflag" => Some(Self::DirectoryFlag),
            _ => None,
        }
    }

    /// Get the `DataFrame` column name.
    #[must_use]
    pub const fn df_column(&self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Name => "name",
            Self::PathOnly => "path_only",
            Self::Size => "size",
            Self::SizeOnDisk => "allocated_size",
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Accessed => "accessed",
            Self::Type => "type",
            // Both Attributes and AttributeValue map to the raw flags column
            // C++ outputs the numeric value in the "Attributes" column
            Self::Attributes | Self::AttributeValue => "flags",
            // MFT reader uses is_ prefix for boolean flags
            Self::Hidden => "is_hidden",
            Self::System => "is_system",
            Self::Archive => "is_archive",
            Self::ReadOnly => "is_readonly",
            Self::Compressed => "is_compressed",
            Self::Encrypted => "is_encrypted",
            Self::Sparse => "is_sparse",
            Self::Reparse => "is_reparse",
            Self::Offline => "is_offline",
            Self::NotIndexed => "is_not_indexed",
            Self::Temporary => "is_temporary",
            Self::Virtual => "is_virtual",
            Self::Pinned => "is_pinned",
            Self::Unpinned => "is_unpinned",
            // Tree columns (computed on-demand)
            Self::Descendants => "descendants",
            Self::TreeSize => "treesize",
            Self::TreeAllocated => "tree_allocated",
            Self::Bulkiness => "bulkiness",
            // New columns for C++ parity
            Self::Integrity => "is_integrity_stream",
            Self::NoScrub => "is_no_scrub_data",
            Self::DirectoryFlag => "is_directory",
        }
    }

    /// Get the display name for headers (matches C++ output exactly).
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Path => "Path",
            Self::Name => "Name",
            Self::PathOnly => "Path Only",
            Self::Size => "Size",
            Self::SizeOnDisk => "Size on Disk",
            Self::Created => "Created",
            Self::Modified => "Last Written",
            Self::Accessed => "Last Accessed",
            Self::Type => "Type",
            Self::Attributes => "Attributes",
            Self::AttributeValue => "AttributeValue",
            Self::Hidden => "Hidden",
            Self::System => "System",
            Self::Archive => "Archive",
            Self::ReadOnly => "Read-only",
            Self::Compressed => "Compressed",
            Self::Encrypted => "Encrypted",
            Self::Sparse => "Sparse",
            Self::Reparse => "Reparse",
            Self::Offline => "Offline",
            Self::NotIndexed => "Not content indexed file",
            Self::Temporary => "Temporary",
            Self::Virtual => "Virtual",
            Self::Pinned => "Pinned",
            Self::Unpinned => "Unpinned",
            Self::Descendants => "Descendants",
            Self::TreeSize => "TreeSize",
            Self::TreeAllocated => "TreeAllocated",
            Self::Bulkiness => "Bulkiness",
            Self::Integrity => "Integrity",
            Self::NoScrub => "No scrub file",
            Self::DirectoryFlag => "Directory Flag",
        }
    }

    /// Check if this column is a tree-derived column.
    #[must_use]
    pub const fn is_tree_column(&self) -> bool {
        matches!(
            self,
            Self::Descendants | Self::TreeSize | Self::TreeAllocated | Self::Bulkiness
        )
    }

    /// Convert to a tree column if applicable.
    #[must_use]
    #[allow(clippy::wildcard_enum_match_arm)] // Intentional: only tree columns convert
    pub const fn to_tree_column(&self) -> Option<crate::tree::TreeColumn> {
        match self {
            Self::Descendants => Some(crate::tree::TreeColumn::Descendants),
            Self::TreeSize => Some(crate::tree::TreeColumn::TreeSize),
            Self::TreeAllocated => Some(crate::tree::TreeColumn::TreeAllocated),
            Self::Bulkiness => Some(crate::tree::TreeColumn::Bulkiness),
            _ => None,
        }
    }

    /// Get the default value for this column when it's missing from the
    /// `DataFrame`.
    ///
    /// Numeric and boolean columns return "0" to match C++ output behavior.
    /// String and timestamp columns return empty string.
    #[must_use]
    pub const fn default_value(&self) -> &'static str {
        match self {
            // Numeric columns default to "0"
            // Boolean columns default to "0" (false)
            Self::Size
            | Self::SizeOnDisk
            | Self::Descendants
            | Self::TreeSize
            | Self::TreeAllocated
            | Self::Bulkiness
            | Self::Attributes
            | Self::Hidden
            | Self::System
            | Self::Archive
            | Self::ReadOnly
            | Self::Compressed
            | Self::Encrypted
            | Self::Sparse
            | Self::Reparse
            | Self::Offline
            | Self::NotIndexed
            | Self::DirectoryFlag
            | Self::Temporary
            | Self::Virtual
            | Self::Pinned
            | Self::Unpinned
            | Self::Integrity
            | Self::NoScrub => "0",
            // String and timestamp columns default to empty
            Self::Path
            | Self::Name
            | Self::PathOnly
            | Self::Type
            | Self::AttributeValue
            | Self::Created
            | Self::Modified
            | Self::Accessed => "",
        }
    }
}

/// Output configuration for customizable formatting.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Columns to output (None = all available).
    pub columns: Option<Vec<OutputColumn>>,
    /// Column separator (default: ",").
    pub separator: String,
    /// Quote character for strings (default: "\"").
    pub quote: String,
    /// Include header row (default: true).
    pub header: bool,
    /// Representation for true/active boolean (default: "1").
    pub pos: String,
    /// Representation for false/inactive boolean (default: "0").
    pub neg: String,
    /// Fixed timezone offset in seconds from UTC (computed once at startup).
    /// This matches C++ behavior where Windows' `FileTimeToLocalFileTime()`
    /// uses the CURRENT timezone offset for ALL timestamps, ignoring
    /// historical DST.
    pub timezone_offset_secs: i32,
}

impl Default for OutputConfig {
    fn default() -> Self {
        // Get current timezone offset once, matching C++ behavior where
        // Windows' FileTimeToLocalFileTime() uses the CURRENT offset for all timestamps
        let timezone_offset_secs = chrono::Local::now().offset().local_minus_utc();

        Self {
            columns: None,
            separator: ",".to_owned(),
            quote: "\"".to_owned(),
            header: true,
            pos: "1".to_owned(),
            neg: "0".to_owned(),
            timezone_offset_secs,
        }
    }
}

impl OutputConfig {
    /// Create a new output configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse columns from a comma-separated string.
    ///
    /// Special value "all" returns None (meaning all columns).
    #[must_use]
    #[allow(clippy::shadow_reuse)]
    pub fn parse_columns(input: &str) -> Option<Vec<OutputColumn>> {
        let input = input.trim().to_lowercase();
        if input == "all" {
            return None;
        }

        let cols: Vec<OutputColumn> = input
            .split(',')
            .filter_map(|col| OutputColumn::parse(col.trim()))
            .collect();

        if cols.is_empty() { None } else { Some(cols) }
    }

    /// Parse separator with special character handling.
    ///
    /// Supports (case-insensitive):
    /// - TAB → `\t`
    /// - NEWLINE, NEW LINE → `\n`
    /// - SPACE → ` `
    /// - RETURN → `\r`
    /// - DOUBLE → `"`
    /// - SINGLE → `'`
    /// - NULL → `\0`
    #[must_use]
    pub fn parse_separator(input: &str) -> String {
        match input.to_uppercase().as_str() {
            "TAB" => "\t".to_owned(),
            "NEWLINE" | "NEW LINE" => "\n".to_owned(),
            "SPACE" => " ".to_owned(),
            "RETURN" => "\r".to_owned(),
            "DOUBLE" => "\"".to_owned(),
            "SINGLE" => "'".to_owned(),
            "NULL" => "\0".to_owned(),
            _ => input.to_owned(),
        }
    }

    /// Set columns from string.
    #[must_use]
    pub fn with_columns(mut self, columns: &str) -> Self {
        self.columns = Self::parse_columns(columns);
        self
    }

    /// Set separator.
    #[must_use]
    pub fn with_separator(mut self, sep: &str) -> Self {
        self.separator = Self::parse_separator(sep);
        self
    }

    /// Set quote character.
    #[must_use]
    pub fn with_quote(mut self, quote: &str) -> Self {
        quote.clone_into(&mut self.quote);
        self
    }

    /// Set header inclusion.
    #[must_use]
    pub const fn with_header(mut self, header: bool) -> Self {
        self.header = header;
        self
    }

    /// Set positive boolean representation.
    #[must_use]
    pub fn with_pos(mut self, pos: &str) -> Self {
        pos.clone_into(&mut self.pos);
        self
    }

    /// Set negative boolean representation.
    #[must_use]
    pub fn with_neg(mut self, neg: &str) -> Self {
        neg.clone_into(&mut self.neg);
        self
    }

    /// Check if the descendants column is requested.
    #[must_use]
    pub fn needs_descendants(&self) -> bool {
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&OutputColumn::Descendants))
    }

    /// Check if the path column is requested.
    ///
    /// The path column requires resolution from FRS + `parent_frs`.
    /// Returns true when columns is None (meaning "all") since "all" includes
    /// Path.
    #[must_use]
    pub fn needs_path_column(&self) -> bool {
        self.columns.as_ref().is_none_or(|cols| {
            cols.contains(&OutputColumn::Path) || cols.contains(&OutputColumn::PathOnly)
        })
    }

    /// Check if any tree-derived columns are requested.
    /// Note: "all" columns does NOT include tree columns by default (they're
    /// expensive to compute).
    #[must_use]
    pub fn needs_tree_columns(&self) -> bool {
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.iter().any(OutputColumn::is_tree_column))
    }

    /// Get the list of requested tree columns.
    #[must_use]
    pub fn get_tree_columns(&self) -> Vec<crate::tree::TreeColumn> {
        self.columns
            .as_ref()
            .map(|cols| {
                cols.iter()
                    .filter_map(OutputColumn::to_tree_column)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Write `DataFrame` to output with this configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    #[allow(clippy::option_if_let_else)]
    pub fn write<W: Write>(&self, df: &DataFrame, mut writer: W) -> Result<()> {
        // Determine columns to output - use CPP_COLUMN_ORDER when "all" is specified
        let output_cols: &[OutputColumn] = if let Some(cols) = &self.columns {
            cols.as_slice()
        } else {
            CPP_COLUMN_ORDER
        };

        // Write header if enabled
        if self.header {
            let header_names: Vec<String> = output_cols
                .iter()
                .map(|col| format!("{}{}{}", self.quote, col.display_name(), self.quote))
                .collect();
            // C++ outputs header followed by empty line
            writeln!(writer, "{}", header_names.join(&self.separator))?;
            writeln!(writer)?;
        }

        // Write data rows
        for row_idx in 0..df.height() {
            let mut row_values = Vec::with_capacity(output_cols.len());

            for col in output_cols {
                let col_name = col.df_column();
                if let Ok(series) = df.column(col_name) {
                    let value = self.format_value(series, row_idx);
                    row_values.push(value);
                } else {
                    // Column not in DataFrame - use appropriate default
                    // Numeric columns (like Descendants) should show "0" to match C++
                    row_values.push(col.default_value().to_owned());
                }
            }

            writeln!(writer, "{}", row_values.join(&self.separator))?;
        }

        Ok(())
    }

    /// Format a single value from a series.
    #[allow(clippy::option_if_let_else, clippy::wildcard_enum_match_arm)]
    fn format_value(&self, series: &Column, row_idx: usize) -> String {
        use chrono::FixedOffset;
        use uffs_polars::{AnyValue, TimeUnit};

        let dtype = series.dtype();

        match dtype {
            DataType::Boolean => {
                if let Ok(val) = series.bool() {
                    match val.get(row_idx) {
                        Some(true) => self.pos.clone(),
                        Some(false) => self.neg.clone(),
                        None => String::new(),
                    }
                } else {
                    String::new()
                }
            }
            DataType::String => {
                if let Ok(val) = series.str() {
                    val.get(row_idx).map_or_else(String::new, |str_val| {
                        format!("{}{}{}", self.quote, str_val, self.quote)
                    })
                } else {
                    String::new()
                }
            }
            DataType::UInt64 | DataType::Int64 | DataType::UInt32 | DataType::Int32 => {
                // C++ outputs "0" for null numeric values, not empty string
                match series.get(row_idx) {
                    Ok(AnyValue::Null) | Err(_) => "0".to_owned(),
                    Ok(val) => val.to_string(),
                }
            }
            DataType::Datetime(TimeUnit::Microseconds, _) => {
                // Convert UTC timestamp to local time using FIXED offset (matching C++ output).
                // C++ uses Windows' FileTimeToLocalFileTime() which applies the CURRENT
                // timezone offset to ALL timestamps, ignoring historical DST transitions.
                // We match this by using a fixed offset computed once at startup.
                if let Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) = series.get(row_idx) {
                    // Use div_euclid/rem_euclid for correct handling of negative timestamps.
                    // rem_euclid(1_000_000) always returns [0, 999_999] for any i64 input.
                    let secs = ts.div_euclid(1_000_000);
                    let micros_i64 = ts.rem_euclid(1_000_000);
                    // Safe: rem_euclid(1_000_000) is always in [0, 999_999], fits in u32
                    let micros = u32::try_from(micros_i64).unwrap_or(0);
                    if let Some(utc_dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
                        // Apply fixed timezone offset (computed once at startup)
                        // This matches C++ behavior: same offset for all timestamps
                        if let Some(fixed_tz) = FixedOffset::east_opt(self.timezone_offset_secs) {
                            let local_dt = utc_dt.with_timezone(&fixed_tz);
                            // Format WITHOUT subseconds to match C++ output exactly
                            local_dt.format("%Y-%m-%d %H:%M:%S").to_string()
                        } else {
                            // Fallback: format as UTC if offset is invalid
                            utc_dt.format("%Y-%m-%d %H:%M:%S").to_string()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
            _ => series
                .get(row_idx)
                .map_or(String::new(), |val| val.to_string()),
        }
    }
}

/// Recursively count descendants for a given FRS.
///
/// Uses memoization to avoid recomputing counts for the same FRS.
/// Compute descendants count for each directory in the `DataFrame`.
///
/// This function builds a parent-child tree from the `frs` and `parent_frs`
/// columns, then counts all descendants (files and subdirectories) for each
/// entry.
///
/// For files, the descendants count is 0.
/// For directories, it's the total count of all nested items.
///
/// # Arguments
///
/// * `df` - `DataFrame` with columns: `frs`, `parent_frs`, `is_directory`,
///   `size`, `allocated_size`
///
/// # Returns
///
/// A new `DataFrame` with an added `descendants` column (u64).
///
/// # Errors
///
/// Returns an error if required columns are missing.
///
/// # Note
///
/// This is a convenience wrapper around [`crate::tree::add_tree_columns`].
/// For more tree columns (`treesize`, `tree_allocated`, `bulkiness`), use the
/// tree module directly.
pub fn add_descendants_column(df: &DataFrame) -> Result<DataFrame> {
    crate::tree::add_tree_columns(df, &[crate::tree::TreeColumn::Descendants])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_column() {
        assert_eq!(OutputColumn::parse("path"), Some(OutputColumn::Path));
        assert_eq!(OutputColumn::parse("PATH"), Some(OutputColumn::Path));
        assert_eq!(OutputColumn::parse("size"), Some(OutputColumn::Size));
        assert_eq!(OutputColumn::parse("unknown"), None);
    }

    #[test]
    fn test_parse_columns_all() {
        assert!(OutputConfig::parse_columns("all").is_none());
        assert!(OutputConfig::parse_columns("ALL").is_none());
    }

    #[test]
    fn test_parse_columns_list() {
        let cols = OutputConfig::parse_columns("path,name,size").expect("should parse");
        assert_eq!(cols.len(), 3);
        assert_eq!(cols.first(), Some(&OutputColumn::Path));
        assert_eq!(cols.get(1), Some(&OutputColumn::Name));
        assert_eq!(cols.get(2), Some(&OutputColumn::Size));
    }

    #[test]
    fn test_parse_separator_special() {
        // Original values
        assert_eq!(OutputConfig::parse_separator("TAB"), "\t");
        assert_eq!(OutputConfig::parse_separator("tab"), "\t");
        assert_eq!(OutputConfig::parse_separator("NEWLINE"), "\n");
        assert_eq!(OutputConfig::parse_separator(";"), ";");
        // CPP compatibility values
        assert_eq!(OutputConfig::parse_separator("NEW LINE"), "\n");
        assert_eq!(OutputConfig::parse_separator("SPACE"), " ");
        assert_eq!(OutputConfig::parse_separator("RETURN"), "\r");
        assert_eq!(OutputConfig::parse_separator("DOUBLE"), "\"");
        assert_eq!(OutputConfig::parse_separator("SINGLE"), "'");
        assert_eq!(OutputConfig::parse_separator("NULL"), "\0");
    }

    #[test]
    fn test_parse_column_aliases() {
        // Short aliases for CPP compatibility
        assert_eq!(OutputColumn::parse("r"), Some(OutputColumn::ReadOnly));
        assert_eq!(OutputColumn::parse("a"), Some(OutputColumn::Archive));
        assert_eq!(OutputColumn::parse("s"), Some(OutputColumn::System));
        assert_eq!(OutputColumn::parse("h"), Some(OutputColumn::Hidden));
        assert_eq!(OutputColumn::parse("o"), Some(OutputColumn::Offline));
        // CPP name aliases
        assert_eq!(OutputColumn::parse("written"), Some(OutputColumn::Modified));
        assert_eq!(
            OutputColumn::parse("notcontent"),
            Some(OutputColumn::NotIndexed)
        );
        assert_eq!(OutputColumn::parse("directory"), Some(OutputColumn::Type));
        // CPP typo support
        assert_eq!(
            OutputColumn::parse("decendents"),
            Some(OutputColumn::Descendants)
        );
    }

    #[test]
    fn test_output_config_builder() {
        let config = OutputConfig::new()
            .with_columns("path,name")
            .with_separator(";")
            .with_quote("'")
            .with_header(false)
            .with_pos("+")
            .with_neg("-");

        assert!(config.columns.is_some());
        assert_eq!(config.separator, ";");
        assert_eq!(config.quote, "'");
        assert!(!config.header);
        assert_eq!(config.pos, "+");
        assert_eq!(config.neg, "-");
    }

    #[test]
    fn test_df_column_mapping() {
        assert_eq!(OutputColumn::Path.df_column(), "path");
        assert_eq!(OutputColumn::SizeOnDisk.df_column(), "allocated_size");
        assert_eq!(OutputColumn::AttributeValue.df_column(), "flags");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(OutputColumn::Path.display_name(), "Path");
        assert_eq!(OutputColumn::SizeOnDisk.display_name(), "Size on Disk");
        assert_eq!(
            OutputColumn::NotIndexed.display_name(),
            "Not content indexed file"
        );
    }

    #[test]
    fn test_needs_descendants() {
        let config_no_desc = OutputConfig::new().with_columns("path,name,size");
        assert!(!config_no_desc.needs_descendants());

        let config_with_desc = OutputConfig::new().with_columns("path,descendants,size");
        assert!(config_with_desc.needs_descendants());

        // "all" columns returns None, so needs_descendants should be false
        let config_all = OutputConfig::new().with_columns("all");
        assert!(!config_all.needs_descendants());
    }

    #[test]
    fn test_add_descendants_column() {
        // Create a test DataFrame with directory structure:
        // root (5) -> Users (100) -> john (101) -> file.txt (102)
        //                         -> Documents (103) -> doc.pdf (104)
        let df = DataFrame::new_infer_height(vec![
            Column::new("frs".into(), &[5_u64, 100, 101, 102, 103, 104]),
            Column::new("parent_frs".into(), &[0_u64, 5, 100, 101, 100, 103]),
            Column::new(
                "is_directory".into(),
                &[true, true, true, false, true, false],
            ),
            Column::new("size".into(), &[0_u64, 0, 0, 1000, 0, 50000]),
            Column::new(
                "allocated_size".into(),
                &[4096_u64, 4096, 4096, 4096, 4096, 53248],
            ),
        ])
        .unwrap();

        let result = add_descendants_column(&df).unwrap();

        // Check that descendants column was added
        let desc_col = result.column("descendants").unwrap().u64().unwrap();

        // Descendants = direct children + all their descendants (recursive)
        // root (5): children=[Users(100)] -> 1 + descendants(100) = 1 + 4 = 5
        // Users (100): children=[john(101), Documents(103)] -> 2 + desc(101) +
        // desc(103) = 2 + 1 + 1 = 4 john (101): children=[file.txt(102)] -> 1 +
        // 0 = 1 file.txt (102): 0 (file)
        // Documents (103): children=[doc.pdf(104)] -> 1 + 0 = 1
        // doc.pdf (104): 0 (file)
        assert_eq!(desc_col.get(0), Some(5)); // root -> all 5 items below
        assert_eq!(desc_col.get(1), Some(4)); // Users -> john, file.txt, Documents, doc.pdf
        assert_eq!(desc_col.get(2), Some(1)); // john -> file.txt
        assert_eq!(desc_col.get(3), Some(0)); // file.txt (file)
        assert_eq!(desc_col.get(4), Some(1)); // Documents -> doc.pdf
        assert_eq!(desc_col.get(5), Some(0)); // doc.pdf (file)
    }
}
