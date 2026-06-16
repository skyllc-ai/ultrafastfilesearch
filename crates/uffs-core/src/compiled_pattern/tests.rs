// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for compiled pattern classification, compilation, and lowering.

use super::*;
use crate::pattern::ParsedPattern;

type TestResult = Result<(), Box<dyn core::error::Error>>;

// ============================================================================
// GlobKind Classification Tests
// ============================================================================

#[test]
fn classify_any() {
    assert!(matches!(classify_glob("*"), GlobKind::Any));
}

#[test]
fn classify_exact() {
    let kind = classify_glob("readme.txt");
    assert!(matches!(kind, GlobKind::Exact(val) if val == "readme.txt"));
}

#[test]
fn classify_prefix() {
    let kind = classify_glob("foo*");
    assert!(matches!(kind, GlobKind::Prefix(val) if val == "foo"));
}

#[test]
fn classify_suffix() {
    let kind = classify_glob("*bar");
    assert!(matches!(kind, GlobKind::Suffix(val) if val == "bar"));
}

#[test]
fn classify_extension() {
    let kind = classify_glob("*.txt");
    assert!(matches!(kind, GlobKind::Extension(val) if val == "txt"));
}

#[test]
fn classify_extension_multi_part() {
    // *.tar.gz should be Suffix, not Extension (has multiple dots)
    let kind = classify_glob("*.tar.gz");
    assert!(matches!(kind, GlobKind::Suffix(val) if val == ".tar.gz"));
}

#[test]
fn classify_contains() {
    let kind = classify_glob("*needle*");
    assert!(matches!(kind, GlobKind::Contains(val) if val == "needle"));
}

#[test]
fn classify_prefix_suffix() {
    let kind = classify_glob("foo*bar");
    assert!(
        matches!(kind, GlobKind::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == "bar")
    );
}

#[test]
fn classify_prefix_suffix_with_extension() {
    let kind = classify_glob("foo*.txt");
    assert!(
        matches!(kind, GlobKind::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == ".txt")
    );
}

#[test]
fn classify_complex_question_mark() {
    let kind = classify_glob("file?.txt");
    assert!(matches!(kind, GlobKind::Complex(_)));
}

#[test]
fn classify_complex_bracket() {
    let kind = classify_glob("[abc]*");
    assert!(matches!(kind, GlobKind::Complex(_)));
}

#[test]
fn classify_complex_double_star() {
    let kind = classify_glob("**/*.rs");
    assert!(matches!(kind, GlobKind::Complex(_)));
}

#[test]
fn classify_complex_multiple_stars() {
    let kind = classify_glob("a*b*c");
    assert!(matches!(kind, GlobKind::Complex(_)));
}

// ============================================================================
// CompiledPattern Compilation Tests
// ============================================================================

#[test]
fn compile_literal() -> TestResult {
    let parsed = ParsedPattern::parse("readme")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "readme"));
    Ok(())
}

#[test]
fn compile_glob_any() -> TestResult {
    let parsed = ParsedPattern::parse("*")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Any));
    Ok(())
}

#[test]
fn compile_glob_prefix() -> TestResult {
    let parsed = ParsedPattern::parse("foo*")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Prefix(val) if val == "foo"));
    Ok(())
}

#[test]
fn compile_glob_suffix() -> TestResult {
    let parsed = ParsedPattern::parse("*bar")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Suffix(val) if val == "bar"));
    Ok(())
}

#[test]
fn compile_glob_extension() -> TestResult {
    let parsed = ParsedPattern::parse("*.txt")?;
    let compiled = compile_pattern(&parsed)?;
    // Extension becomes Suffix with the dot
    assert!(matches!(compiled, CompiledPattern::Suffix(val) if val == ".txt"));
    Ok(())
}

#[test]
fn compile_glob_contains() -> TestResult {
    let parsed = ParsedPattern::parse("*needle*")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "needle"));
    Ok(())
}

#[test]
fn compile_glob_prefix_suffix() -> TestResult {
    let parsed = ParsedPattern::parse("foo*bar")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(
        matches!(compiled, CompiledPattern::PrefixSuffix { prefix, suffix } if prefix == "foo" && suffix == "bar")
    );
    Ok(())
}

#[test]
fn compile_glob_complex() -> TestResult {
    let parsed = ParsedPattern::parse("file?.txt")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Regex {
        anchored: true,
        ..
    }));
    Ok(())
}

