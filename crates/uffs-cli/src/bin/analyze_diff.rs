//! Deep comparison of UFFS C++ vs Rust outputs using Polars.
//!
//! This is a development/debugging tool for comparing C++ and Rust UFFS
//! outputs. It identifies structural differences, missing records, and
//! attribute mismatches.
//!
//! # Usage
//!
//! ```bash
//! analyze_diff <cpp.txt> <rust.txt>
//! ```
//!
//! # Output
//!
//! The tool prints analysis results to stdout, including:
//! - Column comparison between C++ and Rust outputs
//! - Path matching statistics
//! - Missing path analysis by drive and parent directory
//! - Pattern analysis for system files and unknown paths

// This is a CLI analysis/debugging tool where:
// - stdout/stderr output is intentional and required for user interaction
// - Debug formatting is needed to display Polars DataFrames and collections
// - Floating-point arithmetic is acceptable for percentage calculations
// - Shadow reuse is idiomatic when transforming DataFrames through a pipeline
// - String slicing on normalized paths (ASCII only) is safe
// - The main function is intentionally long as a sequential analysis script
// - Unused crate dependencies come from the parent Cargo.toml
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::use_debug,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::min_ident_chars,
    clippy::single_call_fn,
    clippy::shadow_reuse,
    clippy::string_slice,
    clippy::too_many_lines,
    clippy::indexing_slicing,
    clippy::default_numeric_fallback,
    unused_crate_dependencies
)]

use core::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use uffs_polars::{Column, CsvReadOptions, DataFrame, IntoSeries, SerReader, StringChunked};

/// Loads a CSV file into a Polars `DataFrame`.
///
/// # Arguments
///
/// * `path` - Path to the CSV file
/// * `name` - Display name for logging (e.g., "C++" or "Rust")
///
/// # Errors
///
/// Returns an error if the file cannot be read or parsed.
fn load_csv(path: &Path, name: &str) -> Result<DataFrame> {
    println!("Loading {name}: {}", path.display());
    let df = CsvReadOptions::default()
        .with_has_header(true)
        .with_infer_schema_length(Some(10000))
        .try_into_reader_with_file_path(Some(path.into()))?
        .finish()
        .with_context(|| format!("Failed to read CSV: {}", path.display()))?;
    println!("  Loaded {} rows, {} columns", df.height(), df.width());
    println!("  Columns: {:?}", df.get_column_names());
    Ok(df)
}

/// Normalizes paths in a `DataFrame` by converting to lowercase and replacing
/// backslashes.
///
/// Adds a new column `path_norm` with the normalized paths.
///
/// # Errors
///
/// Returns an error if the "Path" column doesn't exist or isn't a string type.
fn normalize_paths(input_df: &DataFrame) -> Result<DataFrame> {
    let path_col = input_df.column("Path")?.str()?;
    let normalized: StringChunked = path_col
        .into_iter()
        .map(|opt: Option<&str>| opt.map(|val| val.to_lowercase().replace('\\', "/")))
        .collect();

    let mut result_df = input_df.clone();
    let col = Column::new("path_norm".into(), normalized.into_series());
    result_df.with_column(col)?;
    Ok(result_df)
}

/// Extracts the parent directory from a normalized path.
///
/// Returns the path up to and including the last `/`.
fn extract_parent(path: &str) -> Option<&str> {
    path.rfind('/').map(|idx| &path[..=idx])
}

/// Extracts the drive letter from a normalized path.
///
/// Returns the first character if it's an ASCII alphabetic character.
fn extract_drive(path: &str) -> Option<char> {
    path.chars().next().filter(char::is_ascii_alphabetic)
}

