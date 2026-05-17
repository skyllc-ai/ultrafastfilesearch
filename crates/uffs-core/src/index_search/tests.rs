// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for the compiled-pattern surface used by `crate::aggregate`.
//!
//! After #263 the only externally-reachable items in this submodule are
//! [`compile_parsed_pattern`] and [`IndexPattern::matches`].  The tests
//! drive every reachable `IndexPattern` variant through the alive entry
//! point (`compile_parsed_pattern`) and assert:
//!
//! - Every variant produced by the `Glob → IndexPattern` routing (`Any`,
//!   `Prefix`, `Suffix`, `Contains`, `PrefixSuffix`, `Regex`).
//! - `PatternType::Literal` → `IndexPattern::Contains` substring match.
//! - OR patterns (`*.txt|*.log` / `foo|bar`) split into `IndexPattern::Or` over
//!   per-alternative compilation, exercising the otherwise-internal `Exact`
//!   variant (no-wildcard alternative).
//! - Case-sensitive vs case-insensitive matching via NTFS `$UpCase` folding
//!   ([`CaseFold`]).
//! - Regex auto-anchoring at end-of-string (`>...` patterns) and compile-time
//!   error surface for malformed regex.

use uffs_text::case_fold::CaseFold;

use super::compile_parsed_pattern;
use super::pattern::IndexPattern;
use crate::pattern::ParsedPattern;

/// Convenience: default fold for tests.
fn fold() -> CaseFold {
    CaseFold::default_table()
}

/// Convenience: parse and compile, panicking on error.  Tests that
/// deliberately exercise the error path (e.g. invalid regex) call
/// `compile_parsed_pattern` directly.
fn compile(pattern: &str) -> IndexPattern {
    let parsed = ParsedPattern::parse(pattern).expect("parse must succeed");
    compile_parsed_pattern(&parsed).expect("compile must succeed")
}

// ===========================================================================
// Glob → IndexPattern variant routing (exercises classify_glob mapping).
// ===========================================================================

#[test]
fn any_glob_matches_everything() {
    let pat = compile("*");
    assert!(pat.matches("anything.txt", true, fold()));
    assert!(pat.matches("", true, fold()));
    assert!(pat.matches(r"C:\Windows\System32", false, fold()));
}

#[test]
fn prefix_glob_matches_only_at_start() {
    let pat = compile("foo*");
    assert!(pat.matches("foo", true, fold()));
    assert!(pat.matches("foobar", true, fold()));
    assert!(!pat.matches("barfoo", true, fold()));
    assert!(pat.matches("FOOBAR", false, fold()));
}

#[test]
fn suffix_glob_matches_only_at_end() {
    let pat = compile("*.txt");
    assert!(pat.matches("foo.txt", true, fold()));
    assert!(pat.matches(".txt", true, fold()));
    assert!(!pat.matches("foo.txt.bak", true, fold()));
    assert!(pat.matches("FOO.TXT", false, fold()));
}

#[test]
fn contains_glob_matches_anywhere() {
    let pat = compile("*needle*");
    assert!(pat.matches("needle", true, fold()));
    assert!(pat.matches("haystackneedlehaystack", true, fold()));
    assert!(!pat.matches("haystack", true, fold()));
    assert!(pat.matches("NEEDLE", false, fold()));
}

#[test]
fn prefix_suffix_glob_requires_both_anchors() {
    let pat = compile("foo*bar");
    assert!(pat.matches("foobar", true, fold()));
    assert!(pat.matches("foo123bar", true, fold()));
    assert!(!pat.matches("foobarbaz", true, fold()));
    assert!(!pat.matches("bazfoobar", true, fold()));
}

#[test]
fn extension_glob_normalizes_to_dot_suffix() {
    // `*.rs` → GlobKind::Extension("rs") → IndexPattern::Suffix(".rs").
    let pat = compile("*.rs");
    assert!(pat.matches("main.rs", true, fold()));
    assert!(pat.matches("MAIN.RS", false, fold()));
    assert!(!pat.matches("main.py", false, fold()));
    assert!(!pat.matches("rs", false, fold()), "bare ext must not match");
}

// ===========================================================================
// PatternType::Literal → IndexPattern::Contains (substring) routing.
// ===========================================================================

#[test]
fn literal_is_substring_match() {
    // No wildcards, no leading `>` → ParsedPattern::Literal →
    // IndexPattern::Contains.
    let pat = compile("nice");
    assert!(pat.matches("nicehouse", false, fold()));
    assert!(pat.matches("venice.jpg", false, fold()));
    assert!(pat.matches("NICE_FILE", false, fold()));
}

#[test]
fn literal_rejects_unrelated_strings() {
    let pat = compile("nice");
    assert!(!pat.matches("bad.txt", false, fold()));
    assert!(!pat.matches("", false, fold()));
}

// ===========================================================================
// OR patterns: per-alternative compilation, including the otherwise
// internal `Exact` variant when an OR alternative has no wildcards.
// ===========================================================================

#[test]
fn or_first_alternative_matches() {
    let pat = compile("*.txt|*.log");
    assert!(pat.matches("notes.txt", false, fold()));
}

#[test]
fn or_second_alternative_matches() {
    let pat = compile("*.txt|*.log");
    assert!(pat.matches("server.log", false, fold()));
}

