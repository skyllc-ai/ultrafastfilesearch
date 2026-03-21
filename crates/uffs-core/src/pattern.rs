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
    fn test_simple_glob_pattern() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "*.txt");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn test_drive_prefix_lowercase() -> TestResult {
        let parsed = ParsedPattern::parse("c:/pro*")?;
        assert_eq!(parsed.drive(), Some('C'));
        assert_eq!(parsed.pattern(), "/pro*");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn test_drive_prefix_uppercase() -> TestResult {
        let parsed = ParsedPattern::parse("D:\\Users\\*")?;
        assert_eq!(parsed.drive(), Some('D'));
        assert_eq!(parsed.pattern(), "\\Users\\*");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn test_no_drive_with_leading_slash() -> TestResult {
        let parsed = ParsedPattern::parse("/pro*.txt")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "/pro*.txt");
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn test_regex_pattern() -> TestResult {
        let parsed = ParsedPattern::parse(r">C:\\Temp.*\.txt")?;
        assert_eq!(parsed.drive(), Some('C'));
        assert_eq!(parsed.pattern_type(), PatternType::Regex);
        assert!(parsed.pattern().contains("Temp"));
        Ok(())
    }

    #[test]
    fn test_regex_pattern_with_quotes() -> TestResult {
        let parsed = ParsedPattern::parse(r#">"C:\\Temp.*""#)?;
        assert_eq!(parsed.pattern_type(), PatternType::Regex);
        assert_eq!(parsed.drive(), Some('C'));
        Ok(())
    }

    #[test]
    fn test_literal_pattern() -> TestResult {
        let parsed = ParsedPattern::parse("readme")?;
        assert_eq!(parsed.drive(), None);
        assert_eq!(parsed.pattern(), "readme");
        assert_eq!(parsed.pattern_type(), PatternType::Literal);
        Ok(())
    }

    #[test]
    fn test_empty_pattern_error() {
        let result = ParsedPattern::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn test_whitespace_only_error() {
        let result = ParsedPattern::parse("   ");
        assert!(result.is_err());
    }

    #[test]
    fn test_to_regex_glob() -> TestResult {
        let parsed = ParsedPattern::parse("*.rs")?;
        let regex = parsed.to_regex()?;
        assert!(regex.starts_with('^'));
        assert!(regex.ends_with('$'));
        Ok(())
    }

    #[test]
    fn test_to_regex_literal() -> TestResult {
        let parsed = ParsedPattern::parse("main")?;
        let regex = parsed.to_regex()?;
        assert!(regex.contains("main"));
        assert!(regex.starts_with(".*"));
        Ok(())
    }

    #[test]
    fn test_case_sensitivity() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?.with_case_sensitive(true);
        assert!(parsed.is_case_sensitive());

        let parsed2 = parsed.with_case_sensitive(false);
        assert!(!parsed2.is_case_sensitive());
        Ok(())
    }

    #[test]
    fn test_drive_override() -> TestResult {
        let parsed = ParsedPattern::parse("*.txt")?.with_drive(Some('E'));
        assert_eq!(parsed.drive(), Some('E'));
        Ok(())
    }

    #[test]
    fn test_double_star_glob() -> TestResult {
        let parsed = ParsedPattern::parse("**\\Users\\**")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        assert!(parsed.pattern().contains("**"));
        Ok(())
    }

    #[test]
    fn test_question_mark_glob() -> TestResult {
        let parsed = ParsedPattern::parse("file?.txt")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }

    #[test]
    fn test_bracket_glob() -> TestResult {
        let parsed = ParsedPattern::parse("[abc].txt")?;
        assert_eq!(parsed.pattern_type(), PatternType::Glob);
        Ok(())
    }
}
