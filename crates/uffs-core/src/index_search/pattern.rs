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
}

impl IndexPattern {
    /// Check if a string matches this pattern.
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
                    input
                        .to_ascii_lowercase()
                        .starts_with(prefix_lower.as_str())
                }
            }
            Self::Suffix {
                suffix,
                suffix_lower,
            } => {
                if case_sensitive {
                    input.ends_with(suffix.as_str())
                } else {
                    input.to_ascii_lowercase().ends_with(suffix_lower.as_str())
                }
            }
            Self::Contains {
                needle,
                needle_lower,
            } => {
                if case_sensitive {
                    input.contains(needle.as_str())
                } else {
                    input.to_ascii_lowercase().contains(needle_lower.as_str())
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
                    let lower = input.to_ascii_lowercase();
                    lower.starts_with(prefix_lower.as_str())
                        && lower.ends_with(suffix_lower.as_str())
                }
            }
            Self::ExactSet {
                values,
                values_lower,
            } => {
                if case_sensitive {
                    values.contains(input)
                } else {
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
                    let lower = input.to_ascii_lowercase();
                    suffixes_lower
                        .iter()
                        .any(|suf| lower.ends_with(suf.as_str()))
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
                    automaton_lower.is_match(&input.to_ascii_lowercase())
                }
            }
            Self::Regex { regex, regex_lower } => {
                if case_sensitive {
                    regex.is_match(input)
                } else {
                    regex_lower.is_match(&input.to_ascii_lowercase())
                }
            }
        }
    }
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
    match parsed.pattern_type() {
        PatternType::Glob => compile_index_pattern(parsed.pattern()),
        PatternType::Regex => {
            let pattern_str = parsed.pattern();
            let regex = Regex::new(pattern_str).map_err(|err| CoreError::InvalidRegex {
                pattern: pattern_str.to_owned(),
                reason: err.to_string(),
            })?;
            let regex_lower = Regex::new(&format!("(?i){pattern_str}")).map_err(|err| {
                CoreError::InvalidRegex {
                    pattern: pattern_str.to_owned(),
                    reason: err.to_string(),
                }
            })?;
            Ok(IndexPattern::Regex { regex, regex_lower })
        }
        PatternType::Literal => {
            let value = parsed.pattern().to_owned();
            let value_lower = value.to_ascii_lowercase();
            Ok(IndexPattern::Exact { value, value_lower })
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
