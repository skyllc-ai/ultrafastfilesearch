//! Pattern compilation and matching for direct `MftIndex` search.

use std::collections::HashSet;

use aho_corasick::AhoCorasick;
use regex::Regex;

use crate::compiled_pattern::{GlobKind, classify_glob};
use crate::error::{CoreError, Result};
use crate::pattern::{ParsedPattern, PatternType};

/// Compiled pattern for direct matching on `MftIndex`.
///
/// This mirrors `CompiledPattern` but generates match functions instead of
/// Polars expressions. Uses SIMD-optimized string matching where possible.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum IndexPattern {
    /// Always matches (e.g., `*`).
    Any,

    /// Exact string match.
    Exact {
        /// The exact value to match (case-sensitive).
        value: String,
        /// Lowercase version for case-insensitive matching.
        value_lower: String,
    },

    /// Prefix match (e.g., `foo*`).
    Prefix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// Lowercase version for case-insensitive matching.
        prefix_lower: String,
    },

    /// Suffix match (e.g., `*bar`, `*.txt`).
    Suffix {
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Lowercase version for case-insensitive matching.
        suffix_lower: String,
    },

    /// Literal substring match (e.g., `*needle*`).
    Contains {
        /// The substring to search for (case-sensitive).
        needle: String,
        /// Lowercase version for case-insensitive matching.
        needle_lower: String,
    },

    /// Prefix AND suffix match (e.g., `foo*bar`).
    PrefixSuffix {
        /// The prefix to match (case-sensitive).
        prefix: String,
        /// The suffix to match (case-sensitive).
        suffix: String,
        /// Lowercase prefix for case-insensitive matching.
        prefix_lower: String,
        /// Lowercase suffix for case-insensitive matching.
        suffix_lower: String,
    },

    /// Multiple exact matches (hash set lookup).
    ExactSet {
        /// Set of exact values (case-sensitive).
        values: HashSet<String>,
        /// Lowercase set for case-insensitive matching.
        values_lower: HashSet<String>,
    },

    /// Multiple suffix matches (e.g., extensions).
    SuffixSet {
        /// List of suffixes (case-sensitive).
        suffixes: Vec<String>,
        /// Lowercase suffixes for case-insensitive matching.
        suffixes_lower: Vec<String>,
    },

    /// Multiple literal substrings (Aho-Corasick).
    ContainsAny {
        /// Aho-Corasick automaton for case-sensitive matching.
        automaton: AhoCorasick,
        /// Aho-Corasick automaton for case-insensitive matching.
        automaton_lower: AhoCorasick,
        /// Original patterns for debugging.
        patterns: Vec<String>,
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
    /// Case-insensitive variants use zero-allocation byte-level comparison
    /// instead of `.to_ascii_lowercase()` which allocates a new `String` per
    /// call.  For 8M records this eliminates 8M heap allocations.
    #[inline]
    #[must_use]
    pub fn matches(&self, input: &str, case_sensitive: bool) -> bool {
        match self {
            Self::Any => true,
            Self::Exact { value, value_lower } => {
                if case_sensitive {
                    input == value
                } else {
                    input.eq_ignore_ascii_case(value_lower)
                }
            }
            Self::Prefix {
                prefix,
                prefix_lower,
            } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str())
                } else {
                    starts_with_ignore_ascii_case(input, prefix_lower)
                }
            }
            Self::Suffix {
                suffix,
                suffix_lower,
            } => {
                if case_sensitive {
                    input.ends_with(suffix.as_str())
                } else {
                    ends_with_ignore_ascii_case(input, suffix_lower)
                }
            }
            Self::Contains {
                needle,
                needle_lower,
            } => {
                if case_sensitive {
                    input.contains(needle.as_str())
                } else {
                    contains_ignore_ascii_case(input, needle_lower)
                }
            }
            Self::PrefixSuffix {
                prefix,
                suffix,
                prefix_lower,
                suffix_lower,
            } => {
                if case_sensitive {
                    input.starts_with(prefix.as_str()) && input.ends_with(suffix.as_str())
                } else {
                    starts_with_ignore_ascii_case(input, prefix_lower)
                        && ends_with_ignore_ascii_case(input, suffix_lower)
                }
            }
            Self::ExactSet {
                values,
                values_lower,
            } => {
                if case_sensitive {
                    values.contains(input)
                } else {
                    // HashSet lookup requires an owned key — unavoidable alloc.
                    // But ExactSet is rare (multi-value exact match).
                    values_lower.contains(&input.to_ascii_lowercase())
                }
            }
            Self::SuffixSet {
                suffixes,
                suffixes_lower,
            } => {
                if case_sensitive {
                    suffixes.iter().any(|suf| input.ends_with(suf.as_str()))
                } else {
                    suffixes_lower
                        .iter()
                        .any(|suf| ends_with_ignore_ascii_case(input, suf))
                }
            }
            Self::ContainsAny {
                automaton,
                automaton_lower,
                ..
            } => {
                if case_sensitive {
                    automaton.is_match(input)
                } else {
                    // Aho-Corasick requires the input pre-lowercased — unavoidable.
                    // But ContainsAny is only used for multi-substring patterns.
                    automaton_lower.is_match(&input.to_ascii_lowercase())
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
                .any(|pat| pat.matches(input, case_sensitive)),
        }
    }
}

