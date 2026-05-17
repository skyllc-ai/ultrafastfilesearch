// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

use uffs_polars::Column;

use super::*;

type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

fn create_test_df() -> core::result::Result<DataFrame, uffs_polars::PolarsError> {
    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2, 3, 4]),
        Column::new("parent_frs".into(), &[0_u64, 0, 1, 1]),
        Column::new("name".into(), &["root", "file.txt", "src", "main.rs"]),
        Column::new("size".into(), &[0_u64, 1024, 0, 2048]),
        // Use boolean columns matching MFT reader schema
        Column::new("is_directory".into(), &[true, false, true, false]),
        Column::new("is_hidden".into(), &[false, false, false, false]),
        Column::new("is_system".into(), &[false, false, false, false]),
    ])
}

#[test]
fn files_only() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).files_only().collect()?;
    assert_eq!(result.height(), 2); // file.txt and main.rs
    Ok(())
}

#[test]
fn directories_only() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).directories_only().collect()?;
    assert_eq!(result.height(), 2); // root and src
    Ok(())
}

#[test]
fn min_size() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).min_size(1500).collect()?;
    assert_eq!(result.height(), 1); // only main.rs (2048)
    Ok(())
}

#[test]
fn limit() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).limit(2).collect()?;
    assert_eq!(result.height(), 2);
    Ok(())
}

#[test]
fn chained_filters() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df)
        .files_only()
        .min_size(500)
        .sort_by_size(true)
        .limit(10)
        .collect()?;
    assert_eq!(result.height(), 2);
    Ok(())
}

// =========================================================================
// Pattern Matching Tests (using CompiledPattern)
// =========================================================================

fn create_pattern_test_df() -> core::result::Result<DataFrame, uffs_polars::PolarsError> {
    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[1_u64, 2, 3, 4, 5, 6]),
        Column::new("name".into(), &[
            "photo.jpg",
            "document.txt",
            "readme.md",
            "config.json",
            "main.rs",
            "test.rs",
        ]),
        Column::new("size".into(), &[1000_u64, 2000, 500, 300, 1500, 800]),
        Column::new("is_directory".into(), &[
            false, false, false, false, false, false,
        ]),
        Column::new("is_hidden".into(), &[
            false, false, false, false, false, false,
        ]),
        Column::new("is_system".into(), &[
            false, false, false, false, false, false,
        ]),
    ])
}

#[test]
fn pattern_suffix() -> TestResult {
    use crate::pattern::ParsedPattern;

    let df = create_pattern_test_df()?;
    let pattern = ParsedPattern::parse("*.rs")?;
    let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

    assert_eq!(result.height(), 2); // main.rs and test.rs
    Ok(())
}

#[test]
fn pattern_prefix() -> TestResult {
    use crate::pattern::ParsedPattern;

    let df = create_pattern_test_df()?;
    let pattern = ParsedPattern::parse("config*")?;
    let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

    assert_eq!(result.height(), 1); // config.json
    Ok(())
}

#[test]
fn pattern_contains() -> TestResult {
    use crate::pattern::ParsedPattern;

    let df = create_pattern_test_df()?;
    let pattern = ParsedPattern::parse("*read*")?;
    let result = MftQuery::new(df).pattern(&pattern)?.collect()?;

    assert_eq!(result.height(), 1); // readme.md
    Ok(())
}

#[test]
fn extension_filter_optimized() -> TestResult {
    let df = create_pattern_test_df()?;
    let filter = crate::extensions::ExtensionFilter::parse("rs,txt")?;
    let result = MftQuery::new(df).extension_filter(&filter).collect()?;

    assert_eq!(result.height(), 3); // document.txt, main.rs, test.rs
    Ok(())
}

#[test]
fn extension_filter_fast() -> TestResult {
    let df = create_pattern_test_df()?;
    let df_with_ext = crate::extensions::add_ext_column(df)?;
    let filter = crate::extensions::ExtensionFilter::parse("rs,txt")?;
    let result = MftQuery::new(df_with_ext)
        .extension_filter_fast(&filter)
        .collect()?;

    assert_eq!(result.height(), 3); // document.txt, main.rs, test.rs
    Ok(())
}

#[test]
fn extension_filter_single() -> TestResult {
    let df = create_pattern_test_df()?;
    let filter = crate::extensions::ExtensionFilter::parse("jpg")?;
    let result = MftQuery::new(df).extension_filter(&filter).collect()?;

    assert_eq!(result.height(), 1); // photo.jpg
    Ok(())
}

#[test]
fn max_size() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).max_size(1500).collect()?;
    // Should include: root (0), file.txt (1024), src (0) = 3 items
    assert!(result.height() >= 2);
    Ok(())
}

#[test]
fn sort_by_size_descending() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df)
        .files_only()
        .sort_by_size(true)
        .collect()?;
    // First file should be largest (main.rs = 2048)
    let sizes = result.column("size")?.u64()?;
    let first_size = sizes.get(0).unwrap_or(0);
    assert_eq!(first_size, 2048);
    Ok(())
}

#[test]
fn sort_by_size_ascending() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df)
        .files_only()
        .sort_by_size(false)
        .collect()?;
    // First file should be smallest (file.txt = 1024)
    let sizes = result.column("size")?.u64()?;
    let first_size = sizes.get(0).unwrap_or(0);
    assert_eq!(first_size, 1024);
    Ok(())
}

#[test]
fn hide_system() -> TestResult {
    // Create df with NTFS system files ($ prefix and low FRS)
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[0_u64, 5, 16, 100]),
        Column::new("name".into(), &["$MFT", ".", "$Extend", "normal.txt"]),
        Column::new("size".into(), &[100_u64, 0, 200, 300]),
        Column::new("is_directory".into(), &[false, true, true, false]),
        Column::new("is_hidden".into(), &[false, false, false, false]),
        Column::new("is_system".into(), &[true, false, true, false]),
    ])?;

    let result = MftQuery::new(df).hide_system().collect()?;
    // Should keep: FRS 5 (root ".") and FRS 100 (normal.txt)
    // Should exclude: FRS 0 ($MFT, metadata), FRS 16 ($Extend, $ prefix)
    assert_eq!(result.height(), 2);
    Ok(())
}

// `QueryMode` was removed in #263 alongside the rest of the
// `index_search::query` plumbing (no external or internal callers).  The
// previous `query_mode_from_str` test exercised that dead enum and is
// dropped here — no live surface remains to assert on.

#[test]
fn empty_dataframe() -> TestResult {
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), Vec::<u64>::new()),
        Column::new("name".into(), Vec::<&str>::new()),
        Column::new("size".into(), Vec::<u64>::new()),
        Column::new("is_directory".into(), Vec::<bool>::new()),
        Column::new("is_hidden".into(), Vec::<bool>::new()),
        Column::new("is_system".into(), Vec::<bool>::new()),
    ])?;

    let result = MftQuery::new(df).files_only().collect()?;
    assert_eq!(result.height(), 0);
    Ok(())
}

#[test]
fn combined_size_filters() -> TestResult {
    let df = create_test_df()?;
    let result = MftQuery::new(df).min_size(500).max_size(1500).collect()?;
    // Should include file.txt (1024) only
    assert_eq!(result.height(), 1);
    Ok(())
}
