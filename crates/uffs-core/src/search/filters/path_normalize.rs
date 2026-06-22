// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Path-separator normalisation for `path_contains` matching.
//!
//! Extracted out of `filters/mod.rs` (2026-04-21) so the `SearchFilters`
//! module stays under the 800-LOC file-size policy.  The function is
//! a pure helper with no dependencies on any `filters` type, so a
//! sibling submodule is the natural home — the unit tests that
//! exercise it live alongside the other filter tests in
//! `filters/tests.rs`, accessed via `filters::normalize_path_separators`.

/// Normalize path separators for `path_contains` matching.
///
/// 1. Replaces `/` with `\` so users can use either separator.
/// 2. Collapses runs of consecutive `\` into a single `\` — this handles
///    transport layers that double-encode backslashes (JSON `\\` → `\\\\`),
///    producing literal `\\` in the pattern that wouldn't match the single `\`
///    in stored NTFS paths.
pub(in crate::search) fn normalize_path_separators(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut prev_was_sep = false;
    for ch in input.chars() {
        let is_sep = ch == '\\' || ch == '/';
        if is_sep {
            if !prev_was_sep {
                result.push('\\');
            }
            prev_was_sep = true;
        } else {
            result.push(ch);
            prev_was_sep = false;
        }
    }
    result
}

/// Parse a comma-separated `path_excludes` spec into normalized directory
/// globs. Splits on `,`, trims, drops blanks, ASCII-lowercases, and normalizes
/// separators so each entry matches `path_dir()` exactly as `path_contains`
/// does. `None` when the spec is absent or entirely blank.
pub(in crate::search) fn parse_path_excludes(spec: Option<&str>) -> Option<Vec<String>> {
    let entries: Vec<String> = spec?
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| normalize_path_separators(&entry.to_ascii_lowercase()))
        .collect();
    (!entries.is_empty()).then_some(entries)
}
