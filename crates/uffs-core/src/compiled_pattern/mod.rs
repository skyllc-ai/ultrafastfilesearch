//! Compiled pattern IR for optimized Polars expression lowering.
//!
//! This module provides a Pattern Intermediate Representation (IR) that enables
//! specialized Polars string kernels instead of regex-everything approach.
//!
//! # Performance Benefits
//!
//! | Pattern Type | Regex Approach | Optimized Approach | Speedup |
//! |--------------|----------------|-------------------|---------|
//! | `*.txt`      | `contains("^.*\\.txt$")` | `ends_with(".txt")` | 5-10x |
//! | `foo*`       | `contains("^foo.*$")` | `starts_with("foo")` | 5-10x |
//! | `*needle*`   | `contains(".*needle.*")` | `contains_literal("needle")` | 2-5x |
//!
//! # Example
//!
//! ```rust,ignore
//! use uffs_core::compiled_pattern::{CompiledPattern, compile_pattern};
//! use uffs_core::pattern::ParsedPattern;
//!
//! let parsed = ParsedPattern::parse("*.txt")?;
//! let compiled = compile_pattern(&parsed)?;
//!
//! // compiled is CompiledPattern::Suffix(".txt")
//! // which lowers to col("name").str().ends_with(lit(".txt"))
//! let expr = compiled.to_expr("name", true);
//! ```

// Helper functions separated for testability and code clarity
#![expect(
    clippy::single_call_fn,
    reason = "helper functions extracted for testability and code clarity"
)]

use uffs_polars::{Expr, NamedFrom, PlSmallStr, Series, col, lit};

use crate::error::Result;
use crate::pattern::{ParsedPattern, PatternType};

// ============================================================================
// CompiledPattern - The Pattern IR
// ============================================================================

/// Compiled pattern ready for Polars expression lowering.
///
/// Each variant maps to the most efficient Polars expression for that pattern
/// type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledPattern {
    /// Always matches (e.g., `*`).
    /// Lowers to: `lit(true)`
    Any,

    /// Exact string match: `col == "value"`.
    /// Lowers to: `col.eq(lit(value))`
    Exact(String),

    /// Prefix match: `col.str().starts_with("prefix")`.
    /// Lowers to: `col.str().starts_with(lit(prefix))`
    Prefix(String),

    /// Suffix match: `col.str().ends_with("suffix")`.
    /// Lowers to: `col.str().ends_with(lit(suffix))`
    Suffix(String),

    /// Literal substring: `col.str().contains_literal("needle")`.
    /// Lowers to: `col.str().contains_literal(lit(needle))`
    Contains(String),

    /// Prefix AND suffix: `starts_with(p) & ends_with(s)`.
    /// Lowers to:
    /// `col.str().starts_with(lit(p)).and(col.str().ends_with(lit(s)))`
    PrefixSuffix {
        /// The prefix to match.
        prefix: String,
        /// The suffix to match.
        suffix: String,
    },

    /// Multiple exact matches: `col.is_in([...])`.
    /// Lowers to: `col.is_in(lit(series).implode(true), false)` (Polars 2.0+)
    ExactSet(Vec<String>),

    /// Multiple literal substrings: `col.str().contains_any([...])`.
    /// Lowers to: `col.str().contains_any(lit(series).implode(true),
    /// ascii_case_insensitive)` (Polars 2.0+)
    ContainsAny(Vec<String>),

    /// Multiple suffixes (extensions): OR of `ends_with` calls.
    /// Lowers to: `ends_with(s1) | ends_with(s2) | ...`
    SuffixSet(Vec<String>),

    /// Fallback to regex: `col.str().contains(regex, strict)`.
    /// Lowers to: `col.str().contains(lit(pattern), true)`
    Regex {
        /// The regex pattern.
        pattern: String,
        /// Whether the regex is anchored (^...$).
        anchored: bool,
    },
}

// ============================================================================
// Expression Lowering
// ============================================================================

