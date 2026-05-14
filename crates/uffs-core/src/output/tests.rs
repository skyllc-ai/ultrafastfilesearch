// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for output column parsing and formatting behavior.

use uffs_polars::{Column, DataFrame};

use super::*;

#[test]
fn parse_column() {
    assert_eq!(OutputColumn::parse("path"), Some(OutputColumn::Path));
    assert_eq!(OutputColumn::parse("PATH"), Some(OutputColumn::Path));
    assert_eq!(OutputColumn::parse("size"), Some(OutputColumn::Size));
    assert_eq!(OutputColumn::parse("unknown"), None);
}

#[test]
fn parse_columns_all() {
    assert!(OutputConfig::parse_columns("all").is_none());
    assert!(OutputConfig::parse_columns("ALL").is_none());
}

#[test]
fn parse_columns_list() {
    let cols = OutputConfig::parse_columns("path,name,size").expect("should parse");
    assert_eq!(cols.len(), 3);
    assert_eq!(cols.first(), Some(&OutputColumn::Path));
    assert_eq!(cols.get(1), Some(&OutputColumn::Name));
    assert_eq!(cols.get(2), Some(&OutputColumn::Size));
}

#[test]
fn parse_columns_with_spaces() {
    // "Name,Size,Path Only" — the exact failing case from CLI validation test 22
    let cols = OutputConfig::parse_columns("Name,Size,Path Only").expect("should parse");
    assert_eq!(cols.len(), 3);
    assert_eq!(cols.first(), Some(&OutputColumn::Name));
    assert_eq!(cols.get(1), Some(&OutputColumn::Size));
    assert_eq!(cols.get(2), Some(&OutputColumn::PathOnly));
}

#[test]
fn parse_columns_size_on_disk() {
    let cols =
        OutputConfig::parse_columns("Name,Size on Disk,Directory Flag").expect("should parse");
    assert_eq!(cols.len(), 3);
    assert_eq!(cols.first(), Some(&OutputColumn::Name));
    assert_eq!(cols.get(1), Some(&OutputColumn::SizeOnDisk));
    assert_eq!(cols.get(2), Some(&OutputColumn::DirectoryFlag));
}

#[test]
fn parse_separator_special() {
    // Original values
    assert_eq!(OutputConfig::parse_separator("TAB"), "\t");
    assert_eq!(OutputConfig::parse_separator("tab"), "\t");
    assert_eq!(OutputConfig::parse_separator("NEWLINE"), "\n");
    assert_eq!(OutputConfig::parse_separator(";"), ";");
    // Legacy compatibility values
    assert_eq!(OutputConfig::parse_separator("NEW LINE"), "\n");
    assert_eq!(OutputConfig::parse_separator("SPACE"), " ");
    assert_eq!(OutputConfig::parse_separator("RETURN"), "\r");
    assert_eq!(OutputConfig::parse_separator("DOUBLE"), "\"");
    assert_eq!(OutputConfig::parse_separator("SINGLE"), "'");
    assert_eq!(OutputConfig::parse_separator("NULL"), "\0");
}

