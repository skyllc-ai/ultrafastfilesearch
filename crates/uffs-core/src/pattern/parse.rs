//! Pattern parsing helpers for `ParsedPattern`.

use super::{ParsedPattern, PatternType};
use crate::error::{CoreError, Result};

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
    #[expect(
        clippy::shadow_reuse,
        reason = "rebinding input to trimmed version is idiomatic"
    )]
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();

        if input.is_empty() {
            return Err(CoreError::InvalidPattern {
                pattern: input.to_owned(),
                reason: "Pattern cannot be empty".to_owned(),
            });
        }

        if let Some(regex_pattern) = input.strip_prefix('>') {
            return Self::parse_regex(regex_pattern);
        }

        let (drive, remaining) = Self::extract_drive_prefix(input);
        let has_separator = Self::contains_path_separator(remaining);
        let pattern_type_preview = Self::detect_pattern_type(remaining);
        // Literal (bare text like "nice") is always path-aware so it matches
        // against full paths (like Everything, WizFile, C++ UFFS).
        // Glob patterns are path-aware only if they contain separators.
        let is_path = has_separator || matches!(pattern_type_preview, PatternType::Literal);
        // Normalize forward slashes to backslashes for Windows path matching.
        let normalized = if has_separator {
            remaining.replace('/', "\\")
        } else {
            remaining.to_owned()
        };
        let pattern_type = Self::detect_pattern_type(&normalized);

        Ok(Self {
            drive,
            pattern: normalized,
            pattern_type,
            case_sensitive: false,
            is_path_pattern: is_path,
        })
    }

    /// Parse a REGEX pattern (after stripping the `>` prefix).
    #[expect(
        clippy::shadow_reuse,
        reason = "rebinding pattern to stripped version is idiomatic"
    )]
    fn parse_regex(pattern: &str) -> Result<Self> {
        let pattern = pattern.trim().trim_matches('"').trim_matches('\'');

        if pattern.is_empty() {
            return Err(CoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: "REGEX pattern cannot be empty".to_owned(),
            });
        }

        let (drive, remaining) = Self::extract_drive_from_regex(pattern);

        let is_path = Self::contains_path_separator(remaining);
        Ok(Self {
            drive,
            pattern: remaining.to_owned(),
            pattern_type: PatternType::Regex,
            case_sensitive: false,
            is_path_pattern: is_path,
        })
    }

    /// Extract drive letter from a path-like pattern.
    ///
    /// Handles: `c:/path`, `C:\path`, `d:path`
    fn extract_drive_prefix(input: &str) -> (Option<char>, &str) {
        let mut chars = input.chars();

        let first = chars.next();
        let second = chars.next();

        if let (Some(drive_char), Some(':')) = (first, second) {
            if drive_char.is_ascii_alphabetic() {
                let drive = drive_char.to_ascii_uppercase();
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

        let first = chars.next();
        let second = chars.next();
        let third = chars.next();

        if let (Some(drive_char), Some(':'), Some(sep)) = (first, second, third) {
            if drive_char.is_ascii_alphabetic() && (sep == '\\' || sep == '/') {
                let drive = drive_char.to_ascii_uppercase();
                return (Some(drive), pattern);
            }
        }

        (None, pattern)
    }

    /// Detect whether a pattern contains path separators (`\` or `/`).
    ///
    /// When true, the pattern should be matched against the full file path
    /// (e.g., `D:\Users\foo\file.txt`) instead of just the filename.
    fn contains_path_separator(pattern: &str) -> bool {
        // A pattern is path-aware if it contains directory separators BETWEEN
        // path components (not just a leading slash like "/pro*").
        // Strip leading separator — it's a glob prefix, not a path indicator.
        let inner = pattern
            .strip_prefix('/')
            .or_else(|| pattern.strip_prefix('\\'))
            .unwrap_or(pattern);
        inner.contains('\\') || inner.contains('/')
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
}
