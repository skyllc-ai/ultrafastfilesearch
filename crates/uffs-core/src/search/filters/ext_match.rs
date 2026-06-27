// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Extension-matching helpers shared by the filter fallback and the
//! sort-key builder.
//!
//! Extracted out of `filters/mod.rs` (2026-04-21) so the parent module
//! stays under the 800-LOC file-size policy.  All three helpers are
//! pure functions with no `SearchFilters` dependency, so grouping them
//! here keeps the extension-handling contract in one place:
//!
//! - [`lowercase_into`] — zero-alloc lowercase into a reusable buffer.
//! - [`extension_matches_filter`] — mixed-case-safe equality for `--ext` tokens
//!   against a normalized extension.
//! - [`extract_extension_after_dot`] — dot-gated extractor matching
//!   `uffs_mft::index::base::MftIndex::intern_extension` semantics (dotless /
//!   dotfile / trailing-dot all return `""`).

/// Lowercase a string into a reusable UTF-8 buffer and return the borrowed
/// string view.
pub(in crate::search) fn lowercase_into<'a>(input: &str, buf: &'a mut Vec<u8>) -> &'a str {
    buf.clear();
    for ch in input.chars() {
        for lower in ch.to_lowercase() {
            let mut char_buf = [0_u8; 4];
            let encoded = lower.encode_utf8(&mut char_buf);
            buf.extend_from_slice(encoded.as_bytes());
        }
    }
    core::str::from_utf8(buf.as_slice()).map_or("", |lowered| lowered)
}

/// Return `true` if a normalized extension matches an allowed filter token.
///
/// The fast/common path compares already-lowercased strings directly. The
/// fallback branch keeps manual test fixtures and any direct struct
/// construction robust if a caller supplied mixed-case extension tokens.
#[must_use]
pub(in crate::search) fn extension_matches_filter(
    allowed: &str,
    normalized_extension: &str,
) -> bool {
    allowed == normalized_extension || allowed.to_lowercase() == normalized_extension
}

/// Extract the filename's extension using the same rules as
/// [`uffs_mft::index::MftIndex::intern_extension`]:
///
/// - Dotless names (e.g. `dbt`, `README`) have no extension.
/// - Hidden files (e.g. `.gitignore`) have no extension.
/// - Trailing-dot names (e.g. `foo.`) have no extension.
/// - Otherwise, the extension is the slice after the last `.`.
///
/// Returning `""` for dotless names keeps this helper aligned with the
/// MFT indexer's `extension_id = 0` assignment, so the fallback
/// `matches_record` path (used when `resolve_ext_ids_for_drive` wasn't
/// called by the caller) reports the same match set as the fast path
/// that compares pre-resolved `extension_id` values.
///
/// Regression pin (2026-04-21): the previous `name.rsplit('.').next().
/// unwrap_or("")` returned the whole name for dotless inputs, so a
/// directory literally named `dbt` matched `--ext dbt` even though the
/// indexer assigned it `extension_id = 0`.  See the `C:ext_rare` row
/// in `@/Users/rnio/Private/Github/UltraFastFileSearch/LOG/Output_cache_new:
/// 323,785`.
#[must_use]
#[inline]
pub(crate) fn extract_extension_after_dot(filename: &str) -> &str {
    let Some(dot_pos) = filename.rfind('.') else {
        return "";
    };
    // Exclude ".gitignore"-style hidden files (dot at start) and "foo."
    // (dot at end).  `filename.get(...)` preserves UTF-8 boundaries.
    if dot_pos == 0 || dot_pos >= filename.len() - 1 {
        return "";
    }
    filename.get(dot_pos + 1..).unwrap_or("")
}
