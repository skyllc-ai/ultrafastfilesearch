// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! NTFS attribute bit-flag parsing and expansion.

/// Maps an attribute name to its NTFS bit-flag value.
#[must_use]
pub fn attr_bit(name: &str) -> u32 {
    match name {
        "readonly" | "read-only" | "r" => 0x0001,
        "hidden" | "h" => 0x0002,
        "system" | "s" => 0x0004,
        "directory" | "dir" | "d" => 0x0010,
        "archive" | "a" => 0x0020,
        "device" => 0x0040,
        "normal" => 0x0080,
        "temporary" | "temp" | "t" => 0x0100,
        "sparse" => 0x0200,
        "reparse" => 0x0400,
        "compressed" | "c" => 0x0800,
        "offline" | "o" => 0x1000,
        "notindexed" | "notcontent" | "n" => 0x2000,
        "encrypted" | "e" => 0x4000,
        "integrity" | "i" => 0x8000,
        "virtual" | "v" => 0x0001_0000,
        "noscrub" | "no_scrub_data" | "x" => 0x0002_0000,
        "pinned" | "p" => 0x0008_0000,
        "unpinned" | "u" => 0x0010_0000,
        _ => 0,
    }
}

/// Expand an attribute preset name into its raw spec string.
///
/// Returns `None` if the token is not a known preset.
fn expand_attr_preset(token: &str) -> Option<&'static str> {
    match token {
        "system-files" | "system_files" | "sysfiles" => Some("hidden,system"),
        "user-files" | "user_files" | "userfiles" => Some("!hidden,!system"),
        "compressed-encrypted" | "compressed_encrypted" | "compenc" => Some("compressed,encrypted"),
        _ => None,
    }
}

/// Expand all preset aliases in a comma-separated attribute spec.
///
/// Tokens that are known presets (e.g. `"system-files"`) are replaced with
/// their primitive equivalents.  Other tokens pass through unchanged.
///
/// ```
/// # use uffs_core::search::filters::expand_attr_spec;
/// assert_eq!(expand_attr_spec("system-files"), "hidden,system");
/// assert_eq!(expand_attr_spec("user-files"), "!hidden,!system");
/// assert_eq!(expand_attr_spec("hidden,readonly"), "hidden,readonly");
/// assert_eq!(
///     expand_attr_spec("compressed-encrypted,readonly"),
///     "compressed,encrypted,readonly",
/// );
/// ```
#[must_use]
pub fn expand_attr_spec(spec: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for raw in spec.split(',') {
        let token = raw.trim();
        let lower = token.to_ascii_lowercase();
        // Check for negated presets: "!system-files" → "!hidden,!system"
        if let Some(inner) = lower.strip_prefix('!')
            && let Some(expanded) = expand_attr_preset(inner)
        {
            // Expanded preset may contain its own '!' prefixes.
            for sub in expanded.split(',') {
                out.push(sub);
            }
            continue;
        }
        if let Some(expanded) = expand_attr_preset(&lower) {
            for sub in expanded.split(',') {
                out.push(sub);
            }
        } else {
            out.push(token);
        }
    }
    out.join(",")
}

/// Parse required attribute bits from an attr spec like `"hidden,compressed"`.
///
/// Supports preset aliases: `system-files` → `hidden,system`,
/// `user-files` → `!hidden,!system`.
#[must_use]
pub(crate) fn parse_attr_require(spec: &str) -> u32 {
    let mut bits = 0_u32;
    for raw_part in spec.split(',') {
        let lowered = raw_part.trim().to_ascii_lowercase();
        if lowered.starts_with('!') {
            continue;
        }
        // Check for presets first.
        if let Some(expanded) = expand_attr_preset(&lowered) {
            bits |= parse_attr_require(expanded);
        } else {
            bits |= attr_bit(&lowered);
        }
    }
    bits
}

/// Parse excluded attribute bits from an attr spec like `"!system,!hidden"`.
///
/// Supports preset aliases: `user-files` → `!hidden,!system`.
#[must_use]
pub(crate) fn parse_attr_exclude(spec: &str) -> u32 {
    let mut bits = 0_u32;
    for raw_part in spec.split(',') {
        let lowered = raw_part.trim().to_ascii_lowercase();
        if let Some(name) = lowered.strip_prefix('!') {
            if let Some(expanded) = expand_attr_preset(name) {
                bits |= parse_attr_exclude(expanded);
            } else {
                bits |= attr_bit(name);
            }
        }
        // Also check if the whole token (without !) is a preset that
        // contains exclusion rules.
        if !lowered.starts_with('!')
            && let Some(expanded) = expand_attr_preset(&lowered)
        {
            bits |= parse_attr_exclude(expanded);
        }
    }
    bits
}

// ════════════════════════════════════════════════════════════════════════
// REGRESSION TESTS — Search Filters Parity Guards
//
// These tests verify that SearchFilters.matches_record covers ALL filter
// types.  During the v0.4.30 refactor, 14 filter parameters were not
// wired into the compact search path (they were all passed as None).
// See `docs/architecture/2026_03_30_04_12_SEARCH_PIPELINE_REGRESSION_ANALYSIS.
// md` ════════════════════════════════════════════════════════════════════════
