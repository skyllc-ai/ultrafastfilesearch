//! Shared record-filter helpers for `IndexQuery` execution.

use uffs_mft::index::{FileRecord, MftIndex};

use super::TypeFilter;
use crate::index_search::IndexPattern;

/// Shared record filter state reused by `collect()` and `count()`.
pub(super) struct RecordFilter<'a> {
    /// Index used for name lookup during pattern matching.
    index: &'a MftIndex,
    /// Optional compiled name pattern.
    pattern: Option<&'a IndexPattern>,
    /// Whether pattern matching should be case-sensitive.
    case_sensitive: bool,
    /// File-vs-directory filter.
    type_filter: TypeFilter,
    /// Minimum first-stream size.
    min_size: Option<u64>,
    /// Maximum first-stream size.
    max_size: Option<u64>,
}

impl<'a> RecordFilter<'a> {
    /// Create a reusable record filter.
    #[must_use]
    pub(super) const fn new(
        index: &'a MftIndex,
        pattern: Option<&'a IndexPattern>,
        case_sensitive: bool,
        type_filter: TypeFilter,
        min_size: Option<u64>,
        max_size: Option<u64>,
    ) -> Self {
        Self {
            index,
            pattern,
            case_sensitive,
            type_filter,
            min_size,
            max_size,
        }
    }

    /// Return whether the record satisfies the shared record-level filters.
    #[must_use]
    pub(super) fn matches(&self, record: &FileRecord) -> bool {
        match self.type_filter {
            TypeFilter::FilesOnly if record.is_directory() => return false,
            TypeFilter::DirsOnly if !record.is_directory() => return false,
            TypeFilter::All | TypeFilter::FilesOnly | TypeFilter::DirsOnly => {}
        }

        let size = record.first_stream.size.length;
        if let Some(min) = self.min_size {
            if size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if size > max {
                return false;
            }
        }

        if let Some(pat) = self.pattern {
            let name = self.index.record_name(record);
            if !pat.matches(name, self.case_sensitive) {
                return false;
            }
        }

        true
    }
}
