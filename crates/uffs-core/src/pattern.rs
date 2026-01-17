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

// Allow single-call functions for clarity and testability
#![allow(clippy::single_call_fn)]
// Allow shadowing for pattern parsing (input -> trimmed input)
#![allow(clippy::shadow_reuse)]

use crate::error::{CoreError, Result};

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
}

impl ParsedPattern {
    /// Parse a search pattern string.
    ///
    /// # Pattern Syntax
    ///
    /// - `c:/pro*` - Drive prefix with glob pattern
    /// - `/pro*.txt` - All drives with glob pattern
    /// - `>C:\\Temp.*` - REGEX pattern (starts with `>`)
    /// - `*.txt` - Simple glob pattern
    /// - `needle` - Literal search (no wildcards)
    ///
    /// # Errors
    ///
    /// Returns an error if the pattern is empty or invalid.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();

        if input.is_empty() {
            return Err(CoreError::InvalidPattern {
                pattern: input.to_owned(),
                reason: "Pattern cannot be empty".to_owned(),
            });
        }

        // Check for REGEX pattern (starts with >)
        if let Some(regex_pattern) = input.strip_prefix('>') {
            return Self::parse_regex(regex_pattern);
        }

        // Check for drive prefix (e.g., "c:/", "C:\", "d:")
        let (drive, remaining) = Self::extract_drive_prefix(input);

        // Determine pattern type
        let pattern_type = Self::detect_pattern_type(remaining);

        Ok(Self {
            drive,
            pattern: remaining.to_owned(),
            pattern_type,
            case_sensitive: false,
        })
    }

    /// Parse a REGEX pattern (after stripping the `>` prefix).
    fn parse_regex(pattern: &str) -> Result<Self> {
        // Strip surrounding quotes if present
        let pattern = pattern.trim().trim_matches('"').trim_matches('\'');

        if pattern.is_empty() {
            return Err(CoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: "REGEX pattern cannot be empty".to_owned(),
            });
        }

        // Extract drive from regex pattern (e.g., "C:\\..." or "C:/...")
        let (drive, remaining) = Self::extract_drive_from_regex(pattern);

        Ok(Self {
            drive,
            pattern: remaining.to_owned(),
            pattern_type: PatternType::Regex,
            case_sensitive: false,
        })
    }

    /// Extract drive letter from a path-like pattern.
    ///
    /// Handles: `c:/path`, `C:\path`, `d:path`
    fn extract_drive_prefix(input: &str) -> (Option<char>, &str) {
        let mut chars = input.chars();

        // Get first two characters safely
        let first = chars.next();
        let second = chars.next();

        // Check for drive letter pattern: X: or X:/ or X:\
        if let (Some(drive_char), Some(':')) = (first, second) {
            if drive_char.is_ascii_alphabetic() {
                let drive = drive_char.to_ascii_uppercase();
                // Skip the "X:" part (2 bytes for ASCII), keep the rest
                let remaining = input.get(2..).unwrap_or("");
                return (Some(drive), remaining);
            }
        }

        (None, input)
    }

    /// Extract drive letter from a regex pattern.
    ///
    /// Handles: `C:\\...`, `C:/...`, `[Cc]:\\...`
    fn extract_drive_from_regex(pattern: &str) -> (Option<char>, &str) {
        let mut chars = pattern.chars();

        // Get first three characters safely
        let first = chars.next();
        let second = chars.next();
        let third = chars.next();

        // Check for literal drive: C:\\ or C:/
        if let (Some(drive_char), Some(':'), Some(sep)) = (first, second, third) {
            if drive_char.is_ascii_alphabetic() && (sep == '\\' || sep == '/') {
                let drive = drive_char.to_ascii_uppercase();
                return (Some(drive), pattern);
            }
        }

        (None, pattern)
    }

    /// Detect whether a pattern is glob, regex, or literal.
    fn detect_pattern_type(pattern: &str) -> PatternType {
        for ch in pattern.chars() {
            match ch {
                '*' | '?' | '[' => return PatternType::Glob,
                _ => {}
            }
        }
        PatternType::Literal
    }

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
