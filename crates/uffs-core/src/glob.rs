//! Glob pattern to regex conversion.
//!
//! Converts glob patterns like "*.rs" to regex patterns for Polars filtering.

use crate::error::{CoreError, Result};

/// Convert a glob pattern to a regex pattern.
///
/// Supports:
/// - `*` - matches any characters except path separator
/// - `**` - matches any characters including path separator
/// - `?` - matches any single character
/// - `[abc]` - matches any character in brackets
/// - `[!abc]` - matches any character not in brackets
///
/// # Errors
///
/// Returns an error if the glob pattern is invalid.
#[allow(clippy::single_call_fn)] // Intentionally separate for clarity and testability
pub fn glob_to_regex(pattern: &str) -> Result<String> {
    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let chars: Vec<char> = pattern.chars().collect();
    let mut idx = 0;

    while idx < chars.len() {
        let current_char = chars.get(idx).copied();
        match current_char {
            Some('*') => {
                if idx + 1 < chars.len() && chars.get(idx + 1) == Some(&'*') {
                    // ** matches everything including path separators
                    regex.push_str(".*");
                    idx += 1;
                } else {
                    // * matches everything except path separators
                    regex.push_str("[^/\\\\]*");
                }
            }
            Some('?') => {
                regex.push_str("[^/\\\\]");
            }
            Some('[') => {
                // Character class
                regex.push('[');
                idx += 1;

                if idx < chars.len() && chars.get(idx) == Some(&'!') {
                    regex.push('^');
                    idx += 1;
                }

                while idx < chars.len() && chars.get(idx) != Some(&']') {
                    if chars.get(idx) == Some(&'\\') && idx + 1 < chars.len() {
                        regex.push('\\');
                        idx += 1;
                        if let Some(&ch) = chars.get(idx) {
                            regex.push(ch);
                        }
                    } else if let Some(&ch) = chars.get(idx) {
                        regex.push(ch);
                    }
                    idx += 1;
                }

                if idx >= chars.len() {
                    return Err(CoreError::InvalidGlob {
                        pattern: pattern.to_owned(),
                        reason: "Unclosed character class".to_owned(),
                    });
                }

                regex.push(']');
            }
            Some(ch @ ('.' | '+' | '^' | '$' | '(' | ')' | '{' | '}' | '|' | '\\')) => {
                // Escape regex metacharacters
                regex.push('\\');
                regex.push(ch);
            }
            Some(ch) => {
                regex.push(ch);
            }
            None => break,
        }
        idx += 1;
    }

    regex.push('$');
    Ok(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    #[test]
    fn test_simple_extension() -> TestResult {
        let regex = glob_to_regex("*.rs")?;
        assert_eq!(regex, "^[^/\\\\]*\\.rs$");
        Ok(())
    }

    #[test]
    fn test_double_star() -> TestResult {
        let regex = glob_to_regex("**/*.rs")?;
        assert_eq!(regex, "^.*/[^/\\\\]*\\.rs$");
        Ok(())
    }

    #[test]
    fn test_question_mark() -> TestResult {
        let regex = glob_to_regex("file?.txt")?;
        assert_eq!(regex, "^file[^/\\\\]\\.txt$");
        Ok(())
    }

    #[test]
    fn test_character_class() -> TestResult {
        let regex = glob_to_regex("[abc].txt")?;
        assert_eq!(regex, "^[abc]\\.txt$");
        Ok(())
    }

    #[test]
    fn test_negated_class() -> TestResult {
        let regex = glob_to_regex("[!abc].txt")?;
        assert_eq!(regex, "^[^abc]\\.txt$");
        Ok(())
    }

    #[test]
    fn test_unclosed_bracket() {
        let result = glob_to_regex("[abc");
        assert!(result.is_err());
    }
}
