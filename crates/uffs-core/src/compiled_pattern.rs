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

#![allow(clippy::single_call_fn)]

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
    #[allow(clippy::too_many_lines)]
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
                    col_expr.str().contains_any(lit(series).implode(true), false)
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    // ========================================================================
    // GlobKind Classification Tests
    // ========================================================================

    #[test]
    fn test_classify_any() {
        assert!(matches!(classify_glob("*"), GlobKind::Any));
    }

    #[test]
    fn test_classify_exact() {
        let kind = classify_glob("readme.txt");
        assert!(matches!(kind, GlobKind::Exact(val) if val == "readme.txt"));
    }

    #[test]
    fn test_classify_prefix() {
        let kind = classify_glob("foo*");
        assert!(matches!(kind, GlobKind::Prefix(val) if val == "foo"));
    }

    #[test]
    fn test_classify_suffix() {
        let kind = classify_glob("*bar");
        assert!(matches!(kind, GlobKind::Suffix(val) if val == "bar"));
    }

    #[test]
    fn test_classify_extension() {
        let kind = classify_glob("*.txt");
        assert!(matches!(kind, GlobKind::Extension(val) if val == "txt"));
    }

    #[test]
    fn test_classify_extension_multi_part() {
        // *.tar.gz should be Suffix, not Extension (has multiple dots)
        let kind = classify_glob("*.tar.gz");
        assert!(matches!(kind, GlobKind::Suffix(val) if val == ".tar.gz"));
    }

    #[test]
    fn test_classify_contains() {
        let kind = classify_glob("*needle*");
        assert!(matches!(kind, GlobKind::Contains(val) if val == "needle"));
    }

    #[test]
    fn test_classify_prefix_suffix() {
        let kind = classify_glob("foo*bar");
        assert!(
            matches!(kind, GlobKind::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == "bar")
        );
    }

    #[test]
    fn test_classify_prefix_suffix_with_extension() {
        let kind = classify_glob("foo*.txt");
        assert!(
            matches!(kind, GlobKind::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == ".txt")
        );
    }

    #[test]
    fn test_classify_complex_question_mark() {
        let kind = classify_glob("file?.txt");
        assert!(matches!(kind, GlobKind::Complex(_)));
    }

    #[test]
    fn test_classify_complex_bracket() {
        let kind = classify_glob("[abc]*");
        assert!(matches!(kind, GlobKind::Complex(_)));
    }

    #[test]
    fn test_classify_complex_double_star() {
        let kind = classify_glob("**/*.rs");
        assert!(matches!(kind, GlobKind::Complex(_)));
    }

    #[test]
    fn test_classify_complex_multiple_stars() {
        let kind = classify_glob("a*b*c");
        assert!(matches!(kind, GlobKind::Complex(_)));
    }

    // ========================================================================
    // CompiledPattern Compilation Tests
    // ========================================================================

    #[test]
    fn test_compile_literal() -> TestResult {
        let parsed = ParsedPattern::parse("readme")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "readme"));
        Ok(())
    }

    #[test]
    fn test_compile_glob_any() -> TestResult {
        let parsed = ParsedPattern::parse("*")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Any));
        Ok(())
    }

    #[test]
    fn test_compile_glob_prefix() -> TestResult {
        let parsed = ParsedPattern::parse("foo*")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Prefix(val) if val == "foo"));
        Ok(())
    }

    #[test]
    fn test_compile_glob_suffix() -> TestResult {
        let parsed = ParsedPattern::parse("*bar")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Suffix(val) if val == "bar"));
        Ok(())
    }

    #[test]
    fn test_compile_glob_extension() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?;
        let compiled = compile_pattern(&parsed)?;
        // Extension becomes Suffix with the dot
        assert!(matches!(compiled, CompiledPattern::Suffix(val) if val == ".txt"));
        Ok(())
    }

    #[test]
    fn test_compile_glob_contains() -> TestResult {
        let parsed = ParsedPattern::parse("*needle*")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "needle"));
        Ok(())
    }

    #[test]
    fn test_compile_glob_prefix_suffix() -> TestResult {
        let parsed = ParsedPattern::parse("foo*bar")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(
            matches!(compiled, CompiledPattern::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == "bar")
        );
        Ok(())
    }

    #[test]
    fn test_compile_glob_complex() -> TestResult {
        let parsed = ParsedPattern::parse("file?.txt")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(
            compiled,
            CompiledPattern::Regex { anchored: true, .. }
        ));
        Ok(())
    }

    #[test]
    fn test_compile_regex() -> TestResult {
        let parsed = ParsedPattern::parse(r">.*\.log$")?;
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(
            compiled,
            CompiledPattern::Regex {
                anchored: false,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn test_compile_exact_no_wildcards() -> TestResult {
        let parsed = ParsedPattern::parse("README.md")?;
        // This is detected as Literal by ParsedPattern, so becomes Contains
        let compiled = compile_pattern(&parsed)?;
        assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "README.md"));
        Ok(())
    }

    // ========================================================================
    // Expression Lowering Tests (to_expr)
    // ========================================================================

    #[test]
    fn test_to_expr_any() {
        let pattern = CompiledPattern::Any;
        let expr = pattern.to_expr("name", true);
        // Should produce lit(true)
        let expr_str = format!("{expr:?}");
        assert!(expr_str.contains("true"), "Any should produce lit(true)");
    }

    #[test]
    fn test_to_expr_exact() {
        let pattern = CompiledPattern::Exact("README.md".to_owned());
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        // Should contain equality check (debug format uses ==)
        assert!(
            expr_str.contains("=="),
            "Exact should produce equality: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_prefix() {
        let pattern = CompiledPattern::Prefix("foo".to_owned());
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("starts_with"),
            "Prefix should use starts_with: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_suffix() {
        let pattern = CompiledPattern::Suffix(".txt".to_owned());
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("ends_with"),
            "Suffix should use ends_with: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_contains() {
        let pattern = CompiledPattern::Contains("needle".to_owned());
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("contains"),
            "Contains should use contains: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_prefix_suffix() {
        let pattern = CompiledPattern::PrefixSuffix {
            prefix: "foo".to_owned(),
            suffix: "bar".to_owned(),
        };
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("starts_with") && expr_str.contains("ends_with"),
            "PrefixSuffix should use both: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_exact_set() {
        let pattern = CompiledPattern::ExactSet(vec!["README.md".to_owned(), "LICENSE".to_owned()]);
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("is_in"),
            "ExactSet should use is_in: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_contains_any() {
        let pattern = CompiledPattern::ContainsAny(vec!["foo".to_owned(), "bar".to_owned()]);
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("contains_any"),
            "ContainsAny should use contains_any: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_suffix_set() {
        let pattern = CompiledPattern::SuffixSet(vec![".txt".to_owned(), ".md".to_owned()]);
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("ends_with"),
            "SuffixSet should use ends_with: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_regex() {
        let pattern = CompiledPattern::Regex {
            pattern: r".*\.log$".to_owned(),
            anchored: false,
        };
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        assert!(
            expr_str.contains("contains"),
            "Regex should use contains: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_case_insensitive() {
        let pattern = CompiledPattern::Suffix(".TXT".to_owned());
        let expr = pattern.to_expr("name", false);
        let expr_str = format!("{expr:?}");
        // Should use regex with (?i) flag for case-insensitive matching
        assert!(
            expr_str.contains("(?i)"),
            "Case-insensitive should use (?i) regex flag: {expr_str}"
        );
    }

    #[test]
    fn test_to_expr_suffix_set_empty() {
        let pattern = CompiledPattern::SuffixSet(vec![]);
        let expr = pattern.to_expr("name", true);
        let expr_str = format!("{expr:?}");
        // Empty set should produce lit(false)
        assert!(
            expr_str.contains("false"),
            "Empty SuffixSet should produce lit(false): {expr_str}"
        );
    }

    // ========================================================================
    // Integration Tests - Verify expressions work against real DataFrames
    // ========================================================================

    #[test]
    fn test_suffix_case_insensitive_integration() -> TestResult {
        use uffs_polars::{Column, DataFrame, IntoLazy};

        // Create test DataFrame with various filenames
        let names = vec![
            "file.TXT",
            "FILE.txt",
            "$recycle.txt",
            "$I07QSZ8.TXT",
            "test.TXT",
            "no_ext",
            "file.doc",
        ];
        let df = DataFrame::new(names.len(), vec![Column::new("name".into(), &names)])?;

        // Test case-insensitive suffix matching
        let pattern = CompiledPattern::Suffix(".txt".to_owned());
        let expr = pattern.to_expr("name", false); // case_sensitive = false

        let result = df.lazy().filter(expr).collect()?;

        // Should match: file.TXT, FILE.txt, $recycle.txt, $I07QSZ8.TXT, test.TXT
        assert_eq!(
            result.height(),
            5,
            "Should match 5 .txt files (case-insensitive): {result:?}"
        );

        Ok(())
    }

    #[test]
    fn test_suffix_case_sensitive_integration() -> TestResult {
        use uffs_polars::{Column, DataFrame, IntoLazy};

        let input_names = vec![
            "file.TXT",
            "FILE.txt",
            "$recycle.txt",
            "$I07QSZ8.TXT",
            "test.TXT",
        ];
        let df = DataFrame::new(
            input_names.len(),
            vec![Column::new("name".into(), &input_names)],
        )?;

        // Test case-sensitive suffix matching
        let pattern = CompiledPattern::Suffix(".txt".to_owned());
        let expr = pattern.to_expr("name", true); // case_sensitive = true

        let result = df.lazy().filter(expr).collect()?;

        // Should match only: FILE.txt, $recycle.txt
        assert_eq!(
            result.height(),
            2,
            "Should match 2 .txt files (case-sensitive): {result:?}"
        );

        Ok(())
    }

    #[test]
    fn test_dollar_prefix_files_matched() -> TestResult {
        use uffs_polars::{Column, DataFrame, IntoLazy};

        let input_names = vec![
            "$MFT",
            "$recycle.bin",
            "$I07QSZ8.txt",
            "normal.txt",
            "$BITMAP",
        ];
        let df = DataFrame::new(
            input_names.len(),
            vec![Column::new("name".into(), &input_names)],
        )?;

        // Test that files starting with $ are matched by *.txt pattern
        let pattern = CompiledPattern::Suffix(".txt".to_owned());
        let expr = pattern.to_expr("name", false);

        let result = df.lazy().filter(expr).collect()?;

        // Should match: $I07QSZ8.txt, normal.txt
        assert_eq!(
            result.height(),
            2,
            "Should match files with $ prefix: {result:?}"
        );

        // Verify $I07QSZ8.txt is in the results
        let matched_names: Vec<&str> = result
            .column("name")?
            .str()?
            .into_iter()
            .flatten()
            .collect();
        assert!(
            matched_names.contains(&"$I07QSZ8.txt") || matched_names.contains(&"$i07qsz8.txt"),
            "Should include $I07QSZ8.txt: {matched_names:?}"
        );

        Ok(())
    }

    #[test]
    fn test_null_values_not_filtered() -> TestResult {
        use uffs_polars::{Column, DataFrame, IntoLazy};

        // Create test DataFrame with null values
        let input_names: Vec<Option<&str>> = vec![
            Some("file.txt"),
            None, // null value
            Some("$recycle.txt"),
            Some("test.TXT"),
        ];
        let name_col = Column::new("name".into(), &input_names);
        let df = DataFrame::new(4, vec![name_col])?;

        // Test case-insensitive suffix matching
        let pattern = CompiledPattern::Suffix(".txt".to_owned());
        let expr = pattern.to_expr("name", false);

        let result = df.lazy().filter(expr).collect()?;

        // Should match: file.txt, $recycle.txt, test.TXT (null should be filtered out)
        assert_eq!(
            result.height(),
            3,
            "Should match 3 .txt files (null filtered): {result:?}"
        );

        Ok(())
    }
}
