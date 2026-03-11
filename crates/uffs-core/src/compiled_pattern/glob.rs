//! Glob classification and compilation helpers for `CompiledPattern`.

use super::CompiledPattern;
use crate::error::Result;
use crate::pattern::{ParsedPattern, PatternType};

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
    let has_star = pattern.contains('*');
    let has_question = pattern.contains('?');
    let has_bracket = pattern.contains('[');
    let has_double_star = pattern.contains("**");

    if !has_star && !has_question && !has_bracket {
        return GlobKind::Exact(pattern.to_owned());
    }

    if has_question || has_bracket || has_double_star {
        return GlobKind::Complex(pattern.to_owned());
    }

    classify_single_star_pattern(pattern)
}

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

/// Classify patterns with only single `*` characters (no `?`, `[]`, or `**`).
fn classify_single_star_pattern(pattern: &str) -> GlobKind {
    let star_count = pattern.matches('*').count();

    match star_count {
        1 => classify_one_star(pattern),
        2 if pattern.starts_with('*') && pattern.ends_with('*') => {
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
            pattern.strip_suffix('*').map_or_else(
                || {
                    let (prefix, suffix) = pattern.split_once('*').unwrap_or((pattern, ""));
                    GlobKind::PrefixSuffix {
                        prefix: prefix.to_owned(),
                        suffix: suffix.to_owned(),
                    }
                },
                |prefix| GlobKind::Prefix(prefix.to_owned()),
            )
        },
        |suffix| {
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
        GlobKind::Extension(ext) => Ok(CompiledPattern::Suffix(format!(".{ext}"))),
        GlobKind::Complex(complex_pattern) => {
            let regex = crate::glob::glob_to_regex(&complex_pattern)?;
            Ok(CompiledPattern::Regex {
                pattern: regex,
                anchored: true,
            })
        }
    }
}
