//! Output type definitions: attribute enums, record filter, sort column types.

/// A single attribute requirement: attribute must be set (Include) or not set
/// (Exclude).
#[derive(Debug, Clone, Copy)]
pub(in crate::commands) enum AttrRequirement {
    /// Attribute must be set (e.g., `hidden`).
    Include(AttrKind),
    /// Attribute must NOT be set (e.g., `!hidden`).
    Exclude(AttrKind),
}

/// Known NTFS file attributes for filtering.
#[derive(Debug, Clone, Copy)]
pub(in crate::commands) enum AttrKind {
    /// Hidden file attribute.
    Hidden,
    /// System file attribute.
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
    /// Integrity stream attribute.
    Integrity,
    /// No scrub data attribute.
    NoScrub,
    /// Directory flag.
    Directory,
}

impl AttrKind {
    /// Parse an attribute name (case-insensitive).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "hidden" | "h" => Some(Self::Hidden),
            "system" | "s" => Some(Self::System),
            "archive" | "a" => Some(Self::Archive),
            "readonly" | "r" | "read-only" => Some(Self::ReadOnly),
            "compressed" => Some(Self::Compressed),
            "encrypted" => Some(Self::Encrypted),
            "sparse" => Some(Self::Sparse),
            "reparse" => Some(Self::Reparse),
            "offline" | "o" => Some(Self::Offline),
            "notindexed" | "notcontent" => Some(Self::NotIndexed),
            "temporary" | "temp" => Some(Self::Temporary),
            "virtual" => Some(Self::Virtual),
            "pinned" => Some(Self::Pinned),
            "unpinned" => Some(Self::Unpinned),
            "integrity" => Some(Self::Integrity),
            "noscrub" => Some(Self::NoScrub),
            "directory" | "dir" => Some(Self::Directory),
            _ => None,
        }
    }

    /// Check if this attribute is set on the given record.
    #[inline]
    #[must_use]
    pub const fn is_set(self, record: &uffs_mft::index::FileRecord) -> bool {
        match self {
            Self::Hidden => record.stdinfo.is_hidden(),
            Self::System => record.stdinfo.is_system(),
            Self::Archive => record.stdinfo.is_archive(),
            Self::ReadOnly => record.stdinfo.is_readonly(),
            Self::Compressed => record.stdinfo.is_compressed(),
            Self::Encrypted => record.stdinfo.is_encrypted(),
            Self::Sparse => record.stdinfo.is_sparse(),
            Self::Reparse => record.stdinfo.is_reparse(),
            Self::Offline => record.stdinfo.is_offline(),
            Self::NotIndexed => record.stdinfo.is_not_indexed(),
            Self::Temporary => record.stdinfo.is_temporary(),
            Self::Virtual => record.stdinfo.is_virtual(),
            Self::Pinned => record.stdinfo.is_pinned(),
            Self::Unpinned => record.stdinfo.is_unpinned(),
            Self::Integrity => record.stdinfo.is_integrity_stream(),
            Self::NoScrub => record.stdinfo.is_no_scrub_data(),
            Self::Directory => record.is_directory(),
        }
    }
}

/// Record-level filters for the streaming writer.
///
/// ALL filters are combined with AND logic and applied inline during the
/// streaming scan — no separate filter pass, zero memory overhead.
///
/// # Example CLI usage
/// ```text
/// uffs *.txt --files-only --min-size 1024 --attr hidden --newer 7d --case
/// ```
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool maps to an independent CLI flag — not a state machine"
)]
pub(in crate::commands) struct StreamingRecordFilter {
    /// Only output files (skip directories).
    pub files_only: bool,
    /// Only output directories (skip files).
    pub dirs_only: bool,
    /// Hide system/hidden files.
    pub hide_system: bool,
    /// Minimum file size filter.
    pub min_size: Option<u64>,
    /// Maximum file size filter.
    pub max_size: Option<u64>,
    /// Attribute requirements (all must be satisfied — AND logic).
    pub attr_filters: Vec<AttrRequirement>,
    /// Only records modified after this timestamp (microseconds since epoch).
    pub newer_modified: Option<i64>,
    /// Only records modified before this timestamp (microseconds since epoch).
    pub older_modified: Option<i64>,
    /// Only records created after this timestamp.
    pub newer_created: Option<i64>,
    /// Only records created before this timestamp.
    pub older_created: Option<i64>,
    /// Only records accessed after this timestamp.
    pub newer_accessed: Option<i64>,
    /// Only records accessed before this timestamp.
    pub older_accessed: Option<i64>,
    /// Exclude pattern — records matching this are rejected.
    pub exclude_pattern: Option<uffs_core::IndexPattern>,
    /// Maximum number of output rows (0 = unlimited).
    pub limit: usize,
    /// Sort specification (empty = no sort, output in FRS order).
    pub sort_spec: Vec<SortColumn>,
    /// Reverse sort order (descending).
    pub sort_desc: bool,
}

/// A single sort tier: column + direction.
#[derive(Debug, Clone, Copy)]
pub(in crate::commands) struct SortColumn {
    /// The column to sort by.
    pub kind: SortKind,
    /// Whether this tier sorts descending.
    pub descending: bool,
}

/// Sort column kind.
#[derive(Debug, Clone, Copy)]
pub(in crate::commands) enum SortKind {
    /// File size.
    Size,
    /// Allocated size on disk.
    SizeOnDisk,
    /// Last modification timestamp.
    Modified,
    /// Creation timestamp.
    Created,
    /// Last access timestamp.
    Accessed,
    /// Filename.
    Name,
    /// Full path.
    Path,
    /// File extension.
    Extension,
    /// Descendant count.
    Descendants,
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
    /// Directory flag.
    Directory,
}

impl SortKind {
    /// Smart default sort direction for this column.
    ///
    /// Dates and sizes default to descending (newest/largest first).
    /// Names and extensions default to ascending (A→Z).
    /// Booleans default to descending (true first).
    #[must_use]
    pub const fn default_descending(self) -> bool {
        matches!(
            self,
            Self::Size
                | Self::SizeOnDisk
                | Self::Modified
                | Self::Created
                | Self::Accessed
                | Self::Descendants
                | Self::Hidden
                | Self::System
                | Self::Archive
                | Self::ReadOnly
                | Self::Compressed
                | Self::Encrypted
                | Self::Directory
        )
    }

    /// Parse a sort column name (case-insensitive).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "size" => Some(Self::Size),
            "sizeondisk" | "allocated" => Some(Self::SizeOnDisk),
            "modified" | "written" | "date" => Some(Self::Modified),
            "created" => Some(Self::Created),
            "accessed" => Some(Self::Accessed),
            "name" => Some(Self::Name),
            "path" => Some(Self::Path),
            "ext" | "extension" | "type" => Some(Self::Extension),
            "descendants" => Some(Self::Descendants),
            "hidden" | "h" => Some(Self::Hidden),
            "system" | "s" => Some(Self::System),
            "archive" | "a" => Some(Self::Archive),
            "readonly" | "r" => Some(Self::ReadOnly),
            "compressed" => Some(Self::Compressed),
            "encrypted" => Some(Self::Encrypted),
            "directory" | "dir" => Some(Self::Directory),
            _ => None,
        }
    }
}
