// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Pattern compilation and matching for direct `MftIndex` search.
//!
//! Uses [`uffs_text::case_fold::CaseFold`] for NTFS-compatible case-insensitive
//! matching. Pattern strings are pre-folded to `Vec<u16>` at compile time;
//! input strings are folded char-by-char at match time.  This is
//! zero-allocation for the common Exact/Prefix/Suffix/Contains variants.

use regex::Regex;
use uffs_text::case_fold::CaseFold;

use crate::compiled_pattern::{GlobKind, classify_glob};
use crate::error::{CoreError, Result};
use crate::pattern::{ParsedPattern, PatternType};

/// Compiled pattern for direct matching on `MftIndex`.
///
/// This mirrors `CompiledPattern` but generates match functions instead of
/// Polars expressions.  Case-insensitive matching uses NTFS `$UpCase` folding
/// via [`CaseFold`] instead of ASCII-only `to_ascii_lowercase()`.
#[derive(Debug, Clone)]
pub(crate) enum IndexPattern {
    /// Always matches (e.g., `*`).
    Any,

    /// Exact string match.
    Exact {
        /// The exact value to match (case-sensitive).
        value: String,
        /// Pre-folded codepoints for case-insensitive matching.
        folded: Vec<u16>,
    },

    /// Prefix match (e.g., `foo*`).
    Prefix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// Pre-folded codepoints for case-insensitive matching.
        folded: Vec<u16>,
    },

    /// Suffix match (e.g., `*bar`, `*.txt`).
    Suffix {
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Pre-folded codepoints for case-insensitive matching.
        folded: Vec<u16>,
    },

    /// Literal substring match (e.g., `*needle*`).
    Contains {
        /// The substring to search for (case-sensitive).
        needle: String,
        /// Pre-folded codepoints for case-insensitive matching.
        folded: Vec<u16>,
    },

    /// Prefix AND suffix match (e.g., `foo*bar`).
    PrefixSuffix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Pre-folded prefix codepoints.
        prefix_folded: Vec<u16>,
        /// Pre-folded suffix codepoints.
        suffix_folded: Vec<u16>,
    },

    /// Fallback to regex.
    Regex {
        /// Compiled regex for case-sensitive matching.
        regex: Regex,
        /// Compiled regex for case-insensitive matching.
        regex_lower: Regex,
    },

    /// OR: match if ANY sub-pattern matches (e.g., `*.txt|*.log`).
    Or {
        /// Sub-patterns — record matches if any one matches.
        patterns: Vec<Self>,
    },
}

impl IndexPattern {
    /// Check if a string matches this pattern.
    ///
    /// `fold` provides NTFS-compatible case folding for case-insensitive
    /// matching.  All variants use zero-allocation char-by-char fold
    /// comparison (or per-character regex matching).
    #[inline]
    #[must_use]
    pub(crate) fn matches(&self, input: &str, case_sensitive: bool, fold: CaseFold) -> bool {
        match self {
            Self::Any => true,
            Self::Exact { value, folded } => {
                if case_sensitive {
                    input == value
                } else {
                    fold.eq_folded(input, folded)
                }
            }
            Self::Prefix { prefix, folded } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str())
                } else {
                    fold.starts_with_folded(input, folded)
                }
            }
            Self::Suffix { suffix, folded } => {
                if case_sensitive {
                    input.ends_with(suffix.as_str())
                } else {
                    fold.ends_with_folded(input, folded)
                }
            }
            Self::Contains { needle, folded } => {
                if case_sensitive {
                    input.contains(needle.as_str())
                } else {
                    fold.contains_folded(input, folded)
                }
            }
            Self::PrefixSuffix {
                prefix,
                suffix,
                prefix_folded,
                suffix_folded,
            } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str()) && input.ends_with(suffix.as_str())
                } else {
                    fold.starts_with_folded(input, prefix_folded)
                        && fold.ends_with_folded(input, suffix_folded)
                }
            }
            Self::Regex { regex, regex_lower } => {
                if case_sensitive {
                    regex.is_match(input)
                } else {
                    regex_lower.is_match(input)
                }
            }
            Self::Or { patterns } => patterns
                .iter()
                .any(|pat| pat.matches(input, case_sensitive, fold)),
        }
    }
}

