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
    CppFooterContext, export_json, format_json_value, results_to_dataframe, write_cpp_drive_footer,
    write_native_results_to, write_results,
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

    let docs = index.get_or_create(7);
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

    let file = index.get_or_create(42);
    file.stdinfo.created = 1_700_001_000_000_000;
    file.stdinfo.modified = 1_700_001_100_000_000;
    file.stdinfo.accessed = 1_700_001_200_000_000;
    file.stdinfo.flags |= StandardInfo::IS_ARCHIVE | StandardInfo::IS_READONLY;
    file.first_name.name = IndexNameRef::new(file_name, 8, true, file_ext);
    file.first_name.parent_frs = 7;
    file.first_stream.size.length = 123;
    file.first_stream.size.allocated = 128;
    file.descendants = 7;
    file.treesize = 4096;
    file.tree_allocated = 8192;

    index
}

fn sample_search_results() -> Vec<SearchResult> {
    vec![
        SearchResult {
            name: "Docs".to_owned(),
            path: Some("C:\\Docs\\".to_owned()),
            size: 10,
            allocated_size: 20,
            frs: 7,
            parent_frs: ROOT_FRS,
            is_directory: true,
            stream_name: String::new(),
            name_index: 0,
            stream_index: 0,
            descendants: 3,
            treesize: 1000,
            tree_allocated: 2000,
        },
        SearchResult {
            name: "file.txt".to_owned(),
            path: Some("C:\\Docs\\file.txt".to_owned()),
            size: 123,
            allocated_size: 128,
            frs: 42,
            parent_frs: 7,
            is_directory: false,
            stream_name: String::new(),
            name_index: 0,
            stream_index: 0,
            descendants: 7,
            treesize: 4096,
            tree_allocated: 8192,
        },
    ]
}

fn write_results_to_string(
    results: &DataFrame,
    format: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    _elapsed: Duration,
    pattern: &str,
) -> Result<String> {
    let mut output = Vec::new();
    let footer_ctx = CppFooterContext {
        output_targets,
        pattern,
        row_count: results.height(),
    };
    match format {
        "json" => export_json(results, &mut output)?,
        "custom" => {
            output_config.write(results, &mut output)?;
            write_cpp_drive_footer(&mut output, &footer_ctx)?;
        }
        _ => output_config.write(results, &mut output)?,
    }
    String::from_utf8(output).map_err(Into::into)
}

fn write_native_results_to_string(
    index: &MftIndex,
    results: &[SearchResult],
    format: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    _elapsed: Duration,
    pattern: &str,
) -> Result<String> {
    let mut output = Vec::new();
    let footer_ctx = CppFooterContext {
        output_targets,
        pattern,
        row_count: results.len(),
    };
    write_native_results_to(
        index,
        results,
        format,
        &mut output,
        output_config,
        &footer_ctx,
    )?;
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

    // With 1 row (< 20,000), should include fast-scan message
    assert_eq!(
        written,
        concat!(
            "\"C:\\Temp\\file.txt\",\"file.txt\"\n",
            "\r\n",
            "\r\n",
            "Drives? \t2\tC:|D:\r\n",
            "\r\n",
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t*.txt\r\n",
            "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n"
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
fn test_write_native_results_matches_dataframe_custom_output() -> TestResult {
    let index = sample_index();
    let results = sample_search_results();
    let output_config = OutputConfig::new().with_columns(
        "path,name,pathonly,size,sizeondisk,created,readonly,archive,hidden,directory,descendants,treesize,treeallocated,type",
    );
    let elapsed = Duration::from_secs(2);
    let pattern = "*.txt";

    let expected = write_results_to_string(
        &results_to_dataframe(&index, results.clone(), true)?,
        "custom",
        &output_config,
        &['C'],
        elapsed,
        pattern,
    )?;
    let actual = write_native_results_to_string(
        &index,
        &results,
        "custom",
        &output_config,
        &['C'],
        elapsed,
        pattern,
    )?;

    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn test_write_native_results_matches_dataframe_csv_output() -> TestResult {
    let index = sample_index();
    let results = sample_search_results();
    let output_config = OutputConfig::new()
        .with_columns("path,name,size,sizeondisk,pathonly")
        .with_separator(";")
        .with_quote("'")
        .with_header(false);
    let elapsed = Duration::from_secs(2);
    let pattern = "*.txt";

    let expected = write_results_to_string(
        &results_to_dataframe(&index, results.clone(), true)?,
        "csv",
        &output_config,
        &['C'],
        elapsed,
        pattern,
    )?;
    let actual = write_native_results_to_string(
        &index,
        &results,
        "csv",
        &output_config,
        &['C'],
        elapsed,
        pattern,
    )?;

    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn test_cpp_footer_includes_fast_scan_message_when_elapsed_le_1s() -> TestResult {
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
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t>G:.*\r\n",
            "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n"
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