#[test]
fn parse_column_aliases() {
    // Short aliases for legacy compatibility
    assert_eq!(OutputColumn::parse("r"), Some(OutputColumn::ReadOnly));
    assert_eq!(OutputColumn::parse("a"), Some(OutputColumn::Archive));
    assert_eq!(OutputColumn::parse("s"), Some(OutputColumn::System));
    assert_eq!(OutputColumn::parse("h"), Some(OutputColumn::Hidden));
    assert_eq!(OutputColumn::parse("o"), Some(OutputColumn::Offline));
    // Legacy name aliases
    assert_eq!(OutputColumn::parse("written"), Some(OutputColumn::Modified));
    assert_eq!(
        OutputColumn::parse("notcontent"),
        Some(OutputColumn::NotIndexed)
    );
    assert_eq!(
        OutputColumn::parse("directory"),
        Some(OutputColumn::DirectoryFlag)
    );
    // Legacy typo support
    assert_eq!(
        OutputColumn::parse("decendents"),
        Some(OutputColumn::Descendants)
    );
    // Space-separated display names
    assert_eq!(
        OutputColumn::parse("path only"),
        Some(OutputColumn::PathOnly)
    );
    assert_eq!(
        OutputColumn::parse("size on disk"),
        Some(OutputColumn::SizeOnDisk)
    );
    assert_eq!(
        OutputColumn::parse("directory flag"),
        Some(OutputColumn::DirectoryFlag)
    );
    assert_eq!(
        OutputColumn::parse("no scrub file"),
        Some(OutputColumn::NoScrub)
    );
    assert_eq!(
        OutputColumn::parse("recall on open"),
        Some(OutputColumn::RecallOnOpen)
    );
    assert_eq!(
        OutputColumn::parse("recall on data access"),
        Some(OutputColumn::RecallOnDataAccess)
    );
    assert_eq!(
        OutputColumn::parse("read-only"),
        Some(OutputColumn::ReadOnly)
    );
    assert_eq!(
        OutputColumn::parse("not content indexed file"),
        Some(OutputColumn::NotIndexed)
    );
}

#[test]
fn output_config_builder() {
    let config = OutputConfig::new()
        .with_columns("path,name")
        .with_separator(";")
        .with_quote("'")
        .with_header(false)
        .with_pos("+")
        .with_neg("-");

    assert!(config.columns.is_some());
    assert_eq!(config.separator, ";");
    assert_eq!(config.quote, "'");
    assert!(!config.header);
    assert_eq!(config.pos, "+");
    assert_eq!(config.neg, "-");
}

#[test]
fn df_column_mapping() {
    assert_eq!(OutputColumn::Path.df_column(), "path");
    assert_eq!(OutputColumn::SizeOnDisk.df_column(), "allocated_size");
    assert_eq!(OutputColumn::AttributeValue.df_column(), "flags");
}

#[test]
fn display_name() {
    assert_eq!(OutputColumn::Path.display_name(), "Path");
    assert_eq!(OutputColumn::SizeOnDisk.display_name(), "Size on Disk");
    assert_eq!(
        OutputColumn::NotIndexed.display_name(),
        "Not content indexed file"
    );
}

#[test]
fn needs_descendants() {
    let config_no_desc = OutputConfig::new().with_columns("path,name,size");
    assert!(!config_no_desc.needs_descendants());

    let config_with_desc = OutputConfig::new().with_columns("path,descendants,size");
    assert!(config_with_desc.needs_descendants());

    // "all" columns returns None, so needs_descendants should be false
    let config_all = OutputConfig::new().with_columns("all");
    assert!(!config_all.needs_descendants());
}

#[test]
fn add_descendants_column_works() {
    // Create a test DataFrame with directory structure:
    // root (5) -> Users (100) -> john (101) -> file.txt (102)
    //                         -> Documents (103) -> doc.pdf (104)
    let df = DataFrame::new_infer_height(vec![
        Column::new("frs".into(), &[5_u64, 100, 101, 102, 103, 104]),
        Column::new("parent_frs".into(), &[0_u64, 5, 100, 101, 100, 103]),
        Column::new("is_directory".into(), &[
            true, true, true, false, true, false,
        ]),
        Column::new("size".into(), &[0_u64, 0, 0, 1000, 0, 50000]),
        Column::new("allocated_size".into(), &[
            4096_u64, 4096, 4096, 4096, 4096, 53248,
        ]),
    ])
    .unwrap();

    let result = add_descendants_column(&df).unwrap();

    // Check that descendants column was added
    let desc_col = result.column("descendants").unwrap().u64().unwrap();

    // Descendants = direct children + all their descendants (recursive)
    // root (5): children=[Users(100)] -> 1 + descendants(100) = 1 + 4 = 5
    // Users (100): children=[john(101), Documents(103)] -> 2 + desc(101) +
    // desc(103) = 2 + 1 + 1 = 4 john (101): children=[file.txt(102)] -> 1 +
    // 0 = 1 file.txt (102): 0 (file)
    // Documents (103): children=[doc.pdf(104)] -> 1 + 0 = 1
    // doc.pdf (104): 0 (file)
    assert_eq!(desc_col.get(0), Some(5)); // root -> all 5 items below
    assert_eq!(desc_col.get(1), Some(4)); // Users -> john, file.txt, Documents, doc.pdf
    assert_eq!(desc_col.get(2), Some(1)); // john -> file.txt
    assert_eq!(desc_col.get(3), Some(0)); // file.txt (file)
    assert_eq!(desc_col.get(4), Some(1)); // Documents -> doc.pdf
    assert_eq!(desc_col.get(5), Some(0)); // doc.pdf (file)
}