#[test]
fn or_no_match_falls_through_all_alternatives() {
    let pat = compile("*.txt|*.log");
    assert!(!pat.matches("main.rs", false, fold()));
}

#[test]
fn or_multi_alternatives_match_middle() {
    let pat = compile("nice|cool|awesome");
    assert!(pat.matches("cool", true, fold()));
    assert!(pat.matches("AWESOME", false, fold()));
    assert!(!pat.matches("bad", false, fold()));
}

#[test]
fn or_with_wildcard_free_alternatives_uses_exact_variant() {
    // Each alternative has no wildcards, so `compile_index_pattern_with_fold`
    // routes through `GlobKind::Exact` → `IndexPattern::Exact`.  We assert
    // strict-equality semantics (not substring) to confirm the routing.
    let pat = compile("foo|bar");
    assert!(pat.matches("foo", true, fold()));
    assert!(pat.matches("bar", true, fold()));
    assert!(pat.matches("FOO", false, fold()));
    assert!(!pat.matches("foobar", true, fold()), "no substring match");
    assert!(!pat.matches("foobaz", true, fold()));
}

// ===========================================================================
// Case sensitivity: NTFS $UpCase folding across glob/literal/OR.
// ===========================================================================

#[test]
fn case_sensitive_glob_suffix_rejects_wrong_case() {
    let pat = compile("*.TXT");
    assert!(!pat.matches("file.txt", true, fold()));
    assert!(pat.matches("file.TXT", true, fold()));
}

#[test]
fn case_insensitive_glob_suffix_accepts_either_case() {
    let pat = compile("*.TXT");
    assert!(pat.matches("file.txt", false, fold()));
    assert!(pat.matches("FILE.TXT", false, fold()));
}

#[test]
fn case_sensitive_literal_distinguishes_case() {
    let pat = compile("nice");
    // Contains is case-sensitive when case_sensitive=true.
    assert!(pat.matches("nice", true, fold()));
    assert!(!pat.matches("Nice", true, fold()));
    assert!(pat.matches("Nice", false, fold()));
}

// ===========================================================================
// Regex (PatternType::Regex via `>` prefix) — auto-anchoring at end-of-string.
//
// Rust's regex::is_match() is substring by default.  compile_parsed_pattern
// appends `$` so >.*\.png matches "photo.png" but NOT "icon.png.vir" or
// ADS entries like "photo.png:com.dropbox.attrs".
// ===========================================================================

#[test]
fn regex_anchors_at_end_of_string() {
    let pat = compile(r">.*\.(jpg|png|heic)");
    assert!(pat.matches("photo.jpg", false, fold()));
    assert!(pat.matches("image.png", false, fold()));
    assert!(pat.matches("camera.heic", false, fold()));
    assert!(pat.matches(r"C:\Users\Pictures\vacation.jpg", false, fold()));
    assert!(
        pat.matches(r"D:\Dropbox\photo.PNG", false, fold()),
        "case-insensitive fold should accept upper-case extension"
    );
}

#[test]
fn regex_rejects_extension_appearing_mid_filename() {
    let pat = compile(r">.*\.(jpg|png|heic)");
    assert!(!pat.matches("icon.png.vir", false, fold()));
    assert!(!pat.matches("backup.jpg.bak", false, fold()));
    assert!(!pat.matches("archive.heic.zip", false, fold()));
}

#[test]
fn regex_rejects_ads_entries() {
    let pat = compile(r">.*\.(jpg|png|heic)");
    assert!(!pat.matches("photo.png:com.dropbox.attrs", false, fold()));
    assert!(!pat.matches("image.jpg:Zone.Identifier", false, fold()));
    assert!(!pat.matches("file.heic:$DATA", false, fold()));
}

#[test]
fn regex_with_explicit_dollar_anchor_is_not_doubled() {
    let pat = compile(r">.*\.txt$");
    assert!(pat.matches("readme.txt", false, fold()));
    assert!(!pat.matches("readme.txt.bak", false, fold()));
}

#[test]
fn regex_with_path_prefix_and_extension() {
    let pat = compile(r">C:\\Users\\.*\.(jpg|png|heic)");
    assert!(pat.matches(r"C:\Users\Pictures\vacation.jpg", false, fold()));
    assert!(pat.matches(r"C:\Users\rnio\photo.png", false, fold()));
    assert!(
        !pat.matches(r"D:\Photos\vacation.jpg", false, fold()),
        "wrong drive prefix"
    );
    assert!(
        !pat.matches(r"C:\Users\file.jpg.tmp", false, fold()),
        "extension mid-name"
    );
}

#[test]
fn regex_digit_pattern_still_anchored() {
    let pat = compile(r">file\d+\.txt");
    assert!(pat.matches("file123.txt", false, fold()));
    assert!(!pat.matches("file123.txt.bak", false, fold()));
    assert!(!pat.matches("fileABC.txt", false, fold()));
}

#[test]
fn invalid_regex_surfaces_at_compile_time() {
    // Parse stage succeeds — the leading `>` marks it as regex.
    // Validation happens inside compile_parsed_pattern when regex::Regex
    // fails to build the unanchored / anchored / case-folded variants.
    let parsed = ParsedPattern::parse(">[invalid(regex").expect("parse must succeed");
    compile_parsed_pattern(&parsed).expect_err("malformed regex must error");
}