/// Compile a glob pattern into an `IndexPattern` with a specific `CaseFold`.
///
/// Internal helper for [`compile_parsed_pattern_with_fold`]; called both
/// directly for the `Glob` pattern type and per-alternative for OR patterns.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
fn compile_index_pattern_with_fold(pattern: &str, fold: CaseFold) -> Result<IndexPattern> {
    let kind = classify_glob(pattern);
    match kind {
        GlobKind::Any => Ok(IndexPattern::Any),
        GlobKind::Exact(value) => {
            let folded = fold.fold_to_u16(&value);
            Ok(IndexPattern::Exact { value, folded })
        }
        GlobKind::Prefix(prefix) => {
            let folded = fold.fold_to_u16(&prefix);
            Ok(IndexPattern::Prefix { prefix, folded })
        }
        GlobKind::Suffix(suffix) => {
            let folded = fold.fold_to_u16(&suffix);
            Ok(IndexPattern::Suffix { suffix, folded })
        }
        GlobKind::Extension(ext) => {
            let suffix = format!(".{ext}");
            let folded = fold.fold_to_u16(&suffix);
            Ok(IndexPattern::Suffix { suffix, folded })
        }
        GlobKind::Contains(needle) => {
            let folded = fold.fold_to_u16(&needle);
            Ok(IndexPattern::Contains { needle, folded })
        }
        GlobKind::PrefixSuffix { prefix, suffix } => {
            let prefix_folded = fold.fold_to_u16(&prefix);
            let suffix_folded = fold.fold_to_u16(&suffix);
            Ok(IndexPattern::PrefixSuffix {
                prefix,
                suffix,
                prefix_folded,
                suffix_folded,
            })
        }
        GlobKind::Complex(glob_pattern) => {
            // globset treats `\` as an escape character, but our patterns use
            // `\` as a Windows path separator.  Convert to `/` so globset
            // interprets them as directory separators.
            let mut globset_pattern = glob_pattern.replace('\\', "/");
            // A leading `/` means "match at any depth" (like Everything),
            // not "anchored at root".  Prepend `**/` so globset allows any
            // prefix before the first segment.
            if globset_pattern.starts_with('/') {
                globset_pattern = format!("**{globset_pattern}");
            }
            let glob =
                globset::Glob::new(&globset_pattern).map_err(|err| CoreError::InvalidGlob {
                    pattern: glob_pattern.clone(),
                    reason: err.to_string(),
                })?;
            let raw_regex = glob.regex();
            // globset emits `(?-u)` which disables Unicode mode — that makes
            // `[/\\]` potentially match invalid UTF-8, which `regex::Regex`
            // rejects.  Our paths are always valid UTF-8, so strip the flag.
            let regex_str = raw_regex.strip_prefix("(?-u)").unwrap_or(raw_regex);
            // globset emits `/` separators in the regex; our paths use `\`.
            // Replace the separator class so the regex matches both.
            let regex_str_win = regex_str.replace('/', r"[/\\]");
            let regex = Regex::new(&regex_str_win).map_err(|err| CoreError::InvalidRegex {
                pattern: regex_str_win.clone(),
                reason: err.to_string(),
            })?;
            let regex_lower = Regex::new(&format!("(?i){regex_str_win}")).map_err(|err| {
                CoreError::InvalidRegex {
                    pattern: regex_str_win,
                    reason: err.to_string(),
                }
            })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
    }
}

/// Compile a `ParsedPattern` into an `IndexPattern`.
///
/// Uses the default `$UpCase` table for pre-folding.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
pub(crate) fn compile_parsed_pattern(parsed: &ParsedPattern) -> Result<IndexPattern> {
    compile_parsed_pattern_with_fold(parsed, CaseFold::default_table())
}

/// Compile a `ParsedPattern` into an `IndexPattern` with a specific `CaseFold`.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
fn compile_parsed_pattern_with_fold(
    parsed: &ParsedPattern,
    fold: CaseFold,
) -> Result<IndexPattern> {
    // OR operator: split on | and compile each part.
    // "*.txt|*.log" → Or([Suffix(".txt"), Suffix(".log")])
    let pat = parsed.pattern();
    if parsed.pattern_type() != PatternType::Regex && pat.contains('|') {
        let parts: Vec<&str> = pat.split('|').collect();
        if parts.len() > 1 {
            let sub_patterns: Result<Vec<IndexPattern>> = parts
                .iter()
                .map(|part| compile_index_pattern_with_fold(part.trim(), fold))
                .collect();
            return Ok(IndexPattern::Or {
                patterns: sub_patterns?,
            });
        }
    }

    match parsed.pattern_type() {
        PatternType::Glob => compile_index_pattern_with_fold(parsed.pattern(), fold),
        PatternType::Regex => {
            let pattern_str = parsed.pattern();
            // Auto-anchor with $ if the pattern isn't already end-anchored.
            // Rust's regex::is_match() does substring matching by default,
            // so >.*\.(jpg|png) would match "icon.png.vir" (finding .png
            // mid-string). Users expect end-of-string matching: the file
            // must END with the extension. Adding $ fixes this to match
            // expected behavior and user intent.
            let anchored = if pattern_str.ends_with('$') {
                pattern_str.to_owned()
            } else {
                format!("{pattern_str}$")
            };
            let regex = Regex::new(&anchored).map_err(|err| CoreError::InvalidRegex {
                pattern: pattern_str.to_owned(),
                reason: err.to_string(),
            })?;
            let regex_lower =
                Regex::new(&format!("(?i){anchored}")).map_err(|err| CoreError::InvalidRegex {
                    pattern: pattern_str.to_owned(),
                    reason: err.to_string(),
                })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
        PatternType::Literal => {
            let needle = parsed.pattern().to_owned();
            let folded = fold.fold_to_u16(&needle);
            Ok(IndexPattern::Contains { needle, folded })
        }
    }
}
