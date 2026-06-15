// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Drive-prefix splitting for search patterns.
//!
//! The single canonical front door shared by the CLI parse layer
//! (`uffs_client::protocol::cli_args`) and the daemon dispatch safety
//! net (`uffs_core::search::dispatch`), so both agree on what a leading
//! `X:` means.
//!
//! Historically each layer carried its own `parse_bare_drive_prefix`
//! copy, and *neither* understood the bare (`C:`), drive-root (`C:\`),
//! or path-anchored (`C:\Users\*.pdf`) forms — those fell through to a
//! literal substring match (`c:` accidentally matched `.heiC:${…}` ADS
//! streams) or to the tree walker, which mistook the `c:` token for a
//! directory name and matched nothing.  This module is the one place
//! that owns the `DRIVE:` concept, so both entry points agree.
//!
//! ## Rule
//!
//! A leading `X:` (single ASCII letter + colon) is split off into a
//! [`DriveLetter`] filter; the remainder becomes a **drive-relative**
//! body that the normal name/glob/tree/regex classifier then handles:
//!
//! | Pattern | Drive | Body | Downstream meaning |
//! | --- | --- | --- | --- |
//! | `C:` / `C:\` / `C:/` | `C` | `*` | everything on C (match-all) |
//! | `C:report` | `C` | `report` | name substring on C |
//! | `C:\report` | `C` | `report` | name substring on C (same as above) |
//! | `C:rep*` / `C:\*.pdf` | `C` | `rep*` / `*.pdf` | name glob on C |
//! | `C:\report\*` | `C` | `report\*` | tree walk on C |
//! | `\path\*` / `report*` / `>re` | — | — | no drive prefix → `None` |
//!
//! The leading separator is trimmed from the body, so a **single**
//! segment after the drive (`C:\report`, `C:\*.pdf`) collapses to a
//! plain name pattern (and globs correctly), while a **multi**-segment
//! body keeps its internal separators and stays a path pattern that the
//! tree walker picks up via `is_path_pattern`.  Forward and back slashes
//! are equivalent.

use super::drive_letter::DriveLetter;

/// Match-all body returned for the bare / drive-root forms (`C:`,
/// `C:\`, `C:/`).
const MATCH_ALL: &str = "*";

/// Split a leading `X:` drive prefix off a search `pattern`.
///
/// Returns `Some((drive, body))` when `pattern` begins with a single
/// ASCII-alphabetic drive letter followed by `:`.  `body` is the
/// **drive-relative** remainder with any leading path separators
/// trimmed, or `*` (match-all) when nothing usable follows the colon.
/// See the module docs for the full acceptance matrix.
///
/// Returns `None` when there is no `X:` prefix (plain names, path-
/// anchored `\path\…` patterns, and `>`-prefixed regex all fall here),
/// so the caller leaves the pattern untouched.
#[must_use]
pub fn split_drive_prefix(pattern: &str) -> Option<(DriveLetter, &str)> {
    let bytes = pattern.as_bytes();
    let letter = *bytes.first()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if *bytes.get(1)? != b':' {
        return None;
    }
    // Drive-letter + ':' are both ASCII → `rest` starts at byte 2.
    let rest = pattern.get(2..)?;
    // The `is_ascii_alphabetic` guard above guarantees this parses.
    let drive = DriveLetter::parse(letter as char).ok()?;

    let core = rest.trim_start_matches(['\\', '/']);
    let body = if core.is_empty() { MATCH_ALL } else { core };
    Some((drive, body))
}

#[cfg(test)]
mod tests {
    use super::{DriveLetter, split_drive_prefix};

    #[test]
    fn bare_drive_is_match_all() {
        assert_eq!(split_drive_prefix("C:"), Some((DriveLetter::C, "*")));
    }

    #[test]
    fn drive_root_backslash_and_slash_are_match_all() {
        assert_eq!(split_drive_prefix("C:\\"), Some((DriveLetter::C, "*")));
        assert_eq!(split_drive_prefix("C:/"), Some((DriveLetter::C, "*")));
    }

    #[test]
    fn drive_plus_name_is_substring_body() {
        assert_eq!(
            split_drive_prefix("C:report"),
            Some((DriveLetter::C, "report"))
        );
    }

    #[test]
    fn drive_root_single_segment_collapses_to_name() {
        // `C:\report` must behave identically to `C:report`.
        assert_eq!(
            split_drive_prefix("C:\\report"),
            Some((DriveLetter::C, "report"))
        );
    }

    #[test]
    fn drive_root_single_glob_segment_stays_a_name_glob() {
        // The leading separator is trimmed, so the leaf globs instead of
        // hitting the tree walker's single-segment substring path.
        assert_eq!(
            split_drive_prefix("C:\\*.pdf"),
            Some((DriveLetter::C, "*.pdf"))
        );
    }

    #[test]
    fn drive_plus_multi_segment_keeps_path_body() {
        // Internal separator retained → downstream `is_path_pattern`
        // routes this to the tree walker, drive-scoped.
        assert_eq!(
            split_drive_prefix("C:\\report\\*"),
            Some((DriveLetter::C, "report\\*"))
        );
    }

    #[test]
    fn lowercase_letter_is_canonicalised() {
        assert_eq!(split_drive_prefix("d:logs"), Some((DriveLetter::D, "logs")));
    }

    #[test]
    fn no_drive_prefix_returns_none() {
        assert_eq!(split_drive_prefix("report*"), None);
        assert_eq!(split_drive_prefix("\\path\\*"), None);
        assert_eq!(split_drive_prefix("/path/*"), None);
        assert_eq!(split_drive_prefix(">regex"), None);
        assert_eq!(split_drive_prefix("*"), None);
    }

    #[test]
    fn non_alpha_first_byte_returns_none() {
        assert_eq!(split_drive_prefix("12:34"), None);
        assert_eq!(split_drive_prefix(":foo"), None);
    }

    #[test]
    fn single_char_without_colon_returns_none() {
        assert_eq!(split_drive_prefix("C"), None);
    }
}