#[test]
fn write_preserves_header_spacing_and_missing_column_defaults() {
    let df = DataFrame::new_infer_height(vec![
        Column::new("path".into(), &["C:\\Temp\\file.txt"]),
        Column::new("name".into(), &["file.txt"]),
    ])
    .unwrap();

    let config = OutputConfig::new()
        .with_columns("path,pathonly,descendants,name")
        .with_quote("'");

    let mut output = Vec::new();
    config.write(&df, &mut output).unwrap();

    assert_eq!(
        String::from_utf8(output).unwrap(),
        concat!(
            "'Path','Path Only','Descendants','Name'\n",
            "\n",
            "'C:\\Temp\\file.txt',,0,'file.txt'\n"
        )
    );
}

#[test]
fn write_preserves_null_and_boolean_value_formatting() {
    let df = DataFrame::new_infer_height(vec![
        Column::new("name".into(), &[Some("alpha"), None, Some("beta")]),
        Column::new("size".into(), &[Some(42_u64), None, Some(0_u64)]),
        Column::new("is_archive".into(), &[Some(true), Some(false), None]),
    ])
    .unwrap();

    let config = OutputConfig::new()
        .with_columns("name,size,archive")
        .with_header(false)
        .with_quote("'")
        .with_pos("+")
        .with_neg("-");

    let mut output = Vec::new();
    config.write(&df, &mut output).unwrap();

    assert_eq!(
        String::from_utf8(output).unwrap(),
        concat!("'alpha',42,+\n", ",0,-\n", "'beta',0,\n")
    );
}

#[test]
fn parity_compat_directory_formatting() {
    use crate::search::backend::DisplayRow;

    let file_row = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Temp\\hello.txt".to_owned(),
        1024,
        false,
        0,
        0,
        0,
        0x20,
        4096,
        0,
        0,
        0,
    );

    let dir_row = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Temp".to_owned(),
        0,
        true,
        0,
        0,
        0,
        0x10,
        4096,
        5,
        9999,
        55555,
    );

    // ── Without parity_compat ──
    let normal_config = OutputConfig::new()
        .with_columns("path,name,pathonly,size,sizeondisk")
        .with_header(false)
        .with_parity_compat(false);

    let mut normal_out = Vec::new();
    normal_config
        .write_display_rows(core::slice::from_ref(&dir_row), &mut normal_out)
        .unwrap();
    let normal = String::from_utf8(normal_out).unwrap();

    // Normal mode: path has NO trailing `\`, name = "Temp", size = 0, sizeondisk =
    // own allocated
    assert!(
        normal.contains("\"C:\\Temp\""),
        "normal: path without trailing \\"
    );
    assert!(normal.contains("\"Temp\""), "normal: actual name");
    assert!(
        normal.contains(",0,4096\n"),
        "normal: size=0, sizeondisk=4096 (own allocated)"
    );

    // ── With parity_compat ──
    let parity_config = OutputConfig::new()
        .with_columns("path,name,pathonly,size,sizeondisk")
        .with_header(false)
        .with_parity_compat(true);

    let mut parity_out = Vec::new();
    parity_config
        .write_display_rows(core::slice::from_ref(&dir_row), &mut parity_out)
        .unwrap();
    let parity = String::from_utf8(parity_out).unwrap();

    // Parity mode: path trailing `\`, empty name, size=treesize,
    // sizeondisk=tree_allocated
    assert!(
        parity.contains("\"C:\\Temp\\\""),
        "parity: path with trailing \\"
    );
    assert!(parity.contains(",\"\","), "parity: empty name");
    assert!(
        parity.contains("\"C:\\Temp\\\""),
        "parity: pathonly = full path with \\"
    );
    assert!(
        parity.contains(",9999,55555\n"),
        "parity: size=treesize, sizeondisk=tree_allocated"
    );

    // Files should NOT be affected by parity_compat
    let mut file_out = Vec::new();
    parity_config
        .write_display_rows(&[file_row], &mut file_out)
        .unwrap();
    let parity_file = String::from_utf8(file_out).unwrap();

    assert!(
        parity_file.contains("\"C:\\Temp\\hello.txt\""),
        "file path unchanged"
    );
    assert!(parity_file.contains("\"hello.txt\""), "file name unchanged");
    assert!(
        parity_file.contains(",1024,4096\n"),
        "file: size=own, sizeondisk=own allocated"
    );
}

