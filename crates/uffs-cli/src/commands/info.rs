//! Info command implementation.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uffs_mft::MftReader;

use super::{format_number, format_size};

/// Show information about an index file.
///
/// # Errors
///
/// Returns an error if:
/// - The index file cannot be loaded
/// - Writing to stdout fails
#[expect(
    clippy::single_call_fn,
    reason = "public CLI command handler called from main dispatch"
)]
pub fn info(path: &Path) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load index: {}", path.display()))?;

    let stats = extract_index_stats(&df, path);
    print_index_info(&stats, &df)?;
    Ok(())
}

/// Statistics extracted from an index file.
struct IndexStats {
    /// Absolute path to the index file.
    abs_path: PathBuf,
    /// Size of the index file on disk in bytes.
    file_size: u64,
    /// Total number of records in the index.
    total_records: usize,
    /// Number of directory entries.
    dir_count: u64,
    /// Number of file entries.
    file_count: u64,
    /// Number of hidden files/directories.
    hidden_count: u64,
    /// Number of system files/directories.
    system_count: u64,
    /// Number of compressed files.
    compressed_count: u64,
    /// Number of encrypted files.
    encrypted_count: u64,
    /// Number of sparse files.
    sparse_count: u64,
    /// Number of reparse points.
    reparse_count: u64,
    /// Number of read-only files.
    readonly_count: u64,
    /// Number of archive files.
    archive_count: u64,
    /// Total logical size of all files in bytes.
    total_size: u64,
    /// Total allocated size on disk in bytes.
    total_allocated: u64,
    /// Number of files with multiple data streams.
    multi_stream_count: u64,
    /// Number of files with multiple names (hard links).
    multi_name_count: u64,
}

/// Count true values in a boolean column.
fn count_bool_column(df: &uffs_mft::DataFrame, name: &str) -> u64 {
    if let Ok(column) = df.column(name) {
        if let Ok(bool_arr) = column.bool() {
            return u64::from(bool_arr.sum().unwrap_or(0));
        }
    }
    0
}

/// Sum values in a u64 column.
fn sum_u64_column(df: &uffs_mft::DataFrame, name: &str) -> u64 {
    if let Ok(column) = df.column(name) {
        if let Ok(u64_arr) = column.u64() {
            return u64_arr.iter().flatten().sum();
        }
    }
    0
}

/// Count entries where u16 column value > 1.
fn count_multi_value_u16(df: &uffs_mft::DataFrame, name: &str) -> u64 {
    if let Ok(column) = df.column(name) {
        if let Ok(u16_arr) = column.u16() {
            return u16_arr
                .iter()
                .filter(|val| val.is_some_and(|num| num > 1))
                .count() as u64;
        }
    }
    0
}

/// Extract statistics from a `DataFrame` index file.
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity and testability"
)]
fn extract_index_stats(df: &uffs_mft::DataFrame, path: &Path) -> IndexStats {
    let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let file_size = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let total_records = df.height();

    let dir_count = count_bool_column(df, "is_directory");
    let file_count = (total_records as u64).saturating_sub(dir_count);

    IndexStats {
        abs_path,
        file_size,
        total_records,
        dir_count,
        file_count,
        hidden_count: count_bool_column(df, "is_hidden"),
        system_count: count_bool_column(df, "is_system"),
        compressed_count: count_bool_column(df, "is_compressed"),
        encrypted_count: count_bool_column(df, "is_encrypted"),
        sparse_count: count_bool_column(df, "is_sparse"),
        reparse_count: count_bool_column(df, "is_reparse"),
        readonly_count: count_bool_column(df, "is_readonly"),
        archive_count: count_bool_column(df, "is_archive"),
        total_size: sum_u64_column(df, "size"),
        total_allocated: sum_u64_column(df, "allocated_size"),
        multi_stream_count: count_multi_value_u16(df, "stream_count"),
        multi_name_count: count_multi_value_u16(df, "name_count"),
    }
}

/// Print index information to stdout.
#[expect(clippy::single_call_fn, reason = "extracted for clarity")]
fn print_index_info(stats: &IndexStats, df: &uffs_mft::DataFrame) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let sep = "═══════════════════════════════════════════════════════════════";
    writeln!(out, "{sep}")?;
    writeln!(out, "                       INDEX FILE INFO")?;
    writeln!(out, "{sep}\n")?;
    writeln!(out, "📁 FILE DETAILS")?;
    writeln!(out, "  Path:                 {}", stats.abs_path.display())?;
    writeln!(
        out,
        "  File size:            {}",
        format_size(stats.file_size)
    )?;
    writeln!(out, "  Columns:              {}\n", df.width())?;
    writeln!(out, "📊 RECORD STATISTICS")?;
    writeln!(
        out,
        "  Total records:        {}",
        format_number(stats.total_records as u64)
    )?;
    writeln!(
        out,
        "  Directories:          {}",
        format_number(stats.dir_count)
    )?;
    writeln!(
        out,
        "  Files:                {}\n",
        format_number(stats.file_count)
    )?;
    writeln!(out, "💾 SIZE METRICS")?;
    writeln!(
        out,
        "  Total file size:      {}",
        format_size(stats.total_size)
    )?;
    writeln!(
        out,
        "  Total allocated:      {}\n",
        format_size(stats.total_allocated)
    )?;
    writeln!(out, "🏷️  ATTRIBUTES")?;
    writeln!(
        out,
        "  Hidden:               {}",
        format_number(stats.hidden_count)
    )?;
    writeln!(
        out,
        "  System:               {}",
        format_number(stats.system_count)
    )?;
    writeln!(
        out,
        "  Read-only:            {}",
        format_number(stats.readonly_count)
    )?;
    writeln!(
        out,
        "  Archive:              {}",
        format_number(stats.archive_count)
    )?;
    writeln!(
        out,
        "  Compressed:           {}",
        format_number(stats.compressed_count)
    )?;
    writeln!(
        out,
        "  Encrypted:            {}",
        format_number(stats.encrypted_count)
    )?;
    writeln!(
        out,
        "  Sparse:               {}",
        format_number(stats.sparse_count)
    )?;
    writeln!(
        out,
        "  Reparse points:       {}\n",
        format_number(stats.reparse_count)
    )?;
    writeln!(out, "🔗 ADVANCED")?;
    writeln!(
        out,
        "  Multi-stream files:   {}",
        format_number(stats.multi_stream_count)
    )?;
    writeln!(
        out,
        "  Multi-name files:     {}\n",
        format_number(stats.multi_name_count)
    )?;
    writeln!(out, "📋 SCHEMA")?;
    for (name, dtype) in df.schema().iter() {
        writeln!(out, "  {name}: {dtype}")?;
    }
    Ok(())
}