impl CompiledPattern {
    /// Lower this pattern to an optimized Polars expression.
    ///
    /// This method generates the most efficient Polars expression for each
    /// pattern variant, using specialized string kernels where possible.
    ///
    /// # Arguments
    ///
    /// * `column` - The column name to match against (e.g., "name", "path")
    /// * `case_sensitive` - Whether matching should be case-sensitive
    ///
    /// # Returns
    ///
    /// A Polars `Expr` that evaluates to a boolean mask for matching rows.
    ///
    /// # Polars 2.0+ API Notes
    ///
    /// - `is_in` requires `.implode(true)` to wrap values as `List` type
    /// - `contains_any` requires `.implode(true)` for the patterns series
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use uffs_core::compiled_pattern::CompiledPattern;
    ///
    /// let pattern = CompiledPattern::Suffix(".txt".to_string());
    /// let expr = pattern.to_expr("name", true);
    /// // expr is: col("name").str().ends_with(lit(".txt"))
    /// ```
    #[expect(
        clippy::too_many_lines,
        reason = "match arms for each pattern variant are inherently verbose"
    )]
    pub fn to_expr(&self, column: &str, case_sensitive: bool) -> Expr {
        // For case-sensitive matching, use optimized string operations directly.
        // For case-insensitive matching, we use regex with (?i) flag which is
        // more efficient than calling to_lowercase() on every row.
        let col_expr = col(column);

        match self {
            // Any: always matches
            Self::Any => lit(true),

            // Exact: direct equality check (case-sensitive) or regex (case-insensitive)
            Self::Exact(value) => {
                if case_sensitive {
                    col_expr.eq(lit(value.clone()))
                } else {
                    // Use regex for case-insensitive exact match
                    let escaped = regex_escape(value);
                    col_expr
                        .str()
                        .contains(lit(format!("(?i)^{escaped}$")), true)
                }
            }

            // Prefix: starts_with (case-sensitive) or regex (case-insensitive)
            Self::Prefix(prefix) => {
                if case_sensitive {
                    col_expr.str().starts_with(lit(prefix.clone()))
                } else {
                    let escaped = regex_escape(prefix);
                    col_expr
                        .str()
                        .contains(lit(format!("(?i)^{escaped}")), true)
                }
            }

            // Suffix: ends_with (case-sensitive) or regex (case-insensitive)
            Self::Suffix(suffix) => {
                if case_sensitive {
                    col_expr.str().ends_with(lit(suffix.clone()))
                } else {
                    let escaped = regex_escape(suffix);
                    col_expr
                        .str()
                        .contains(lit(format!("(?i){escaped}$")), true)
                }
            }

            // Contains: contains_literal (case-sensitive) or regex (case-insensitive)
            Self::Contains(needle) => {
                if case_sensitive {
                    col_expr.str().contains_literal(lit(needle.clone()))
                } else {
                    let escaped = regex_escape(needle);
                    col_expr.str().contains(lit(format!("(?i){escaped}")), true)
                }
            }

            // PrefixSuffix: combine starts_with AND ends_with
            Self::PrefixSuffix { prefix, suffix } => {
                if case_sensitive {
                    col_expr
                        .clone()
                        .str()
                        .starts_with(lit(prefix.clone()))
                        .and(col_expr.str().ends_with(lit(suffix.clone())))
                } else {
                    let escaped_prefix = regex_escape(prefix);
                    let escaped_suffix = regex_escape(suffix);
                    col_expr.str().contains(
                        lit(format!("(?i)^{escaped_prefix}.*{escaped_suffix}$")),
                        true,
                    )
                }
            }

            // ExactSet: is_in with hash lookup
            // For case-insensitive, we lowercase both column and values
            Self::ExactSet(values) => {
                if case_sensitive {
                    let series = Series::new(PlSmallStr::EMPTY, values);
                    col_expr.is_in(lit(series).implode(true), false)
                } else {
                    // For case-insensitive, lowercase both column and values
                    let lower_values: Vec<String> =
                        values.iter().map(|val| val.to_lowercase()).collect();
                    let series = Series::new(PlSmallStr::EMPTY, &lower_values);
                    col_expr
                        .str()
                        .to_lowercase()
                        .is_in(lit(series).implode(true), false)
                }
            }

            // ContainsAny: Aho-Corasick multi-pattern matching
            Self::ContainsAny(patterns) => {
                if case_sensitive {
                    let series = Series::new(PlSmallStr::EMPTY, patterns);
                    col_expr
                        .str()
                        .contains_any(lit(series).implode(true), false)
                } else {
                    // Use ascii_case_insensitive=true for case-insensitive matching
                    let series = Series::new(PlSmallStr::EMPTY, patterns);
                    col_expr.str().contains_any(lit(series).implode(true), true)
                }
            }

            // SuffixSet: OR of ends_with calls
            Self::SuffixSet(suffixes) => {
                if suffixes.is_empty() {
                    return lit(false);
                }
                if case_sensitive {
                    suffixes
                        .iter()
                        .map(|suf| col_expr.clone().str().ends_with(lit(suf.clone())))
                        .reduce(Expr::or)
                        .unwrap_or_else(|| lit(false))
                } else {
                    // Use regex with alternation for case-insensitive
                    let escaped: Vec<String> =
                        suffixes.iter().map(|suf| regex_escape(suf)).collect();
                    let pattern = format!("(?i)(?:{})$", escaped.join("|"));
                    col_expr.str().contains(lit(pattern), true)
                }
            }

            // Regex: fallback to regex engine
            Self::Regex { pattern, anchored } => {
                let regex_pattern = if *anchored && !pattern.starts_with('^') {
                    format!("^(?:{pattern})$")
                } else if case_sensitive {
                    pattern.clone()
                } else {
                    format!("(?i){pattern}")
                };
                // strict=true means invalid regex is an error
                col_expr.str().contains(lit(regex_pattern), true)
            }
        }
    }
}