/// Regression test: root directory path must NOT get a double trailing
/// backslash in parity mode. Root's path is already `G:\` — appending `\` would
/// produce `G:\\` which mismatches the legacy baseline.
#[test]
fn parity_root_no_double_trailing_backslash() {
    use crate::search::backend::DisplayRow;

    let root_row = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::G,
        "G:\\".to_owned(),
        0,
        true,
        0,
        0,
        0,
        0x10,
        0,
        100,
        500_000,
        600_000,
    );

    let config = OutputConfig::new()
        .with_columns("path,name,pathonly,size,sizeondisk")
        .with_header(false)
        .with_parity_compat(true);

    let mut out = Vec::new();
    config.write_display_rows(&[root_row], &mut out).unwrap();
    let output = String::from_utf8(out).unwrap();

    // Path must be `"G:\"` (single trailing backslash), NOT `"G:\\"`.
    assert!(
        output.contains("\"G:\\\""),
        "root path must have single trailing backslash, got: {output}"
    );
    assert!(
        !output.contains("\"G:\\\\\""),
        "root path must NOT have double trailing backslash, got: {output}"
    );
    // Root name must be empty in parity mode.
    assert!(
        output.contains(",\"\","),
        "root name must be empty in parity mode, got: {output}"
    );
    // Size = treesize, SizeOnDisk = tree_allocated.
    assert!(
        output.contains(",500000,600000\n"),
        "root: size=treesize, sizeondisk=tree_allocated, got: {output}"
    );
}

// ── Regression tests for T101–T118: computed columns in `--columns all` ──

/// `BASELINE_COLUMN_ORDER` ("--columns all") must include Tree Size, Tree
/// Allocated, Bulkiness, Type, Extension, Name Length, and Path Length.
#[test]
fn baseline_column_order_includes_computed_columns() {
    use super::BASELINE_COLUMN_ORDER;

    let has = |col: OutputColumn| BASELINE_COLUMN_ORDER.contains(&col);
    assert!(has(OutputColumn::TreeSize), "TreeSize missing from all");
    assert!(
        has(OutputColumn::TreeAllocated),
        "TreeAllocated missing from all"
    );
    assert!(has(OutputColumn::Bulkiness), "Bulkiness missing from all");
    assert!(has(OutputColumn::Type), "Type missing from all");
    assert!(has(OutputColumn::Extension), "Extension missing from all");
    assert!(has(OutputColumn::NameLength), "NameLength missing from all");
    assert!(has(OutputColumn::PathLength), "PathLength missing from all");
}

