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
pub fn glob_to_regex(pattern: &str) -> Result<String> {
    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    // ** matches everything including path separators
                    regex.push_str(".*");
                    i += 1;
                } else {
                    // * matches everything except path separators
                    regex.push_str("[^/\\\\]*");
                }
            }
            '?' => {
                regex.push_str("[^/\\\\]");
            }
            '[' => {
                // Character class
                regex.push('[');
                i += 1;

                if i < chars.len() && chars[i] == '!' {
                    regex.push('^');
                    i += 1;
                }

                while i < chars.len() && chars[i] != ']' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        regex.push('\\');
                        i += 1;
                        regex.push(chars[i]);
                    } else {
                        regex.push(chars[i]);
                    }
                    i += 1;
                }

                if i >= chars.len() {
                    return Err(CoreError::InvalidGlob {
                        pattern: pattern.to_string(),
                        reason: "Unclosed character class".to_string(),
                    });
                }

                regex.push(']');
            }
            '.' | '+' | '^' | '$' | '(' | ')' | '{' | '}' | '|' | '\\' => {
                // Escape regex metacharacters
                regex.push('\\');
                regex.push(chars[i]);
            }
            c => {
                regex.push(c);
            }
        }
        i += 1;
    }

    regex.push('$');
    Ok(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_extension() {
        let regex = glob_to_regex("*.rs").unwrap();
        assert_eq!(regex, "^[^/\\\\]*\\.rs$");
    }

    #[test]
    fn test_double_star() {
        let regex = glob_to_regex("**/*.rs").unwrap();
        assert_eq!(regex, "^.*/[^/\\\\]*\\.rs$");
    }

    #[test]
    fn test_question_mark() {
        let regex = glob_to_regex("file?.txt").unwrap();
        assert_eq!(regex, "^file[^/\\\\]\\.txt$");
    }

    #[test]
    fn test_character_class() {
        let regex = glob_to_regex("[abc].txt").unwrap();
        assert_eq!(regex, "^[abc]\\.txt$");
    }

    #[test]
    fn test_negated_class() {
        let regex = glob_to_regex("[!abc].txt").unwrap();
        assert_eq!(regex, "^[^abc]\\.txt$");
    }

    #[test]
    fn test_unclosed_bracket() {
        let result = glob_to_regex("[abc");
        assert!(result.is_err());
    }
}