// ── Zero-allocation ASCII case-insensitive helpers ──────────────────────
//
// These replace `.to_ascii_lowercase().starts_with(...)` etc. which allocate
// a new String on every call.  For 8M records per query, this eliminates
// 8M heap allocations.

/// Check if `haystack` starts with `needle` (ASCII case-insensitive).
///
/// `needle` must already be lowercase.
#[inline]
fn starts_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let hb = haystack.as_bytes();
    let nb = needle.as_bytes();
    if hb.len() < nb.len() {
        return false;
    }
    hb.iter()
        .zip(nb)
        .all(|(hay, ndl)| hay.to_ascii_lowercase() == *ndl)
}

/// Check if `haystack` ends with `needle` (ASCII case-insensitive).
///
/// `needle` must already be lowercase.
#[inline]
fn ends_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let hb = haystack.as_bytes();
    let nb = needle.as_bytes();
    if hb.len() < nb.len() {
        return false;
    }
    let start = hb.len() - nb.len();
    hb.iter()
        .skip(start)
        .zip(nb)
        .all(|(hay, ndl)| hay.to_ascii_lowercase() == *ndl)
}

/// Check if `haystack` contains `needle` (ASCII case-insensitive).
///
/// `needle` must already be lowercase.  Uses a simple sliding-window
/// approach.  For short needles (typical filenames), this is faster than
/// allocating a lowercased copy of the entire haystack.
#[expect(
    clippy::single_call_fn,
    reason = "extracted for readability — contains is semantically distinct from starts_with/ends_with"
)]
#[inline]
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hb = haystack.as_bytes();
    let nb = needle.as_bytes();
    if hb.len() < nb.len() {
        return false;
    }
    let Some(&first) = nb.first() else {
        return true;
    };

    // Sliding window: find candidate positions where first byte matches.
    for start in 0..=hb.len() - nb.len() {
        if let Some(hay_byte) = hb.get(start) {
            if hay_byte.to_ascii_lowercase() == first
                && hb.get(start..start + nb.len()).is_some_and(|window| {
                    window
                        .iter()
                        .zip(nb)
                        .all(|(hay, ndl)| hay.to_ascii_lowercase() == *ndl)
                })
            {
                return true;
            }
        }
    }
    false
}

