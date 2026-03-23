//! Tests for output helpers.

use core::time::Duration;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use uffs_core::SearchResult;
use uffs_core::output::OutputConfig;
use uffs_mft::index::{IndexNameRef, MftIndex, ROOT_FRS, StandardInfo};
use uffs_polars::{Column, DataFrame};

use super::{
    CppFooterContext, StreamingRecordFilter, format_json_value, results_to_dataframe,
    write_index_streaming, write_results,
};

type TestResult = Result<()>;

fn temp_output_path(extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u128, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "uffs-cli-output-contract-{}-{nanos}.{extension}",
        std::process::id()
    ))
}

fn sample_df() -> Result<DataFrame> {
    DataFrame::new_infer_height(vec![
        Column::new("path".into(), &["C:\\Temp\\file.txt"]),
        Column::new("name".into(), &["file.txt"]),
    ])
    .map_err(Into::into)
}

/// Create a large sample `DataFrame` (20,000+ rows) for testing slow-scan
/// footer.
fn large_sample_df() -> Result<DataFrame> {
    let count: i32 = 20_000;
    let paths: Vec<String> = (0_i32..count)
        .map(|idx| format!("C:\\Temp\\file{idx}.txt"))
        .collect();
    let names: Vec<String> = (0_i32..count).map(|idx| format!("file{idx}.txt")).collect();

    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();

    DataFrame::new_infer_height(vec![
        Column::new("path".into(), &path_refs),
        Column::new("name".into(), &name_refs),
    ])
    .map_err(Into::into)
}

/// FRS for the Docs directory in `sample_index`.
const DOCS_FRS: u64 = 7;
/// FRS for file.txt in `sample_index`.
const FILE_FRS: u64 = 42;

fn sample_index() -> MftIndex {
    let mut index = MftIndex::new('C');
    let root_name = index.add_name(".");
    let file_name = index.add_name("file.txt");
    let dir_name = index.add_name("Docs");
    let file_ext = index.intern_extension("file.txt");

    let root = index.get_or_create(ROOT_FRS);
    root.stdinfo.set_directory(true);
    root.first_name.name = IndexNameRef::new(root_name, 1, true, IndexNameRef::NO_EXTENSION);
    root.first_name.parent_frs = ROOT_FRS;

    let docs = index.get_or_create(DOCS_FRS);
    docs.stdinfo.set_directory(true);
    docs.stdinfo.created = 1_700_000_000_000_000;
    docs.stdinfo.modified = 1_700_000_100_000_000;
    docs.stdinfo.accessed = 1_700_000_200_000_000;
    docs.stdinfo.flags |= StandardInfo::IS_HIDDEN;
    docs.first_name.name = IndexNameRef::new(dir_name, 4, true, IndexNameRef::NO_EXTENSION);
    docs.first_name.parent_frs = ROOT_FRS;
    docs.first_stream.size.length = 10;
    docs.first_stream.size.allocated = 20;
    docs.descendants = 3;
    docs.treesize = 1000;
    docs.tree_allocated = 2000;

    let file = index.get_or_create(FILE_FRS);
    file.stdinfo.created = 1_700_001_000_000_000;
    file.stdinfo.modified = 1_700_001_100_000_000;
    file.stdinfo.accessed = 1_700_001_200_000_000;
    file.stdinfo.flags |= StandardInfo::IS_ARCHIVE | StandardInfo::IS_READONLY;
    file.first_name.name = IndexNameRef::new(file_name, 8, true, file_ext);
    file.first_name.parent_frs = DOCS_FRS;
    file.first_stream.size.length = 123;
    file.first_stream.size.allocated = 128;
    file.descendants = 7;
    file.treesize = 4096;
    file.tree_allocated = 8192;

    index
}

