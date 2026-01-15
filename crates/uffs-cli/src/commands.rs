//! CLI command implementations.

use std::path::Path;

use anyhow::{Context, Result};
use uffs_core::{export_csv, export_json, export_table, MftQuery};
use uffs_mft::MftReader;

/// Search for files matching a pattern.
#[allow(clippy::too_many_arguments)]
pub async fn search(
    pattern: &str,
    drive: Option<char>,
    index: Option<std::path::PathBuf>,
    files_only: bool,
    dirs_only: bool,
    min_size: Option<u64>,
    max_size: Option<u64>,
    limit: u32,
    format: &str,
) -> Result<()> {
    // Load data from index or live MFT
    let df = if let Some(index_path) = index {
        MftReader::load_parquet(&index_path)
            .with_context(|| format!("Failed to load index: {}", index_path.display()))?
    } else {
        let drive = drive.context("Either --drive or --index must be specified")?;
        let reader = MftReader::open(drive)
            .await
            .with_context(|| format!("Failed to open drive {drive}:"))?;
        reader.read_all().await?
    };

    // Build query
    let mut query = MftQuery::new(df);

    // Apply pattern filter
    query = query.glob(pattern)?;

    // Apply type filters
    if files_only {
        query = query.files_only();
    } else if dirs_only {
        query = query.directories_only();
    }

    // Apply size filters
    if let Some(min) = min_size {
        query = query.min_size(min);
    }
    if let Some(max) = max_size {
        query = query.max_size(max);
    }

    // Apply limit
    query = query.limit(limit);

    // Execute query
    let results = query.collect()?;

    // Output results
    let stdout = std::io::stdout();
    match format {
        "json" => export_json(&results, stdout)?,
        "csv" => export_csv(&results, stdout)?,
        _ => export_table(&results, stdout)?,
    }

    eprintln!("\nFound {} results", results.height());

    Ok(())
}

/// Build an index from a drive's MFT.
pub async fn index(drive: char, output: &Path) -> Result<()> {
    eprintln!("Indexing drive {drive}:...");

    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {drive}:"))?;

    let mut df = reader.read_all().await?;

    eprintln!("Read {} records", df.height());

    MftReader::save_parquet(&mut df, output)
        .with_context(|| format!("Failed to save index to {}", output.display()))?;

    eprintln!("Index saved to {}", output.display());

    Ok(())
}

/// Show information about an index file.
pub fn info(path: &Path) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load index: {}", path.display()))?;

    println!("Index: {}", path.display());
    println!("Records: {}", df.height());
    println!("Columns: {}", df.width());
    println!();
    println!("Schema:");
    let schema = df.schema();
    for (name, dtype) in schema.iter() {
        println!("  {name}: {dtype:?}");
    }

    Ok(())
}