/// Compile a glob pattern into an `IndexPattern`.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
pub fn compile_index_pattern(pattern: &str) -> Result<IndexPattern> {
    let kind = classify_glob(pattern);
    match kind {
        GlobKind::Any => Ok(IndexPattern::Any),
        GlobKind::Exact(value) => {
            let value_lower = value.to_ascii_lowercase();
            Ok(IndexPattern::Exact { value, value_lower })
        }
        GlobKind::Prefix(prefix) => {
            let prefix_lower = prefix.to_ascii_lowercase();
            Ok(IndexPattern::Prefix {
                prefix,
                prefix_lower,
            })
        }
        GlobKind::Suffix(suffix) => {
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::Suffix {
                suffix,
                suffix_lower,
            })
        }
        GlobKind::Extension(ext) => {
            let suffix = format!(".{ext}");
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::Suffix {
                suffix,
                suffix_lower,
            })
        }
        GlobKind::Contains(needle) => {
            let needle_lower = needle.to_ascii_lowercase();
            Ok(IndexPattern::Contains {
                needle,
                needle_lower,
            })
        }
        GlobKind::PrefixSuffix { prefix, suffix } => {
            let prefix_lower = prefix.to_ascii_lowercase();
            let suffix_lower = suffix.to_ascii_lowercase();
            Ok(IndexPattern::PrefixSuffix {
                prefix,
                suffix,
                prefix_lower,
                suffix_lower,
            })
        }
        GlobKind::Complex(glob_pattern) => {
            let glob = globset::Glob::new(&glob_pattern).map_err(|err| CoreError::InvalidGlob {
                pattern: glob_pattern.clone(),
                reason: err.to_string(),
            })?;
            let regex_str = glob.regex();
            let regex = Regex::new(regex_str).map_err(|err| CoreError::InvalidRegex {
                pattern: regex_str.to_owned(),
                reason: err.to_string(),
            })?;
            let regex_lower =
                Regex::new(&format!("(?i){regex_str}")).map_err(|err| CoreError::InvalidRegex {
                    pattern: regex_str.to_owned(),
                    reason: err.to_string(),
                })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
    }
}

/// Compile a `ParsedPattern` into an `IndexPattern`.
///
/// # Errors
///
/// Returns an error if the pattern is invalid (e.g., malformed glob or regex).
pub fn compile_parsed_pattern(parsed: &ParsedPattern) -> Result<IndexPattern> {
    // OR operator: split on | and compile each part.
    // "*.txt|*.log" → Or([Suffix(".txt"), Suffix(".log")])
    let pat = parsed.pattern();
    if parsed.pattern_type() != PatternType::Regex && pat.contains('|') {
        let parts: Vec<&str> = pat.split('|').collect();
        if parts.len() > 1 {
            let sub_patterns: Result<Vec<IndexPattern>> = parts
                .iter()
                .map(|part| compile_index_pattern(part.trim()))
                .collect();
            return Ok(IndexPattern::Or {
                patterns: sub_patterns?,
            });
        }
    }

    match parsed.pattern_type() {
        PatternType::Glob => compile_index_pattern(parsed.pattern()),
        PatternType::Regex => {
            let pattern_str = parsed.pattern();
            // Auto-anchor with $ if the pattern isn't already end-anchored.
            // Rust's regex::is_match() does substring matching by default,
            // so >.*\.(jpg|png) would match "icon.png.vir" (finding .png
            // mid-string). Users expect end-of-string matching: the file
            // must END with the extension. Adding $ fixes this to match
            // C++ behavior and user intent.
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
            // Bare text = substring match (like Everything, WizFile, C++ UFFS).
            // "nice" finds "nicehouse", "my_nice_file.txt", "venice.jpg".
            // Combined with is_path_pattern, this also matches against full
            // paths — so "AppData" finds "C:\Users\john\AppData\Local\".
            let needle = parsed.pattern().to_owned();
            let needle_lower = needle.to_ascii_lowercase();
            Ok(IndexPattern::Contains {
                needle,
                needle_lower,
            })
        }
    }
}

/// Compile multiple extension patterns into a `SuffixSet`.
#[must_use]
pub fn compile_extensions(extensions: &[&str]) -> IndexPattern {
    let suffixes: Vec<String> = extensions
        .iter()
        .map(|ext| {
            if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            }
        })
        .collect();
    let suffixes_lower: Vec<String> = suffixes
        .iter()
        .map(|suf| suf.to_ascii_lowercase())
        .collect();
    IndexPattern::SuffixSet {
        suffixes,
        suffixes_lower,
    }
}