/// Escape special regex characters in a string.
#[inline]
fn regex_escape(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    for ch in input.chars() {
        match ch {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

// ============================================================================
// GlobKind - Glob Pattern Classification
// ============================================================================

/// Classification of a glob pattern into its most efficient form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobKind {
    /// `*` - matches everything.
    Any,

    /// No metacharacters - exact match.
    Exact(String),

    /// `foo*` - prefix only.
    Prefix(String),

    /// `*bar` - suffix only.
    Suffix(String),

    /// `*needle*` - contains.
    Contains(String),

    /// `foo*bar` - single star in middle.
    PrefixSuffix {
        /// The prefix before the star.
        prefix: String,
        /// The suffix after the star.
        suffix: String,
    },

    /// `*.ext` - extension pattern (special case of suffix).
    Extension(String),

    /// Complex: `?`, `[]`, `**`, multiple `*` not reducible.
    Complex(String),
}

// ============================================================================
// Classification Functions
// ============================================================================

/// Classify a glob pattern into the most efficient form.
///
/// This function analyzes a glob pattern and determines which specialized
/// Polars operation can be used for matching.
///
/// # Examples
///
/// ```rust,ignore
/// use uffs_core::compiled_pattern::classify_glob;
///
/// assert!(matches!(classify_glob("*"), GlobKind::Any));
/// assert!(matches!(classify_glob("foo*"), GlobKind::Prefix(_)));
/// assert!(matches!(classify_glob("*.txt"), GlobKind::Extension(_)));
/// ```
#[must_use]
pub fn classify_glob(pattern: &str) -> GlobKind {
    // Check for metacharacters
    let has_star = pattern.contains('*');
    let has_question = pattern.contains('?');
    let has_bracket = pattern.contains('[');
    let has_double_star = pattern.contains("**");

    // No metacharacters = exact match
    if !has_star && !has_question && !has_bracket {
        return GlobKind::Exact(pattern.to_owned());
    }

    // Complex patterns (?, [], **)
    if has_question || has_bracket || has_double_star {
        return GlobKind::Complex(pattern.to_owned());
    }

    // Single star patterns
    classify_single_star_pattern(pattern)
}

/// Classify patterns with only single `*` characters (no `?`, `[]`, or `**`).
fn classify_single_star_pattern(pattern: &str) -> GlobKind {
    let star_count = pattern.matches('*').count();

    match star_count {
        1 => classify_one_star(pattern),
        2 if pattern.starts_with('*') && pattern.ends_with('*') => {
            // *needle* with exactly 2 stars at boundaries
            // Safe: we know pattern starts and ends with '*' (ASCII)
            let needle = pattern
                .strip_prefix('*')
                .and_then(|rest| rest.strip_suffix('*'))
                .unwrap_or("");
            if needle.contains('*') {
                GlobKind::Complex(pattern.to_owned())
            } else {
                GlobKind::Contains(needle.to_owned())
            }
        }
        _ => GlobKind::Complex(pattern.to_owned()),
    }
}

/// Classify patterns with exactly one `*`.
fn classify_one_star(pattern: &str) -> GlobKind {
    if pattern == "*" {
        return GlobKind::Any;
    }

    pattern.strip_prefix('*').map_or_else(
        || {
            // No leading '*', check for trailing '*'
            pattern.strip_suffix('*').map_or_else(
                || {
                    // prefix*suffix → PrefixSuffix
                    // Safe: we know there's exactly one '*' in the pattern
                    let (prefix, suffix) = pattern.split_once('*').unwrap_or((pattern, ""));
                    GlobKind::PrefixSuffix {
                        prefix: prefix.to_owned(),
                        suffix: suffix.to_owned(),
                    }
                },
                |prefix| {
                    // prefix* → Prefix
                    GlobKind::Prefix(prefix.to_owned())
                },
            )
        },
        |suffix| {
            // *suffix → Suffix or Extension
            extract_extension(suffix).map_or_else(
                || GlobKind::Suffix(suffix.to_owned()),
                |ext| GlobKind::Extension(ext.to_owned()),
            )
        },
    )
}

/// Extract extension from a suffix pattern (e.g., `.txt` → `txt`).
///
/// Returns `Some(ext)` if the suffix is a simple extension pattern:
/// - Starts with a dot
/// - Has no other dots after the first one
/// - Has at least one character after the dot
fn extract_extension(suffix: &str) -> Option<&str> {
    let after_dot = suffix.strip_prefix('.')?;
    (!after_dot.is_empty() && !after_dot.contains('.')).then_some(after_dot)
}

// ============================================================================
// Pattern Compilation
// ============================================================================

/// Compile a `ParsedPattern` into a `CompiledPattern` for optimized matching.
///
/// This function analyzes the pattern and produces the most efficient IR
/// representation for Polars expression lowering.
///
/// # Errors
///
/// Returns an error if glob-to-regex conversion fails for complex patterns.
///
/// # Examples
///
/// ```rust,ignore
/// use uffs_core::pattern::ParsedPattern;
/// use uffs_core::compiled_pattern::compile_pattern;
///
/// let parsed = ParsedPattern::parse("*.txt")?;
/// let compiled = compile_pattern(&parsed)?;
/// // compiled is CompiledPattern::Suffix(".txt")
/// ```
pub fn compile_pattern(parsed: &ParsedPattern) -> Result<CompiledPattern> {
    match parsed.pattern_type() {
        PatternType::Regex => Ok(CompiledPattern::Regex {
            pattern: parsed.pattern().to_owned(),
            anchored: false,
        }),
        PatternType::Literal => Ok(CompiledPattern::Contains(parsed.pattern().to_owned())),
        PatternType::Glob => compile_glob(parsed.pattern()),
    }
}

/// Compile a glob pattern into a `CompiledPattern`.
fn compile_glob(glob_pattern: &str) -> Result<CompiledPattern> {
    let kind = classify_glob(glob_pattern);

    match kind {
        GlobKind::Any => Ok(CompiledPattern::Any),
        GlobKind::Exact(exact) => Ok(CompiledPattern::Exact(exact)),
        GlobKind::Prefix(prefix) => Ok(CompiledPattern::Prefix(prefix)),
        GlobKind::Suffix(suffix) => Ok(CompiledPattern::Suffix(suffix)),
        GlobKind::Contains(needle) => Ok(CompiledPattern::Contains(needle)),
        GlobKind::PrefixSuffix { prefix, suffix } => {
            Ok(CompiledPattern::PrefixSuffix { prefix, suffix })
        }
        GlobKind::Extension(ext) => {
            // Extension becomes a suffix with the dot
            Ok(CompiledPattern::Suffix(format!(".{ext}")))
        }
        GlobKind::Complex(complex_pattern) => {
            // Fall back to regex for complex patterns
            let regex = crate::glob::glob_to_regex(&complex_pattern)?;
            Ok(CompiledPattern::Regex {
                pattern: regex,
                anchored: true,
            })
        }
    }
}

#[cfg(test)]
mod tests;