/// Display names must use spaces for multi-word columns so that CSV header
/// lookups like `col_val(row, &h, "Tree Size")` succeed.
#[test]
fn tree_column_display_names_have_spaces() {
    assert_eq!(
        OutputColumn::TreeSize.display_name(),
        "Tree Size",
        "TreeSize display name must be 'Tree Size'"
    );
    assert_eq!(
        OutputColumn::TreeAllocated.display_name(),
        "Tree Allocated",
        "TreeAllocated display name must be 'Tree Allocated'"
    );
}

/// Regression T101/T118: `write_display_rows` must emit `TreeSize` and
/// `TreeAllocated` values for directory rows when those columns are requested.
#[test]
fn write_display_rows_emits_treesize_and_tree_allocated() {
    use crate::search::backend::DisplayRow;

    let dir_row = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Big".to_owned(),
        0,
        true,
        0,
        0,
        0,
        0x10,
        4096,
        42,
        104_857_600, // treesize = 100 MB
        209_715_200, // tree_allocated = 200 MB
    );

    let config = OutputConfig::new()
        .with_columns("name,treesize,treeallocated")
        .with_header(true)
        .with_quote("\"");

    let mut out = Vec::new();
    config
        .write_display_rows(core::slice::from_ref(&dir_row), &mut out)
        .unwrap();
    let csv = String::from_utf8(out).unwrap();

    assert!(
        csv.contains("\"Tree Size\""),
        "header must contain 'Tree Size', got: {csv}"
    );
    assert!(
        csv.contains("\"Tree Allocated\""),
        "header must contain 'Tree Allocated', got: {csv}"
    );
    assert!(
        csv.contains("104857600"),
        "treesize value must be emitted, got: {csv}"
    );
    assert!(
        csv.contains("209715200"),
        "tree_allocated value must be emitted, got: {csv}"
    );
}

