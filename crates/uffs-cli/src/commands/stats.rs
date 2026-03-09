//! Stats command implementation.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use uffs_core::MftQuery;
use uffs_mft::MftReader;

use super::format_size;

/// Show statistics about files in an index.
///
/// # Errors
///
/// Returns an error if:
/// - The index file cannot be loaded
/// - Query execution fails
/// - Writing to stdout fails
#[expect(
    clippy::single_call_fn,
    reason = "public CLI command handler called from main dispatch"
)]
pub fn stats(path: &Path, top: u32) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load index: {}", path.display()))?;

    let total_records = df.height();
    let files = MftQuery::new(df.clone()).files_only().collect()?;
    let dirs = MftQuery::new(df.clone()).directories_only().collect()?;

    let file_count = files.height();
    let dir_count = dirs.height();
    let file_size_col = files.column("size")?.u64()?;
    let total_size: u64 = file_size_col.into_iter().flatten().sum();

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "=== Index Statistics ===")?;
    writeln!(stdout)?;
    writeln!(stdout, "Total records: {total_records}")?;
    writeln!(stdout, "Files:         {file_count}")?;
    writeln!(stdout, "Directories:   {dir_count}")?;
    writeln!(stdout, "Total size:    {}", format_size(total_size))?;
    writeln!(stdout)?;

    writeln!(stdout, "=== Top {top} Largest Files ===")?;
    writeln!(stdout)?;

    let largest = MftQuery::new(df)
        .files_only()
        .sort_by_size(true)
        .limit(top)
        .collect()?;

    let name_col = largest.column("name")?.str()?;
    let largest_size_col = largest.column("size")?.u64()?;

    for idx in 0..largest.height() {
        let name = name_col.get(idx).unwrap_or("<unknown>");
        let size = largest_size_col.get(idx).unwrap_or(0);
        writeln!(stdout, "  {:>12}  {}", format_size(size), name)?;
    }

    Ok(())
}
