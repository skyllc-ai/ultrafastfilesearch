// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Search pattern parsing and matching.
//!
//! Parses search patterns with support for:
//! - Drive prefix extraction: `c:/pro*` → drive=C, pattern=`/pro*`
//! - REGEX detection: patterns starting with `>` are treated as regex
//! - Glob patterns: standard glob syntax with `*`, `**`, `?`, `[...]`
//!
//! # Examples
//!
//! ```
//! use uffs_core::pattern::{ParsedPattern, PatternType};
//!
//! // Drive prefix extraction
//! let parsed = ParsedPattern::parse("c:/pro*").unwrap();
//! assert_eq!(parsed.drive(), Some('C'));
//! assert_eq!(parsed.pattern(), "/pro*");
//! assert_eq!(parsed.pattern_type(), PatternType::Glob);
//!
//! // REGEX pattern
//! let parsed = ParsedPattern::parse(r">C:\\Temp.*\.txt").unwrap();
//! assert_eq!(parsed.pattern_type(), PatternType::Regex);
//!
//! // All drives (no prefix)
//! let parsed = ParsedPattern::parse("/pro*.txt").unwrap();
//! assert_eq!(parsed.drive(), None);
//! ```

// Helper functions separated for testability and code clarity
#![expect(
    clippy::single_call_fn,
    reason = "helper functions extracted for testability and code clarity"
)]

mod parse;

use crate::error::Result;

/// Type of search pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternType {
    /// Glob pattern (default): `*.rs`, `**/*.txt`
    Glob,
    /// Regular expression: starts with `>`
    Regex,
    /// Literal string match (no wildcards)
    Literal,
}

/// A parsed search pattern with extracted metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPattern {
    /// The drive letter (uppercase), if specified in pattern.
    drive: Option<char>,
    /// The pattern string (without drive prefix).
    pattern: String,
    /// The type of pattern.
    pattern_type: PatternType,
    /// Whether the pattern is case-sensitive.
    case_sensitive: bool,
    /// Whether this is a path pattern (contains `\` or `/`).
    /// When true, matching should be against the full path, not filename.
    is_path_pattern: bool,
}

impl ParsedPattern {
    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get the drive letter (uppercase) if specified in the pattern.
    #[must_use]
    pub const fn drive(&self) -> Option<char> {
        self.drive
    }

    /// Get the pattern string (without drive prefix).
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Get the pattern type.
    #[must_use]
    pub const fn pattern_type(&self) -> PatternType {
        self.pattern_type
    }

