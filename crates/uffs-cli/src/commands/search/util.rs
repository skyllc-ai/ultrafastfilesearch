//! Pure utility helpers for the search command.

use super::super::raw_io::QueryFilters;

/// Compute the list of output targets (drive letters) for results.
pub(super) fn compute_output_targets(
    single_drive: Option<char>,
    multi_drives: Option<&Vec<char>>,
    pattern_drive: Option<char>,
) -> Vec<char> {
    single_drive
        .map(|drive| vec![drive])
        .or_else(|| multi_drives.cloned())
        .or_else(|| pattern_drive.map(|drive| vec![drive]))
        .unwrap_or_default()
}

/// Check if the query is a full-scan (no filtering).
///
/// A full-scan means all files are returned without filtering,
/// which allows bypassing `SearchResult` allocation in streaming paths.
pub(super) fn is_full_scan_query(filters: &QueryFilters<'_>) -> bool {
    !filters.files_only
        && !filters.dirs_only
        && !filters.hide_system
        && filters.ext_filter.is_none()
        && filters.min_size.is_none()
        && filters.max_size.is_none()
        && filters.limit == 0
        && is_any_match_pattern(filters.parsed.pattern())
}

/// Check if the pattern matches all files (i.e., `*`, `**`, `**/*`, or empty).
pub(super) fn is_any_match_pattern(pattern: &str) -> bool {
    matches!(pattern, "*" | "**" | "**/*" | "")
}

/// Infer a drive letter from an MFT filename.
///
/// If the filename starts with a single ASCII letter followed by a
/// non-letter (e.g., `C.bin`, `C_mft.bin`, `D-drive.mft`), returns
/// that letter uppercased.  Otherwise returns `'X'` as fallback.
pub(super) fn infer_drive_from_filename(path: &std::path::Path) -> char {
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut chars = stem.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii_alphabetic() {
            // Second char must be non-alphabetic (or end of string)
            match chars.next() {
                None | Some('.' | '_' | '-' | ' ') => {
                    return first.to_ascii_uppercase();
                }
                _ => {}
            }
        }
    }
    'X'
}

/// Extract a trailing literal file extension from a glob/regex pattern.
///
/// Returns the extension (without dot) if the pattern ends with a literal
/// `.ext` where `ext` contains no wildcards, dots, or special chars.
///
/// # Examples
/// - `"*.txt"` → `Some("txt")`
/// - `"*hallo*.txt"` → `Some("txt")`
/// - `"foo*.rs"` → `Some("rs")`
/// - `"*.tar.gz"` → `None` (ext contains dot)
/// - `"*hallo*"` → `None` (no extension)
/// - `"nice"` → `None` (no dot)
/// - `"*.tx?"` → `None` (wildcard in ext)
pub(super) fn extract_trailing_extension(pattern: &str) -> Option<&str> {
    // Find the last dot in the pattern.
    let dot_pos = pattern.rfind('.')?;
    let ext = pattern.get(dot_pos + 1..)?;

    // Extension must be non-empty and contain no wildcards or dots.
    if ext.is_empty()
        || ext.contains('*')
        || ext.contains('?')
        || ext.contains('.')
        || ext.contains('[')
    {
        return None;
    }

    Some(ext)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{MAX_CONCURRENT_SEARCH_DRIVE_TASKS, search_drive_task_budget};
    use super::infer_drive_from_filename;

    #[test]
    fn search_drive_task_budget_handles_empty_input() {
        assert_eq!(search_drive_task_budget(0), 0);
    }

    #[test]
    fn search_drive_task_budget_never_exceeds_drive_count() {
        assert_eq!(search_drive_task_budget(1), 1);
        assert!(search_drive_task_budget(3) <= 3);
    }

    #[test]
    fn search_drive_task_budget_caps_drive_fan_out() {
        assert!(
            search_drive_task_budget(MAX_CONCURRENT_SEARCH_DRIVE_TASKS + 8)
                <= MAX_CONCURRENT_SEARCH_DRIVE_TASKS
        );
    }

    #[test]
    fn infer_drive_from_common_mft_filenames() {
        assert_eq!(infer_drive_from_filename(Path::new("C.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("c.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("D_mft.bin")), 'D');
        assert_eq!(infer_drive_from_filename(Path::new("f-drive.mft")), 'F');
        assert_eq!(infer_drive_from_filename(Path::new("G mft.raw")), 'G');
    }

    #[test]
    fn infer_drive_falls_back_to_x_for_ambiguous_names() {
        assert_eq!(infer_drive_from_filename(Path::new("raw.bin")), 'X');
        assert_eq!(infer_drive_from_filename(Path::new("backup_mft.bin")), 'X');
        assert_eq!(infer_drive_from_filename(Path::new("12345.bin")), 'X');
    }

    #[test]
    fn infer_drive_handles_full_paths() {
        assert_eq!(infer_drive_from_filename(Path::new("/tmp/C.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("/data/D_mft.raw")), 'D');
    }
}