/// Entry point for the UFFS diff analysis tool.
///
/// Compares C++ and Rust UFFS outputs to identify differences in:
/// - Column schemas
/// - Path matching
/// - Missing files by drive and parent directory
/// - System file patterns
///
/// # Errors
///
/// Returns an error if input files cannot be read or parsed.
fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: analyze_diff <cpp.txt> <rust.txt>");
        std::process::exit(1);
    }

    let cpp_path = Path::new(&args[1]);
    let rust_path = Path::new(&args[2]);

    println!("{}", "=".repeat(70));
    println!("UFFS Deep Comparison Analysis (Rust)");
    println!("{}", "=".repeat(70));

    // Load data
    let cpp = load_csv(cpp_path, "C++")?;
    let rust = load_csv(rust_path, "Rust")?;

    println!("\n{}", "=".repeat(70));
    println!("STEP 1: Column Comparison");
    println!("{}", "=".repeat(70));

    let cpp_cols: HashSet<_> = cpp.get_column_names().into_iter().collect();
    let rust_cols: HashSet<_> = rust.get_column_names().into_iter().collect();

    let only_cpp: Vec<_> = cpp_cols.difference(&rust_cols).collect();
    let only_rust: Vec<_> = rust_cols.difference(&cpp_cols).collect();
    let common: Vec<_> = cpp_cols.intersection(&rust_cols).collect();

    println!(
        "C++ columns ({}): {:?}",
        cpp_cols.len(),
        cpp.get_column_names()
    );
    println!(
        "Rust columns ({}): {:?}",
        rust_cols.len(),
        rust.get_column_names()
    );
    println!("Only in C++: {only_cpp:?}");
    println!("Only in Rust: {only_rust:?}");
    println!("Common columns ({}): {:?}", common.len(), common);

    println!("\n{}", "=".repeat(70));
    println!("STEP 2: Path Analysis");
    println!("{}", "=".repeat(70));

    // Normalize paths
    let cpp = normalize_paths(&cpp)?;
    let rust = normalize_paths(&rust)?;

    // Sample paths
    println!("\nSample C++ paths:");
    println!("{:?}", cpp.column("path_norm")?.head(Some(5)));
    println!("\nSample Rust paths:");
    println!("{:?}", rust.column("path_norm")?.head(Some(5)));

    println!("\n{}", "=".repeat(70));
    println!("STEP 3: Build Path Sets");
    println!("{}", "=".repeat(70));

    // Extract paths into sets
    let cpp_paths: HashSet<String> = cpp
        .column("path_norm")?
        .str()?
        .into_iter()
        .filter_map(|opt_str: Option<&str>| opt_str.map(String::from))
        .collect();

    let rust_paths: HashSet<String> = rust
        .column("path_norm")?
        .str()?
        .into_iter()
        .filter_map(|opt_str: Option<&str>| opt_str.map(String::from))
        .collect();

    println!("C++ unique paths: {:?}", cpp_paths.len());
    println!("Rust unique paths: {:?}", rust_paths.len());

    let common_paths: HashSet<_> = cpp_paths.intersection(&rust_paths).collect();
    let cpp_only: Vec<_> = cpp_paths.difference(&rust_paths).collect();
    let rust_only: Vec<_> = rust_paths.difference(&cpp_paths).collect();

    println!("Exact matches: {:?}", common_paths.len());
    println!("C++ only: {:?}", cpp_only.len());
    println!("Rust only: {:?}", rust_only.len());

    let match_rate = 100.0 * common_paths.len() as f64 / cpp_paths.len() as f64;
    println!("Match rate: {match_rate:.2}%");

    println!("\n{}", "=".repeat(70));
    println!("STEP 4: Analyze Missing Paths");
    println!("{}", "=".repeat(70));

    // Sample missing paths
    println!("\nSample paths in C++ but NOT in Rust (first 20):");
    for (idx, path) in cpp_only.iter().take(20).enumerate() {
        println!("  {}: {path}", idx + 1);
    }

    if !rust_only.is_empty() {
        println!("\nSample paths in Rust but NOT in C++ (first 20):");
        for (idx, path) in rust_only.iter().take(20).enumerate() {
            println!("  {}: {path}", idx + 1);
        }
    }

    // Analyze by drive
    println!("\nMissing paths by drive:");
    let mut drive_counts: HashMap<char, usize> = HashMap::new();
    for path in &cpp_only {
        if let Some(drive) = extract_drive(path) {
            *drive_counts.entry(drive).or_insert(0) += 1;
        }
    }
    let mut drives: Vec<_> = drive_counts.iter().collect();
    drives.sort_by_key(|(_, cnt)| Reverse(*cnt));
    for (drive, count) in drives {
        println!("  {drive}: {count}");
    }

    println!("\n{}", "=".repeat(70));
    println!("STEP 5: Parent Directory Analysis");
    println!("{}", "=".repeat(70));

    // Check which parent directories are missing
    let cpp_parents: HashSet<String> = cpp_paths
        .iter()
        .filter_map(|path| extract_parent(path).map(String::from))
        .collect();
    let rust_parents: HashSet<String> = rust_paths
        .iter()
        .filter_map(|path| extract_parent(path).map(String::from))
        .collect();

    let missing_parents: Vec<_> = cpp_parents.difference(&rust_parents).collect();
    println!(
        "Parent dirs in C++ but not Rust: {:?}",
        missing_parents.len()
    );

    // Top missing parent directories by frequency
    let mut parent_freq: HashMap<&str, usize> = HashMap::new();
    for path in &cpp_only {
        if let Some(parent) = extract_parent(path) {
            *parent_freq.entry(parent).or_insert(0) += 1;
        }
    }
    let mut parents: Vec<_> = parent_freq.iter().collect();
    parents.sort_by_key(|(_, cnt)| Reverse(*cnt));

    println!("\nTop 20 parent directories with most missing files:");
    for (parent, count) in parents.iter().take(20) {
        println!("  {count:>6} files missing in: {parent}");
    }

    println!("\n{}", "=".repeat(70));
    println!("STEP 6: Pattern Analysis in Missing Paths");
    println!("{}", "=".repeat(70));

    // Check for $-prefixed (system) files
    let system_files_count = cpp_only
        .iter()
        .filter(|path| {
            path.split('/')
                .next_back()
                .is_some_and(|name| name.starts_with('$'))
        })
        .count();
    println!("Missing $-prefixed (system) files: {system_files_count}");

    // Check for <unknown> in Rust paths
    let unknown_paths: Vec<_> = rust_paths
        .iter()
        .filter(|path| path.contains("<unknown>"))
        .collect();
    println!("Rust paths with '<unknown>': {}", unknown_paths.len());
    if !unknown_paths.is_empty() {
        println!("  Sample:");
        for path in unknown_paths.iter().take(10) {
            println!("    {path}");
        }
    }

    // Check for very short paths (root-level issues)
    let cpp_short_count = cpp_paths.iter().filter(|path| path.len() < 10).count();
    let rust_short_count = rust_paths.iter().filter(|path| path.len() < 10).count();
    println!("\nC++ paths < 10 chars: {cpp_short_count}");
    println!("Rust paths < 10 chars: {rust_short_count}");

    // Check for paths with "." as filename (directory entries)
    let cpp_dot_count = cpp_paths.iter().filter(|path| path.ends_with("/.")).count();
    let rust_dot_count = rust_paths
        .iter()
        .filter(|path| path.ends_with("/."))
        .count();
    println!("\nC++ paths ending with '/.' (dir entries): {cpp_dot_count}");
    println!("Rust paths ending with '/.' (dir entries): {rust_dot_count}");

    println!("\n{}", "=".repeat(70));
    println!("SUMMARY & ROOT CAUSE HYPOTHESIS");
    println!("{}", "=".repeat(70));

    let missing_pct = 100.0 * cpp_only.len() as f64 / cpp_paths.len() as f64;

    println!("\nAnalysis Complete:");
    println!("  - C++ found {} unique paths", cpp_paths.len());
    println!("  - Rust found {} unique paths", rust_paths.len());
    println!(
        "  - Missing from Rust: {} ({missing_pct:.1}%)",
        cpp_only.len(),
    );
    println!("  - Extra in Rust: {}", rust_only.len());
    println!("\nLikely Issues:");
    println!(
        "  1. <unknown> paths: {} paths have unresolved parents",
        unknown_paths.len()
    );
    println!(
        "  2. Missing parent dirs: {} parent directories not in Rust",
        missing_parents.len()
    );
    println!("  3. System files ($): {system_files_count} $-prefixed files missing");
    println!("  4. Directory entries (.): C++ has {cpp_dot_count}, Rust has {rust_dot_count}");

    Ok(())
}