    /// Check if the pattern is case-sensitive.
    #[must_use]
    pub const fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Check if this is a path pattern (matches against full path, not
    /// filename).
    ///
    /// A pattern is path-aware when it contains directory separators (`\` or
    /// `/`), indicating the user wants to match against the file's location
    /// in the tree.
    #[must_use]
    pub const fn is_path_pattern(&self) -> bool {
        self.is_path_pattern
    }

    /// Set case sensitivity.
    #[must_use]
    pub const fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    /// Override the drive letter.
    #[must_use]
    pub const fn with_drive(mut self, drive: Option<char>) -> Self {
        self.drive = drive;
        self
    }

    /// Check if pattern has any drive specification.
    #[must_use]
    pub const fn has_drive(&self) -> bool {
        self.drive.is_some()
    }

    /// Check if this is a regex pattern.
    #[must_use]
    pub const fn is_regex(&self) -> bool {
        matches!(self.pattern_type, PatternType::Regex)
    }

    /// Check if this is a glob pattern.
    #[must_use]
    pub const fn is_glob(&self) -> bool {
        matches!(self.pattern_type, PatternType::Glob)
    }

    /// Check if this is a literal pattern (no wildcards).
    #[must_use]
    pub const fn is_literal(&self) -> bool {
        matches!(self.pattern_type, PatternType::Literal)
    }

    /// Convert the pattern to a regex string for matching.
    ///
    /// - Glob patterns are converted using `glob_to_regex`
    /// - Regex patterns are returned as-is
    /// - Literal patterns are escaped and wrapped for substring match
    ///
    /// # Errors
    ///
    /// Returns an error if glob-to-regex conversion fails.
    pub fn to_regex(&self) -> Result<String> {
        match self.pattern_type {
            PatternType::Regex => Ok(self.pattern.clone()),
            PatternType::Glob => crate::glob::glob_to_regex(&self.pattern),
            PatternType::Literal => {
                let escaped = regex_escape(&self.pattern);
                Ok(format!(".*{escaped}.*"))
            }
        }
    }
}

/// Escape regex metacharacters in a string.
fn regex_escape(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    for ch in input.chars() {
        match ch {
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    #[test]
    fn simple_glob_pattern() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "*.txt");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn drive_prefix_lowercase() -> TestResult {
        let parsed = ParsedPattern::parse("c:/pro*")?;
        assert_eq!(parsed.drive(), Some('C'));
        assert_eq!(parsed.pattern(), "/pro*");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn drive_prefix_uppercase() -> TestResult {
        let parsed = ParsedPattern::parse("D:\\Users\\*")?;
        assert_eq!(parsed.drive(), Some('D'));
        assert_eq!(parsed.pattern(), "\\Users\\*");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn no_drive_with_leading_slash() -> TestResult {
        let parsed = ParsedPattern::parse("/pro*.txt")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "/pro*.txt");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn regex_pattern() -> TestResult {
        let parsed = ParsedPattern::parse(r">C:\\Temp.*\.txt")?;
        assert_eq!(parsed.drive(), Some('C'));
        assert_eq!(parsed.pattern_type(), PatternType::Regex);
        assert!(parsed.pattern().contains("Temp"));
        Ok(())
    }

    #[test]
    fn regex_pattern_with_quotes() -> TestResult {
        let parsed = ParsedPattern::parse(r#">"C:\\Temp.*""#)?;
        assert_eq!(parsed.pattern_type(), PatternType::Regex);
        assert_eq!(parsed.drive(), Some('C'));
        Ok(())
    }

    #[test]
    fn literal_pattern() -> TestResult {
        let parsed = ParsedPattern::parse("readme")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "readme");
        assert_eq!(parsed.pattern_type(), PatternType::Literal);
        Ok(())
    }

    #[test]
    fn empty_pattern_error() {
        let result = ParsedPattern::parse("");
        result.unwrap_err();
    }

    #[test]
    fn whitespace_only_error() {
        let result = ParsedPattern::parse("   ");
        result.unwrap_err();
    }

    #[test]
    fn to_regex_glob() -> TestResult {
        let parsed = ParsedPattern::parse("*.rs")?;
        let regex = parsed.to_regex()?;
        assert!(regex.starts_with('^'));
        assert!(regex.ends_with('$'));
        Ok(())
    }

    #[test]
    fn to_regex_literal() -> TestResult {
        let parsed = ParsedPattern::parse("main")?;
        let regex = parsed.to_regex()?;
        assert!(regex.contains("main"));
        assert!(regex.starts_with(".*"));
        Ok(())
    }

    #[test]
    fn case_sensitivity() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?.with_case_sensitive(true);
        assert!(parsed.is_case_sensitive());

        let parsed2 = parsed.with_case_sensitive(false);
        assert!(!parsed2.is_case_sensitive());
        Ok(())
    }

    #[test]
    fn drive_override() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?.with_drive(Some('E'));
        assert_eq!(parsed.drive(), Some('E'));
        Ok(())
    }

    #[test]
    fn double_star_glob() -> TestResult {
        let parsed = ParsedPattern::parse("**\\Users\\**")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        assert!(parsed.pattern().contains("**"));
        Ok(())
    }

    #[test]
    fn question_mark_glob() -> TestResult {
        let parsed = ParsedPattern::parse("file?.txt")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn bracket_glob() -> TestResult {
        let parsed = ParsedPattern::parse("[abc].txt")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    // =========================================================================
    // Path-aware detection (P6-P8 from branch matrix)
    // =========================================================================

    #[test]
    fn backslash_path_detection() -> TestResult {
        let parsed = ParsedPattern::parse("\\Users\\*")?;
        assert!(
            parsed.is_path_pattern(),
            "backslash should trigger path detection"
        );
        Ok(())
    }

    #[test]
    fn forward_slash_path_detection() -> TestResult {
        let parsed = ParsedPattern::parse("Users/foo/*")?;
        assert!(
            parsed.is_path_pattern(),
            "forward slash should trigger path detection"
        );
        Ok(())
    }

    #[test]
    fn literal_always_path_aware() -> TestResult {
        let parsed = ParsedPattern::parse("nice")?;
        assert_eq!(parsed.pattern_type(), PatternType::Literal);
        assert!(
            parsed.is_path_pattern(),
            "literals should always be path-aware"
        );
        Ok(())
    }

    #[test]
    fn simple_glob_not_path_aware() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?;
        assert!(
            !parsed.is_path_pattern(),
            "simple glob without separator should not be path-aware"
        );
        Ok(())
    }

    #[test]
    fn glob_with_backslash_is_path_aware() -> TestResult {
        let parsed = ParsedPattern::parse("**\\Users\\**\\AppData\\**")?;
        assert!(
            parsed.is_path_pattern(),
            "glob with backslash should be path-aware"
        );
        Ok(())
    }

    #[test]
    fn leading_slash_not_path_aware() -> TestResult {
        // Leading slash is a glob prefix, not a path separator between components
        let parsed = ParsedPattern::parse("/pro*")?;
        assert!(
            !parsed.is_path_pattern(),
            "leading slash alone should not be path-aware"
        );
        Ok(())
    }

    #[test]
    fn drive_prefix_with_path() -> TestResult {
        let parsed = ParsedPattern::parse("C:\\Windows\\*")?;
        assert_eq!(parsed.drive(), Some('C'));
        assert!(
            parsed.is_path_pattern(),
            "drive + backslash path should be path-aware"
        );
        Ok(())
    }

    #[test]
    fn regex_with_path_separators() -> TestResult {
        let parsed = ParsedPattern::parse(r">C:\\TemP.*\.txt")?;
        assert_eq!(parsed.pattern_type(), PatternType::Regex);
        assert!(
            parsed.is_path_pattern(),
            "regex with backslashes should be path-aware"
        );
        Ok(())
    }

    // =========================================================================
    // Forward slash normalization
    // =========================================================================

    #[test]
    fn forward_slash_normalized_in_path_pattern() -> TestResult {
        let parsed = ParsedPattern::parse("Users/foo/bar/*")?;
        assert!(
            parsed.pattern().contains('\\'),
            "forward slashes should be normalized to backslashes"
        );
        assert!(
            !parsed.pattern().contains('/'),
            "no forward slashes should remain after normalization"
        );
        Ok(())
    }

    #[test]
    fn forward_slash_not_normalized_in_non_path_glob() -> TestResult {
        // /pro* has leading slash only — not path-aware, no normalization
        let parsed = ParsedPattern::parse("/pro*")?;
        assert_eq!(
            parsed.pattern(),
            "/pro*",
            "leading slash should not be normalized"
        );
        Ok(())
    }
}
