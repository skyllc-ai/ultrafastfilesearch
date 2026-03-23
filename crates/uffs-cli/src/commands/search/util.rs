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

/// Extract literal file extensions from a regex pattern's trailing alternation.
///
/// Parses patterns like `>.*\.(jpg|png|heic)` and returns `["jpg", "png",
/// "heic"]`. This enables the extension index to pre-filter records before
/// applying the full regex — turning O(n) scans into O(matches).
///
/// Returns `None` if the pattern doesn't end with a recognizable extension
/// alternation, or if any extension contains regex metacharacters.
///
/// # Supported patterns
///
/// - `>.*\.(jpg|png|heic)` → `["jpg", "png", "heic"]`
/// - `>C:\\Users\\.*\.(jpg|png|heic)` → `["jpg", "png", "heic"]`
/// - `>.*\.txt` → `["txt"]` (single extension, no alternation)
/// - `>.*\.(jpg|png)$` → `["jpg", "png"]` (trailing `$` anchor stripped)
/// - `>.*\.(tar\.gz|zip)` → `None` (dot inside alternation)
/// - `>.*\.(jp.?)` → `None` (metacharacter inside alternation)
pub(super) fn extract_extensions_from_regex(pattern: &str) -> Option<Vec<String>> {
    // Must be a regex pattern (starts with >).
    let regex_body = pattern.strip_prefix('>')?;

    // Strip trailing $ anchor if present.
    let body = regex_body.strip_suffix('$').unwrap_or(regex_body);

    // Find the last `\.` (escaped dot) — this is the extension separator.
    let escaped_dot_pos = body.rfind("\\.")?;
    let after_dot = body.get(escaped_dot_pos + 2..)?;

    if after_dot.is_empty() {
        return None;
    }

    // Case 1: Alternation group `(ext1|ext2|ext3)`
    if let Some(inner) = after_dot
        .strip_prefix('(')
        .and_then(|inner_str| inner_str.strip_suffix(')'))
    {
        let extensions: Vec<String> = inner
            .split('|')
            .map(|name| name.trim().to_ascii_lowercase())
            .collect();

        // Validate: every extension must be pure alphanumeric (no metacharacters).
        if extensions.is_empty()
            || extensions
                .iter()
                .any(|name| name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric()))
        {
            return None;
        }

        return Some(extensions);
    }

    // Case 2: Single literal extension (no alternation) e.g. `>.*\.txt`
    if !after_dot.is_empty() && after_dot.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Some(vec![after_dot.to_ascii_lowercase()]);
    }

    None
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

    // ── extract_trailing_extension ──────────────────────────────────────

    use super::extract_trailing_extension;

    #[test]
    fn glob_single_extension() {
        assert_eq!(extract_trailing_extension("*.txt"), Some("txt"));
        assert_eq!(extract_trailing_extension("*.rs"), Some("rs"));
        assert_eq!(extract_trailing_extension("foo*.rs"), Some("rs"));
    }

    #[test]
    fn glob_extension_with_prefix_wildcards() {
        assert_eq!(extract_trailing_extension("*hallo*.txt"), Some("txt"));
        assert_eq!(extract_trailing_extension("**/*.rs"), Some("rs"));
    }

    #[test]
    fn glob_extracts_last_extension_from_multi_dot() {
        // *.tar.gz → the LAST extension is "gz" (last dot wins).
        // This matches NTFS behavior where extension = everything after the last dot.
        assert_eq!(extract_trailing_extension("*.tar.gz"), Some("gz"));
    }

    #[test]
    fn glob_rejects_wildcards_in_extension() {
        assert_eq!(extract_trailing_extension("*.tx?"), None);
        assert_eq!(extract_trailing_extension("*.t*"), None);
        assert_eq!(extract_trailing_extension("*.[ch]"), None);
    }

    #[test]
    fn glob_rejects_no_extension() {
        assert_eq!(extract_trailing_extension("*hallo*"), None);
        assert_eq!(extract_trailing_extension("nice"), None);
        assert_eq!(extract_trailing_extension(""), None);
    }

    // ── extract_extensions_from_regex ────────────────────────────────────

    use super::extract_extensions_from_regex;

    #[test]
    fn regex_alternation_extracts_multiple_extensions() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.(jpg|png|heic)"),
            Some(vec!["jpg".into(), "png".into(), "heic".into()])
        );
    }

    #[test]
    fn regex_alternation_with_path_prefix() {
        assert_eq!(
            extract_extensions_from_regex(r">C:\\Users\\.*\.(jpg|png|heic)"),
            Some(vec!["jpg".into(), "png".into(), "heic".into()])
        );
    }

    #[test]
    fn regex_alternation_with_dollar_anchor() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.(jpg|png|gif)$"),
            Some(vec!["jpg".into(), "png".into(), "gif".into()])
        );
    }

    #[test]
    fn regex_single_extension_no_alternation() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.txt"),
            Some(vec!["txt".into()])
        );
    }

    #[test]
    fn regex_single_extension_with_anchor() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.pdf$"),
            Some(vec!["pdf".into()])
        );
    }

    #[test]
    fn regex_normalizes_to_lowercase() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.(JPG|PNG)"),
            Some(vec!["jpg".into(), "png".into()])
        );
        assert_eq!(
            extract_extensions_from_regex(r">.*\.TXT"),
            Some(vec!["txt".into()])
        );
    }

    #[test]
    fn regex_rejects_dot_inside_alternation() {
        // tar.gz contains a dot — not a pure extension
        assert_eq!(extract_extensions_from_regex(r">.*\.(tar\.gz|zip)"), None);
    }

    #[test]
    fn regex_rejects_metacharacters_in_alternation() {
        assert_eq!(extract_extensions_from_regex(r">.*\.(jp.?)"), None);
        assert_eq!(extract_extensions_from_regex(r">.*\.(j.*|png)"), None);
        assert_eq!(extract_extensions_from_regex(r">.*\.(jpg|pn[gG])"), None);
    }

    #[test]
    fn regex_rejects_empty_alternation_arms() {
        assert_eq!(extract_extensions_from_regex(r">.*\.(|jpg)"), None);
        assert_eq!(extract_extensions_from_regex(r">.*\.(jpg|)"), None);
    }

    #[test]
    fn regex_rejects_non_regex_patterns() {
        // Not a regex (no > prefix)
        assert_eq!(extract_extensions_from_regex("*.txt"), None);
        assert_eq!(extract_extensions_from_regex("hello"), None);
    }

    #[test]
    fn regex_rejects_no_escaped_dot() {
        // Uses plain dot instead of \. — could match any character
        assert_eq!(extract_extensions_from_regex(">.*.(jpg|png)"), None);
    }

    #[test]
    fn regex_rejects_empty_after_dot() {
        assert_eq!(extract_extensions_from_regex(r">.*\."), None);
    }

    #[test]
    fn regex_two_extensions_is_common_case() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.(doc|docx)"),
            Some(vec!["doc".into(), "docx".into()])
        );
    }

    #[test]
    fn regex_many_extensions() {
        assert_eq!(
            extract_extensions_from_regex(r">.*\.(jpg|jpeg|png|gif|bmp|webp|svg|tiff)"),
            Some(vec![
                "jpg".into(),
                "jpeg".into(),
                "png".into(),
                "gif".into(),
                "bmp".into(),
                "webp".into(),
                "svg".into(),
                "tiff".into()
            ])
        );
    }
}