/// Regression: `NameLength` and `PathLength` columns must emit actual values.
#[test]
fn write_display_rows_emits_name_length_and_path_length() {
    use crate::search::backend::DisplayRow;

    let row = DisplayRow::new(
        0,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Very\\Long\\Path\\readme.txt".to_owned(),
        100,
        false,
        0,
        0,
        0,
        0x20,
        4096,
        0,
        0,
        0,
    );

    let config = OutputConfig::new()
        .with_columns("name,namelength,pathlength")
        .with_header(false)
        .with_quote("\"");

    let mut out = Vec::new();
    config
        .write_display_rows(core::slice::from_ref(&row), &mut out)
        .unwrap();
    let csv = String::from_utf8(out).unwrap();

    // name = "readme.txt" (10 chars), path = "C:\Very\Long\Path\readme.txt" (28
    // chars)
    assert!(csv.contains(",10,"), "name length must be 10, got: {csv}");
    assert!(csv.contains(",28\n"), "path length must be 28, got: {csv}");
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression: Polars `Datetime(TimeUnit::Microseconds)` formatter must
// interpret values as FILETIME, not Unix microseconds.
//
// The DataFrames produced by `MftIndex::to_dataframe` (and the Windows
// live-scan DataFrame in `uffs_mft::reader::dataframe_build`) declare
// timestamp columns as `Datetime(Microseconds)` for Polars compatibility
// but populate them with raw FILETIME i64 values (100-ns ticks since
// 1601-01-01).  The `write_value` formatter at `output::config` must
// therefore decompose the value via `uffs_time::filetime_to_calendar`.
// Previously it used `chrono::DateTime::from_timestamp(micros)` which
// produced year-6220 output for 2026-era FILETIMEs — same bug class as
// the `append_datetime_native` regression covered in `config::tests`.
// ═══════════════════════════════════════════════════════════════════════════

/// Pick one specific FILETIME (2026-01-20 00:00:00 UTC) and assert the
/// Polars-Datetime column formatter emits `2026-01-20 00:00:00`.  If
/// someone re-introduces the Unix-micros interpretation, this renders
/// as `8225-01-17` or similar and the test fails immediately.
#[test]
fn write_datetime_column_formats_filetime_as_2026() {
    use uffs_polars::{DataType, IntoColumn as _, NamedFrom as _, Series, TimeUnit};

    // 2026-01-20 00:00:00 UTC as raw FILETIME.
    // Unix seconds: 1_768_867_200 (2026-01-20).  Convert to 100-ns ticks
    // since 1970, then add FILETIME_UNIX_DIFF to shift the epoch to 1601.
    let ft_2026: i64 = 1_768_867_200_000_000_i64 * 10 + 116_444_736_000_000_000;

    let ts_column = Series::new("modified".into(), vec![ft_2026])
        .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
        .expect("Datetime cast should succeed")
        .into_column();

    let df = DataFrame::new_infer_height(vec![Column::new("name".into(), &["a.txt"]), ts_column])
        .expect("DataFrame construction should succeed");

    let config = OutputConfig::new()
        .with_columns("name,modified")
        .with_header(false)
        .with_quote("\"")
        .with_tz_offset_hours(0_i32);

    let mut out = Vec::new();
    config.write(&df, &mut out).expect("write should succeed");
    let csv = String::from_utf8(out).expect("UTF-8");

    assert!(
        csv.contains("2026-01-20 00:00:00"),
        "Polars Datetime formatter must interpret i64 as FILETIME (expected 2026-01-20, got: {csv})"
    );
    assert!(
        !csv.contains("8225-") && !csv.contains("6220-"),
        "Polars Datetime formatter must NOT treat FILETIME as Unix micros (got: {csv})"
    );
}

/// Regression: parallel write path (>= `PARALLEL_WRITE_THRESHOLD` rows)
/// must emit byte-identical output to the sequential path.
///
/// Pins the v0.5.58 Option D refactor that split `write_display_rows`
/// into a sequential branch (< 16 K rows) and a rayon `par_chunks`
/// branch (>= 16 K).  If a future refactor reintroduces the sequential
/// path for all sizes, or changes the chunk-merge order, this test
/// catches the drift.  Uses 20 K synthetic rows to force the parallel
/// branch (threshold is 16 384) and diffs the bytes against a loop
/// that mirrors the old sequential formatter exactly.
#[test]
fn write_display_rows_parallel_matches_sequential() {
    use crate::search::backend::DisplayRow;

    let rows: Vec<DisplayRow> = (0..20_000_u32)
        .map(|idx| {
            DisplayRow::new(
                idx,
                uffs_mft::platform::DriveLetter::C,
                format!("C:\\tmp\\file_{idx:05}.dll"),
                u64::from(idx) * 1024,
                false,
                i64::from(idx),
                i64::from(idx) + 1,
                i64::from(idx) + 2,
                0x20,
                (u64::from(idx) * 1024).next_multiple_of(4096),
                0,
                0,
                0,
            )
        })
        .collect();

    let config = OutputConfig::new()
        .with_columns("path,size,modified")
        .with_header(false)
        .with_quote("\"")
        .with_tz_offset_hours(0_i32);

    // Parallel branch (>= 16 K rows hits `PARALLEL_WRITE_THRESHOLD`).
    let mut parallel_out = Vec::new();
    config
        .write_display_rows(&rows, &mut parallel_out)
        .expect("parallel write should succeed");

    // Sequential branch: format half the rows twice (two <16 K
    // writes) so the sequential code path runs for every row.
    let mut sequential_out = Vec::new();
    let (half_a, half_b) = rows.split_at(rows.len() / 2);
    config
        .write_display_rows(half_a, &mut sequential_out)
        .expect("sequential write half A should succeed");
    config
        .write_display_rows(half_b, &mut sequential_out)
        .expect("sequential write half B should succeed");

    assert_eq!(
        parallel_out.len(),
        sequential_out.len(),
        "parallel byte length must match sequential",
    );
    assert_eq!(
        parallel_out, sequential_out,
        "parallel bytes must match sequential exactly",
    );
}

/// Zero FILETIME is the NTFS sentinel for "unset" — the formatter must
/// emit an empty field, never decompose to `1601-01-01`.
#[test]
fn write_datetime_column_zero_filetime_is_empty() {
    use uffs_polars::{DataType, IntoColumn as _, NamedFrom as _, Series, TimeUnit};

    let ts_column = Series::new("modified".into(), vec![0_i64])
        .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
        .expect("Datetime cast should succeed")
        .into_column();

    let df =
        DataFrame::new_infer_height(vec![Column::new("name".into(), &["unset.txt"]), ts_column])
            .expect("DataFrame construction should succeed");

    let config = OutputConfig::new()
        .with_columns("name,modified")
        .with_header(false)
        .with_quote("\"")
        .with_tz_offset_hours(0_i32);

    let mut out = Vec::new();
    config.write(&df, &mut out).expect("write should succeed");
    let csv = String::from_utf8(out).expect("UTF-8");

    assert!(
        !csv.contains("1601-"),
        "FILETIME 0 must NOT render as 1601-01-01 (got: {csv})"
    );
}

/// Regression: the `Extension` column must use dot-gated extraction so the
/// displayed value matches the sort engine's key
/// (`crate::search::sorting::build_row_sort_key`, which calls
/// `extract_extension_after_dot`) and the indexer's `intern_extension`
/// semantics.  Pre-fix, naive `rfind('.')` extraction produced
/// `ext = "bash_history"` for `.bash_history`, while the sort key was `""`,
/// causing `--sort extension` ascending to place `.bash_history` at row 1
/// with a displayed `ext` value that violated the asc invariant against
/// every following row.  Caught by Windows MCP T62 (`scripts/tests/
/// definitions/03-sort.toml`).
#[test]
fn extension_column_dot_gated_for_dotfiles_dotless_and_trailing_dot() {
    use crate::search::backend::DisplayRow;

    // Build one DisplayRow per case: dotfile, dotless, trailing-dot,
    // multi-dot directory name, and a normal file (positive control).
    fn row(path: &str) -> DisplayRow {
        DisplayRow::new(
            0,
            uffs_mft::platform::DriveLetter::C,
            path.to_owned(),
            0,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
    }

    let rows = [
        row("F:\\Ch\\.bash_history"),
        row("C:\\Users\\rnio\\.gitignore"),
        row("C:\\tmp\\README"),
        row("C:\\tmp\\foo."),
        row(
            "C:\\Windows\\WinSxS\\amd64_microsoft-windows-mdmappinstaller_31bf3856ad364e35_10.0.26100.8115_none_3591783d4bfd6e96",
        ),
        row("C:\\Projects\\report.txt"),
    ];

    let config = OutputConfig::new()
        .with_columns("name,ext")
        .with_header(false)
        .with_quote("\"");

    let mut out = Vec::new();
    config
        .write_display_rows(&rows, &mut out)
        .expect("write should succeed");
    let csv = String::from_utf8(out).expect("UTF-8");
    assert_eq!(
        csv.lines().count(),
        rows.len(),
        "one output line per row (no header), got: {csv}"
    );

    // Each expected line covers a distinct extension-extraction edge case.
    let expected_lines = [
        // Dotfiles → empty ext.
        "\".bash_history\",\"\"",
        "\".gitignore\",\"\"",
        // Dotless name → empty ext.
        "\"README\",\"\"",
        // Trailing-dot name → empty ext.
        "\"foo.\",\"\"",
        // Multi-dot directory name → segment after the last dot
        // (matches the sort key produced by `extract_extension_after_dot`).
        "\"amd64_microsoft-windows-mdmappinstaller_31bf3856ad364e35_10.0.26100.8115_none_3591783d4bfd6e96\",\"8115_none_3591783d4bfd6e96\"",
        // Normal file → ext after the last dot.
        "\"report.txt\",\"txt\"",
    ];
    for expected in expected_lines {
        assert!(
            csv.contains(expected),
            "Extension column output missing expected line {expected:?}; full CSV:\n{csv}"
        );
    }
}