fn write_streaming_to_string(
    index: &MftIndex,
    format: &str,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<String> {
    let mut output = Vec::new();
    write_index_streaming(index, &mut output, format, output_config, footer_ctx)?;
    String::from_utf8(output).map_err(Into::into)
}

#[test]
fn test_format_json_value_escapes_windows_paths_and_control_chars() {
    let column = Column::new("path".into(), &["C:\\Temp\\tab\t\"quote\"\n\r\u{0001}"]);

    assert_eq!(
        format_json_value(&column, 0),
        "\"C:\\\\Temp\\\\tab\\t\\\"quote\\\"\\n\\r\\u0001\""
    );
}

#[test]
fn test_write_results_csv_uses_output_config_without_cpp_footer() -> Result<()> {
    let path = temp_output_path("csv");
    let results = sample_df()?;
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_separator(";")
        .with_quote("'")
        .with_header(false);

    write_results(
        &results,
        "csv",
        &path.to_string_lossy(),
        &output_config,
        &['C', 'D'],
        Duration::from_secs(2),
        "*.txt",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    assert_eq!(written, "'C:\\Temp\\file.txt';'file.txt'\n");
    Ok(())
}

#[test]
fn test_write_results_custom_file_appends_cpp_drive_footer() -> Result<()> {
    let path = temp_output_path("txt");
    let results = sample_df()?;
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_header(false);

    write_results(
        &results,
        "custom",
        &path.to_string_lossy(),
        &output_config,
        &['C', 'D'],
        Duration::from_secs(2),
        "*.txt",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    // With glob pattern "*.txt", few results is expected — no MMMmmm warning.
    // The warning only triggers for full-scan patterns (*, **, **/*).
    assert_eq!(
        written,
        concat!(
            "\"C:\\Temp\\file.txt\",\"file.txt\"\n",
            "\r\n",
            "\r\n",
            "Drives? \t2\tC:|D:\r\n",
            "\r\n",
        )
    );
    Ok(())
}

#[test]
fn test_write_results_json_file_has_no_footer() -> Result<()> {
    let path = temp_output_path("json");
    let results = sample_df()?;
    let output_config = OutputConfig::new().with_columns("path,name");

    write_results(
        &results,
        "json",
        &path.to_string_lossy(),
        &output_config,
        &['C', 'D'],
        Duration::from_secs(2),
        "*.txt",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    assert!(!written.contains("Drives?"));
    assert!(written.contains(r#""C:\\Temp\\file.txt""#));
    Ok(())
}

#[test]
fn test_results_to_dataframe_preserves_search_result_fields() -> TestResult {
    let index = sample_index();
    let df = results_to_dataframe(
        &index,
        vec![SearchResult {
            name: "file.txt".to_owned(),
            path: Some("C:\\Docs\\file.txt".to_owned()),
            size: 123,
            allocated_size: 128,
            frs: 42,
            parent_frs: ROOT_FRS,
            is_directory: false,
            stream_name: String::new(),
            name_index: 0,
            stream_index: 0,
            descendants: 7,
            treesize: 4096,
            tree_allocated: 8192,
        }],
        true,
    )?;

    assert_eq!(df.column("name")?.str()?.get(0), Some("file.txt"));
    assert_eq!(df.column("type")?.str()?.get(0), Some("txt"));
    assert_eq!(df.column("path")?.str()?.get(0), Some("C:\\Docs\\file.txt"));
    assert_eq!(df.column("path_only")?.str()?.get(0), Some("C:\\Docs\\"));
    assert_eq!(df.column("stream_name")?.str()?.get(0), Some(""));
    assert_eq!(df.column("size")?.u64()?.get(0), Some(123));
    assert_eq!(df.column("allocated_size")?.u64()?.get(0), Some(128));
    assert_eq!(df.column("descendants")?.u32()?.get(0), Some(7));
    assert_eq!(df.column("treesize")?.u64()?.get(0), Some(4096));
    assert_eq!(df.column("tree_allocated")?.u64()?.get(0), Some(8192));
    Ok(())
}

#[test]
fn test_can_write_native_results_rejects_bulkiness_columns() {
    use super::can_write_native_results;

    let bulkiness = OutputConfig::new().with_columns("path,bulkiness");
    let descendants = OutputConfig::new().with_columns("path,descendants,treesize");

    assert!(!can_write_native_results("custom", &bulkiness));
    assert!(can_write_native_results("csv", &descendants));
    assert!(!can_write_native_results("json", &descendants));
}

#[test]
fn test_streaming_output_produces_valid_csv() -> TestResult {
    let index = sample_index();
    let output_config = OutputConfig::new()
        .with_columns("path,name,size")
        .with_header(true);
    let footer_ctx = CppFooterContext {
        output_targets: &['C'],
        pattern: "*.txt",
        row_count: 0,
    };

    let output = write_streaming_to_string(&index, "csv", &output_config, &footer_ctx)?;

    // Should contain header row with column names
    assert!(output.contains("\"Path\""), "missing Path header");
    assert!(output.contains("\"Name\""), "missing Name header");
    assert!(output.contains("\"Size\""), "missing Size header");
    // Should contain at least one data row (root, Docs, or file.txt)
    assert!(
        output
            .lines()
            .any(|line| !line.is_empty() && (!line.starts_with('"') || line.contains(','))),
        "no data rows in streaming output"
    );
    Ok(())
}

#[test]
fn test_streaming_output_custom_format_includes_footer() -> TestResult {
    let index = sample_index();
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_header(false);
    let footer_ctx = CppFooterContext {
        output_targets: &['C'],
        pattern: "*.txt",
        row_count: 0,
    };

    let output = write_streaming_to_string(&index, "custom", &output_config, &footer_ctx)?;

    // Custom format should include the C++ footer with drive info
    assert!(output.contains("Drives?"), "missing footer Drives? line");
    assert!(output.contains("C:"), "missing drive letter in footer");
    Ok(())
}

#[test]
fn test_cpp_footer_includes_fast_scan_message_for_full_scan_pattern() -> TestResult {
    let path = temp_output_path("txt");
    let results = sample_df()?;
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_header(false);

    // Full-scan pattern "*" with few results → should trigger the warning
    write_results(
        &results,
        "custom",
        &path.to_string_lossy(),
        &output_config,
        &['G'],
        Duration::from_millis(999),
        "*",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    assert_eq!(
        written,
        concat!(
            "\"C:\\Temp\\file.txt\",\"file.txt\"\n",
            "\r\n",
            "\r\n",
            "Drives? \t1\tG:\r\n",
            "\r\n",
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t*\r\n",
            "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n"
        )
    );
    Ok(())
}

#[test]
fn test_cpp_footer_omits_fast_scan_message_for_regex_pattern() -> TestResult {
    let path = temp_output_path("txt");
    let results = sample_df()?;
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_header(false);

    // Regex pattern with few results → should NOT trigger the warning
    // (few results is expected for filtered queries)
    write_results(
        &results,
        "custom",
        &path.to_string_lossy(),
        &output_config,
        &['G'],
        Duration::from_millis(999),
        ">G:.*",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    assert_eq!(
        written,
        concat!(
            "\"C:\\Temp\\file.txt\",\"file.txt\"\n",
            "\r\n",
            "\r\n",
            "Drives? \t1\tG:\r\n",
            "\r\n",
        )
    );
    Ok(())
}

#[test]
fn test_cpp_footer_omits_fast_scan_message_when_elapsed_gt_1s() -> TestResult {
    let path = temp_output_path("txt");
    let results = large_sample_df()?; // Use 20,000 rows to trigger slow scan
    let output_config = OutputConfig::new()
        .with_columns("path,name")
        .with_header(false);

    write_results(
        &results,
        "custom",
        &path.to_string_lossy(),
        &output_config,
        &['G'],
        Duration::from_secs(2),
        ">G:.*",
    )?;

    let written = fs::read_to_string(&path)?;
    drop(fs::remove_file(&path));

    // Should NOT contain fast-scan message (row_count >= 20,000)
    // Only check footer structure - first row will be
    // "C:\Temp\file0.txt","file0.txt"
    let lines: Vec<&str> = written.lines().collect();
    let footer_start = lines.len().saturating_sub(4);
    assert_eq!(lines.get(footer_start), Some(&""));
    assert_eq!(lines.get(footer_start + 1), Some(&""));
    assert_eq!(lines.get(footer_start + 2), Some(&"Drives? \t1\tG:"));
    assert_eq!(lines.get(footer_start + 3), Some(&""));
    Ok(())
}

// =========================================================================
// StreamingRecordFilter Tests (F1-F13 from branch matrix)
// =========================================================================

fn make_file_record(is_dir: bool, size: u64, flags: u32) -> uffs_mft::index::FileRecord {
    let mut rec = uffs_mft::index::FileRecord::default();
    rec.first_stream.size.length = size;
    rec.stdinfo.flags = flags;
    if is_dir {
        rec.stdinfo.set_directory(true);
    }
    rec
}

#[test]
fn test_filter_files_only_skips_dirs() {
    let filter = StreamingRecordFilter {
        files_only: true,
        ..StreamingRecordFilter::default()
    };
    let dir_rec = make_file_record(true, 0, 0);
    assert!(
        !filter.matches(&dir_rec),
        "files_only should skip directories"
    );
}

#[test]
fn test_filter_files_only_passes_files() {
    let filter = StreamingRecordFilter {
        files_only: true,
        ..StreamingRecordFilter::default()
    };
    let file_rec = make_file_record(false, 100, 0);
    assert!(filter.matches(&file_rec), "files_only should pass files");
}

#[test]
fn test_filter_dirs_only_skips_files() {
    let filter = StreamingRecordFilter {
        dirs_only: true,
        ..StreamingRecordFilter::default()
    };
    let file_rec = make_file_record(false, 100, 0);
    assert!(!filter.matches(&file_rec), "dirs_only should skip files");
}

#[test]
fn test_filter_dirs_only_passes_dirs() {
    let filter = StreamingRecordFilter {
        dirs_only: true,
        ..StreamingRecordFilter::default()
    };
    let dir_rec = make_file_record(true, 0, 0);
    assert!(
        filter.matches(&dir_rec),
        "dirs_only should pass directories"
    );
}

#[test]
fn test_filter_hide_system() {
    let filter = StreamingRecordFilter {
        hide_system: true,
        ..StreamingRecordFilter::default()
    };
    let system_rec = make_file_record(false, 100, StandardInfo::IS_SYSTEM);
    assert!(
        !filter.matches(&system_rec),
        "hide_system should skip system files"
    );
}

#[test]
fn test_filter_hide_hidden() {
    let filter = StreamingRecordFilter {
        hide_system: true,
        ..StreamingRecordFilter::default()
    };
    let hidden_rec = make_file_record(false, 100, StandardInfo::IS_HIDDEN);
    assert!(
        !filter.matches(&hidden_rec),
        "hide_system should skip hidden files"
    );
}

#[test]
fn test_filter_min_size() {
    let filter = StreamingRecordFilter {
        min_size: Some(1024),
        ..StreamingRecordFilter::default()
    };
    let small_rec = make_file_record(false, 512, 0);
    let big_rec = make_file_record(false, 2048, 0);
    assert!(
        !filter.matches(&small_rec),
        "min_size should skip small files"
    );
    assert!(filter.matches(&big_rec), "min_size should pass large files");
}

#[test]
fn test_filter_max_size() {
    let filter = StreamingRecordFilter {
        max_size: Some(1024),
        ..StreamingRecordFilter::default()
    };
    let small_rec = make_file_record(false, 512, 0);
    let big_rec = make_file_record(false, 2048, 0);
    assert!(
        filter.matches(&small_rec),
        "max_size should pass small files"
    );
    assert!(
        !filter.matches(&big_rec),
        "max_size should skip large files"
    );
}

#[test]
fn test_filter_default_passes_everything() {
    let filter = StreamingRecordFilter::default();
    let file_rec = make_file_record(false, 100, 0);
    let dir_rec = make_file_record(true, 0, 0);
    let system_rec = make_file_record(false, 100, StandardInfo::IS_SYSTEM);
    assert!(
        filter.matches(&file_rec),
        "default filter should pass files"
    );
    assert!(filter.matches(&dir_rec), "default filter should pass dirs");
    assert!(
        filter.matches(&system_rec),
        "default filter should pass system files"
    );
}

#[test]
fn test_filter_combined_files_only_and_min_size() {
    let filter = StreamingRecordFilter {
        files_only: true,
        min_size: Some(100),
        ..StreamingRecordFilter::default()
    };
    let small_file = make_file_record(false, 50, 0);
    let big_file = make_file_record(false, 200, 0);
    let dir_rec = make_file_record(true, 0, 0);
    assert!(
        !filter.matches(&small_file),
        "combined: small file should fail min_size"
    );
    assert!(
        filter.matches(&big_file),
        "combined: large file should pass"
    );
    assert!(
        !filter.matches(&dir_rec),
        "combined: directory should fail files_only"
    );
}

#[test]
fn test_filter_date_newer_modified() {
    let now_us = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(i64::MAX);
    let filter = StreamingRecordFilter {
        newer_modified: Some(now_us - 86_400 * 1_000_000), // 1 day ago
        ..StreamingRecordFilter::default()
    };
    let recent = {
        let mut rec = make_file_record(false, 100, 0);
        rec.stdinfo.modified = now_us - 3600 * 1_000_000; // 1 hour ago
        rec
    };
    let old = {
        let mut rec = make_file_record(false, 100, 0);
        rec.stdinfo.modified = now_us - 7 * 86_400 * 1_000_000; // 7 days ago
        rec
    };
    assert!(
        filter.matches(&recent),
        "newer_modified should pass recent files"
    );
    assert!(
        !filter.matches(&old),
        "newer_modified should skip old files"
    );
}

// =========================================================================
// Parse helpers tests
// =========================================================================

#[test]
fn test_parse_attr_filter_single_include() {
    let reqs = super::parse_attr_filter("hidden");
    assert_eq!(reqs.len(), 1);
}

#[test]
fn test_parse_attr_filter_single_exclude() {
    let reqs = super::parse_attr_filter("!hidden");
    assert_eq!(reqs.len(), 1);
}

#[test]
fn test_parse_attr_filter_multiple() {
    let reqs = super::parse_attr_filter("hidden,compressed,!system");
    assert_eq!(reqs.len(), 3);
}

#[test]
fn test_parse_attr_filter_empty() {
    let reqs = super::parse_attr_filter("");
    assert!(reqs.is_empty());
}

#[test]
fn test_parse_attr_filter_unknown_ignored() {
    let reqs = super::parse_attr_filter("hidden,bogus,system");
    assert_eq!(reqs.len(), 2, "unknown attrs should be silently ignored");
}

#[test]
fn test_parse_sort_spec_single() {
    let cols = super::parse_sort_spec("size");
    assert_eq!(cols.len(), 1);
    assert!(
        cols.first().expect("empty").descending,
        "size should default to descending"
    );
}

#[test]
fn test_parse_sort_spec_with_direction() {
    let cols = super::parse_sort_spec("name:asc");
    assert_eq!(cols.len(), 1);
    assert!(
        !cols.first().expect("empty").descending,
        "explicit :asc should be ascending"
    );
}

#[test]
fn test_parse_sort_spec_multi_tier() {
    let cols = super::parse_sort_spec("modified:desc,size:asc,name");
    assert_eq!(cols.len(), 3);
    assert!(
        cols.first().expect("missing 0").descending,
        "modified should be desc"
    );
    assert!(
        !cols.get(1).expect("missing 1").descending,
        "size:asc should be ascending"
    );
    assert!(
        !cols.get(2).expect("missing 2").descending,
        "name should default to ascending"
    );
}

#[test]
fn test_parse_sort_spec_smart_defaults() {
    let cols = super::parse_sort_spec("size,name,modified,created");
    assert!(
        cols.first().expect("missing 0").descending,
        "size defaults to desc"
    );
    assert!(
        !cols.get(1).expect("missing 1").descending,
        "name defaults to asc"
    );
    assert!(
        cols.get(2).expect("missing 2").descending,
        "modified defaults to desc"
    );
    assert!(
        cols.get(3).expect("missing 3").descending,
        "created defaults to desc"
    );
}

#[test]
fn test_parse_age_filter_days() {
    let ts = super::parse_age_filter("7d");
    assert!(ts.is_some(), "7d should parse");
    let now_us = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(i64::MAX);
    let diff = now_us - ts.unwrap();
    // Should be approximately 7 days in microseconds (± 1 second tolerance)
    let seven_days_us = 7 * 86_400 * 1_000_000_i64;
    assert!(
        (diff - seven_days_us).abs() < 2_000_000,
        "7d should be ~7 days ago"
    );
}

#[test]
fn test_parse_age_filter_hours() {
    let ts = super::parse_age_filter("24h");
    assert!(ts.is_some(), "24h should parse");
}

#[test]
fn test_parse_age_filter_minutes() {
    let ts = super::parse_age_filter("30m");
    assert!(ts.is_some(), "30m should parse");
}

#[test]
fn test_parse_age_filter_invalid() {
    assert!(
        super::parse_age_filter("abc").is_none(),
        "invalid duration should return None"
    );
    assert!(
        super::parse_age_filter("").is_none(),
        "empty should return None"
    );
}

#[test]
fn test_parse_age_filter_iso_date() {
    let ts = super::parse_age_filter("2026-01-15");
    assert!(ts.is_some(), "ISO date should parse");
}

#[test]
fn test_parse_age_filter_iso_datetime() {
    let ts = super::parse_age_filter("2026-01-15T10:30:00");
    assert!(ts.is_some(), "ISO datetime should parse");
}