#[test]
fn compile_regex() -> TestResult {
    let parsed = ParsedPattern::parse(r">.*\.log$")?;
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Regex {
        anchored: false,
        ..
    }));
    Ok(())
}

#[test]
fn compile_exact_no_wildcards() -> TestResult {
    let parsed = ParsedPattern::parse("README.md")?;
    // This is detected as Literal by ParsedPattern, so becomes Contains
    let compiled = compile_pattern(&parsed)?;
    assert!(matches!(compiled, CompiledPattern::Contains(val) if val == "README.md"));
    Ok(())
}

// ============================================================================
// Expression Lowering Tests (to_expr)
// ============================================================================

#[test]
fn to_expr_any() {
    let pattern = CompiledPattern::Any;
    let expr = pattern.to_expr("name", true);
    // Should produce lit(true)
    let expr_str = format!("{expr:?}");
    assert!(expr_str.contains("true"), "Any should produce lit(true)");
}

#[test]
fn to_expr_exact() {
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
fn to_expr_prefix() {
    let pattern = CompiledPattern::Prefix("foo".to_owned());
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("starts_with"),
        "Prefix should use starts_with: {expr_str}"
    );
}

#[test]
fn to_expr_suffix() {
    let pattern = CompiledPattern::Suffix(".txt".to_owned());
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("ends_with"),
        "Suffix should use ends_with: {expr_str}"
    );
}

#[test]
fn to_expr_contains() {
    let pattern = CompiledPattern::Contains("needle".to_owned());
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("contains"),
        "Contains should use contains: {expr_str}"
    );
}

#[test]
fn to_expr_prefix_suffix() {
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
fn to_expr_exact_set() {
    let pattern = CompiledPattern::ExactSet(vec!["README.md".to_owned(), "LICENSE".to_owned()]);
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("is_in"),
        "ExactSet should use is_in: {expr_str}"
    );
}

#[test]
fn to_expr_contains_any() {
    let pattern = CompiledPattern::ContainsAny(vec!["foo".to_owned(), "bar".to_owned()]);
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("contains_any"),
        "ContainsAny should use contains_any: {expr_str}"
    );
}

#[test]
fn to_expr_suffix_set() {
    let pattern = CompiledPattern::SuffixSet(vec![".txt".to_owned(), ".md".to_owned()]);
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    assert!(
        expr_str.contains("ends_with"),
        "SuffixSet should use ends_with: {expr_str}"
    );
}

#[test]
fn to_expr_regex() {
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
fn to_expr_case_insensitive() {
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
fn to_expr_suffix_set_empty() {
    let pattern = CompiledPattern::SuffixSet(vec![]);
    let expr = pattern.to_expr("name", true);
    let expr_str = format!("{expr:?}");
    // Empty set should produce lit(false)
    assert!(
        expr_str.contains("false"),
        "Empty SuffixSet should produce lit(false): {expr_str}"
    );
}

// ============================================================================
// Integration Tests - Verify expressions work against real DataFrames
// ============================================================================

#[test]
fn suffix_case_insensitive_integration() -> TestResult {
    use uffs_polars::{Column, DataFrame, IntoLazy as _};

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
fn suffix_case_sensitive_integration() -> TestResult {
    use uffs_polars::{Column, DataFrame, IntoLazy as _};

    let input_names = vec![
        "file.TXT",
        "FILE.txt",
        "$recycle.txt",
        "$I07QSZ8.TXT",
        "test.TXT",
    ];
    let df = DataFrame::new(input_names.len(), vec![Column::new(
        "name".into(),
        &input_names,
    )])?;

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
fn dollar_prefix_files_matched() -> TestResult {
    use uffs_polars::{Column, DataFrame, IntoLazy as _};

    let input_names = vec![
        "$MFT",
        "$recycle.bin",
        "$I07QSZ8.txt",
        "normal.txt",
        "$BITMAP",
    ];
    let df = DataFrame::new(input_names.len(), vec![Column::new(
        "name".into(),
        &input_names,
    )])?;

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
    let matched_names: Vec<&str> = result.column("name")?.str()?.iter().flatten().collect();
    assert!(
        matched_names.contains(&"$I07QSZ8.txt") || matched_names.contains(&"$i07qsz8.txt"),
        "Should include $I07QSZ8.txt: {matched_names:?}"
    );

    Ok(())
}

#[test]
fn null_values_not_filtered() -> TestResult {
    use uffs_polars::{Column, DataFrame, IntoLazy as _};

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
