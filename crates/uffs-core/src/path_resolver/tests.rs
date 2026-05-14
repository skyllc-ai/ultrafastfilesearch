// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for `path_resolver` module behavior.
//!
//! Covers legacy and fast resolver path reconstruction, caching, and helper
//! column generation.

use uffs_polars::{Column, DataFrame};

use super::*;

type TestResult = Result<(), Box<dyn core::error::Error>>;

fn create_test_df() -> Result<DataFrame, uffs_polars::PolarsError> {
    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[5_u64, 100, 101, 102]),
        Column::new("parent_frs".into(), &[0_u64, 5, 100, 101]),
        Column::new("name".into(), &["", "Users", "john", "Documents"]),
    ])
}

// ═══════════════════════════════════════════════════════════════════════════
// PathResolver tests (HashMap-based)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn resolve_path() -> TestResult {
    let df = create_test_df()?;
    let mut resolver = PathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    let path = resolver.resolve(102)?;
    assert_eq!(path, "C:\\Users\\john\\Documents");
    Ok(())
}

#[test]
fn path_caching() -> TestResult {
    let df = create_test_df()?;
    let mut resolver = PathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    // First resolution
    let path1 = resolver.resolve(102)?;
    // Second resolution (should use cache)
    let path2 = resolver.resolve(102)?;

    assert_eq!(path1, path2);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// FastPathResolver tests (Vec-based)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fast_resolve_path() -> TestResult {
    let df = create_test_df()?;
    let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    let path = resolver.resolve(102);
    assert_eq!(path, "C:\\Users\\john\\Documents");
    Ok(())
}

#[test]
fn fast_resolve_root() -> TestResult {
    let df = create_test_df()?;
    let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    // Root directory (FRS 5) should resolve to just "C:\"
    let path = resolver.resolve(5);
    assert_eq!(path, "C:\\");
    Ok(())
}

#[test]
fn fast_resolve_cached() -> TestResult {
    let df = create_test_df()?;
    let mut resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    // First resolution (builds and caches)
    let path1 = resolver.resolve_cached(102);
    // Second resolution (uses cache)
    let path2 = resolver.resolve_cached(102);

    assert_eq!(path1, path2);
    assert_eq!(path1, "C:\\Users\\john\\Documents");

    // Check stats show cached path
    let stats = resolver.stats();
    assert!(stats.cached_paths >= 1);
    Ok(())
}

#[test]
fn fast_resolve_missing_frs() -> TestResult {
    let df = create_test_df()?;
    let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    // FRS 999 doesn't exist
    let path = resolver.resolve(999);
    assert!(path.starts_with("<unknown:"));
    Ok(())
}

#[test]
fn fast_add_path_column() -> TestResult {
    let df = create_test_df()?;
    let mut resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    let result = resolver.add_path_column(&df)?;

    // Check that path column was added
    let path_col = result.column("path")?.str()?;
    assert_eq!(path_col.len(), 4);

    // Check specific paths
    assert_eq!(path_col.get(3), Some("C:\\Users\\john\\Documents"));
    Ok(())
}

#[test]
fn fast_resolver_stats() -> TestResult {
    let df = create_test_df()?;
    let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    let stats = resolver.stats();
    assert_eq!(stats.entry_count, 4);
    assert!(stats.name_arena_bytes > 0);
    assert!(stats.entries_vec_bytes > 0);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// NameArena tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn name_arena() {
    let mut arena = NameArena::with_capacity(100);

    let (off1, len1) = arena.add("hello");
    let (off2, len2) = arena.add("world");

    assert_eq!(arena.get(off1, len1), "hello");
    assert_eq!(arena.get(off2, len2), "world");
    assert_eq!(arena.len(), 10); // "hello" + "world"
}

#[test]
fn name_arena_empty() {
    let arena = NameArena::with_capacity(100);
    assert!(arena.is_empty());
    assert_eq!(arena.len(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Parallel path resolution tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fast_add_path_column_parallel() -> TestResult {
    let df = create_test_df()?;
    let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    let result = resolver.add_path_column_parallel(&df)?;

    let path_col = result.column("path")?.str()?;

    // Check that paths are resolved correctly
    let paths: Vec<_> = path_col.into_iter().collect();
    assert!(
        paths
            .iter()
            .any(|path| path.is_some_and(|str_val| str_val.contains("Users")))
    );
    Ok(())
}

#[test]
fn fast_add_path_column_auto() -> TestResult {
    let df = create_test_df()?;
    let mut resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)?;

    // Small DataFrame should use sequential
    let result = resolver.add_path_column_auto(&df)?;

    result.column("path")?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// path_only column tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn add_path_only_column_works() -> TestResult {
    // Create a DataFrame with path column
    let df = DataFrame::new_infer_height(vec![Column::new("path".into(), &[
        "G:\\",
        "G:\\MFT_TEST\\",
        "G:\\MFT_TEST\\Backup\\",
        "G:\\MFT_TEST\\Backup\\backup1.bak",
        "G:\\MFT_TEST\\Backup\\doc1_hardlink.txt",
    ])])?;

    let result = add_path_only_column(&df)?;

    let path_only_col = result.column("path_only")?.str()?;

    // Check values
    assert_eq!(path_only_col.get(0), Some("G:\\"));
    assert_eq!(path_only_col.get(1), Some("G:\\MFT_TEST\\"));
    assert_eq!(path_only_col.get(2), Some("G:\\MFT_TEST\\Backup\\"));
    assert_eq!(path_only_col.get(3), Some("G:\\MFT_TEST\\Backup\\"));
    assert_eq!(path_only_col.get(4), Some("G:\\MFT_TEST\\Backup\\"));

    Ok(())
}
