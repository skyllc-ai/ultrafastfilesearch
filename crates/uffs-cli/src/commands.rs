//! CLI command implementations.
//!
//! This module provides the core command implementations for the UFFS CLI.
//! All public functions are async where I/O is involved and return
//! `anyhow::Result`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
#[cfg(windows)]
use indicatif::MultiProgress;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;
use uffs_core::extensions::ExtensionFilter;

/// Check if progress bars are disabled via `UFFS_NO_PROGRESS=1` environment
/// variable.
#[cfg(windows)]
#[inline]
fn is_progress_disabled() -> bool {
    std::env::var("UFFS_NO_PROGRESS")
        .map(|val| val == "1" || val.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Create a multi-progress container for multiple drives.
/// Returns `None` if progress is disabled via `UFFS_NO_PROGRESS=1`.
#[cfg(windows)]
fn create_multi_progress() -> Option<MultiProgress> {
    if is_progress_disabled() {
        None
    } else {
        Some(MultiProgress::new())
    }
}

/// Add a drive progress bar to a multi-progress container.
#[cfg(windows)]
fn add_drive_progress(multi_progress: &MultiProgress, drive: char) -> ProgressBar {
    let progress_bar = multi_progress.add(ProgressBar::new(0));
    let template = format!(
        "{{spinner:.cyan}} [{drive}:] [{{elapsed_precise}}] {{bar:30.cyan/blue}} {{bytes}}/{{total_bytes}} ({{eta}})"
    );
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template(&template)
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("━━╸"),
    );
    progress_bar
}

/// Create a progress bar for saving raw MFT bytes.
/// Returns `None` if progress is disabled via `UFFS_NO_PROGRESS=1`.
#[cfg(windows)]
fn create_save_raw_progress() -> Option<ProgressBar> {
    if is_progress_disabled() {
        return None;
    }

    let progress_bar = ProgressBar::new(0);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan} [{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} 💾 saving...")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("━━╸"),
    );
    Some(progress_bar)
}

use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;
use uffs_core::{MftQuery, export_csv, export_json, export_table};
#[cfg(windows)]
use uffs_mft::SaveRawOptions;
use uffs_mft::{MftProgress, MftReader, load_raw_mft_header};

/// Search for files matching a pattern.
///
/// Supports:
/// - Drive prefix in pattern: `c:/pro*` extracts drive C
/// - REGEX patterns: `>C:\\Temp.*` (starts with `>`)
/// - Glob patterns: `*.txt`, `**/*.rs`
/// - Literal search: `readme` (no wildcards)
/// - Multi-drive search: `--drives C,D,E`
/// - Extension filtering: `--ext pictures,mp4,pdf`
/// - Output customization: `--out`, `--columns`, `--sep`, `--quotes`,
///   `--header`, `--pos`, `--neg`
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub async fn search(
    pattern: &str,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
    index: Option<std::path::PathBuf>,
    files_only: bool,
    dirs_only: bool,
    min_size: Option<u64>,
    max_size: Option<u64>,
    limit: u32,
    format: &str,
    case_sensitive: bool,
    ext_filter: Option<&str>,
    out: &str,
    columns: &str,
    sep: &str,
    quotes: &str,
    header: bool,
    pos: &str,
    neg: &str,
) -> Result<()> {
    // Parse the pattern to extract drive prefix and pattern type
    let parsed = ParsedPattern::parse(pattern)
        .with_context(|| format!("Invalid pattern: {pattern}"))?
        .with_case_sensitive(case_sensitive);

    // Build filters struct for reuse
    let filters = QueryFilters {
        parsed: &parsed,
        ext_filter,
        files_only,
        dirs_only,
        min_size,
        max_size,
        limit,
    };

    // Load and filter data - for multi-drive, filter per-drive to reduce memory
    let mut results = load_and_filter_data(index, multi_drives, single_drive, &filters).await?;

    // Build output configuration
    let output_config = OutputConfig::new()
        .with_columns(columns)
        .with_separator(sep)
        .with_quote(quotes)
        .with_header(header)
        .with_pos(pos)
        .with_neg(neg);

    // Compute tree columns only if specifically requested
    if output_config.needs_tree_columns() {
        let tree_cols = output_config.get_tree_columns();
        info!(columns = tree_cols.len(), "Computing tree metrics");
        results =
            add_tree_columns(&results, &tree_cols).context("Failed to compute tree columns")?;
    }

    // Output results
    write_results(&results, format, out, &output_config)?;

    info!(count = results.height(), "Search complete");
    Ok(())
}

/// Load and filter search data from index file, multiple drives, single drive,
/// or all NTFS drives.
///
/// For multi-drive searches, applies filters per-drive to reduce memory usage.
/// This prevents OOM errors when searching many drives with millions of files.
#[allow(clippy::single_call_fn)] // Extracted to reduce search() line count below clippy::too_many_lines limit
async fn load_and_filter_data(
    index: Option<std::path::PathBuf>,
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
) -> Result<uffs_mft::DataFrame> {
    if let Some(index_path) = index {
        // Load from pre-built index and filter
        let df = MftReader::load_parquet(&index_path)
            .with_context(|| format!("Failed to load index: {}", index_path.display()))?;
        return execute_query(df, filters);
    }

    if let Some(drives) = multi_drives {
        // Multi-drive search with per-drive filtering (memory efficient)
        return search_multi_drive_filtered(&drives, filters).await;
    }

    // Check for single drive: CLI flag overrides pattern-embedded drive
    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    if let Some(drive_letter) = effective_drive {
        // Single drive search
        let reader = MftReader::open(drive_letter)
            .await
            .with_context(|| format!("Failed to open drive {drive_letter}:"))?;
        let df = reader.read_all().await?;
        return execute_query(df, filters);
    }

    // No drive specified - search ALL available NTFS drives
    #[cfg(windows)]
    {
        if !uffs_mft::is_elevated() {
            bail!(
                "Administrator privileges required.\n\n\
                 UFFS reads the NTFS Master File Table directly, which requires elevated access.\n\n\
                 Solutions:\n\
                 1. Run PowerShell/Terminal as Administrator\n\
                 2. Use a pre-built index: uffs search --index <file.parquet> \"*.txt\""
            );
        }
        let all_drives = uffs_mft::detect_ntfs_drives();
        if all_drives.is_empty() {
            bail!("No NTFS drives found on this system");
        }
        info!(drives = ?all_drives, count = all_drives.len(), "No drive specified - searching all NTFS drives");
        search_multi_drive_filtered(&all_drives, filters).await
    }
    #[cfg(not(windows))]
    {
        bail!(
            "No drive specified. Use --drive, --drives, --index, or include drive in pattern (e.g., c:/pro*)"
        )
    }
}

/// Query filter options for the search command.
struct QueryFilters<'a> {
    /// Parsed search pattern (glob, regex, or literal).
    parsed: &'a ParsedPattern,
    /// Extension filter string (e.g., "pictures,mp4,pdf").
    ext_filter: Option<&'a str>,
    /// Only return files (not directories).
    files_only: bool,
    /// Only return directories (not files).
    dirs_only: bool,
    /// Minimum file size filter.
    min_size: Option<u64>,
    /// Maximum file size filter.
    max_size: Option<u64>,
    /// Maximum number of results to return.
    limit: u32,
}

/// Build and execute the MFT query with all filters applied.
#[allow(clippy::single_call_fn)] // Extracted to reduce search() line count below clippy::too_many_lines limit
fn execute_query(
    df: uffs_mft::DataFrame,
    filters: &QueryFilters<'_>,
) -> Result<uffs_mft::DataFrame> {
    let mut query = MftQuery::new(df);

    // Apply pattern filter
    query = query.pattern(filters.parsed)?;

    // Apply extension filter if specified
    if let Some(ext_str) = filters.ext_filter {
        let parsed_ext_filter = ExtensionFilter::parse(ext_str)
            .map_err(|err| anyhow::anyhow!("Invalid extension filter: {err}"))?;
        query = query.extension_filter(&parsed_ext_filter);
    }

    // Apply type filters
    if filters.files_only {
        query = query.files_only();
    } else if filters.dirs_only {
        query = query.directories_only();
    }

    // Apply size filters
    if let Some(min) = filters.min_size {
        query = query.min_size(min);
    }
    if let Some(max) = filters.max_size {
        query = query.max_size(max);
    }

    // Apply limit and execute
    Ok(query.limit(filters.limit).collect()?)
}

/// Write search results to console or file.
#[allow(clippy::single_call_fn)] // Extracted to reduce search() line count below clippy::too_many_lines limit
fn write_results(
    results: &uffs_mft::DataFrame,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
) -> Result<()> {
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout = std::io::stdout();
        match format {
            "json" => export_json(results, stdout)?,
            "csv" => export_csv(results, stdout)?,
            "custom" => output_config.write(results, stdout)?,
            _ => export_table(results, stdout)?,
        }
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let writer = BufWriter::new(file);

        match format {
            "json" => export_json(results, writer)?,
            "csv" => export_csv(results, writer)?,
            _ => output_config.write(results, writer)?,
        }
        info!(file = out, "Results written to file");
    }

    Ok(())
}

/// Search multiple drives sequentially with per-drive filtering.
///
/// This approach processes one drive at a time and applies filters immediately,
/// keeping only matching results in memory. This prevents OOM errors when
/// searching many drives with millions of files.
#[cfg(windows)]
async fn search_multi_drive_filtered(
    drives: &[char],
    filters: &QueryFilters<'_>,
) -> Result<uffs_mft::DataFrame> {
    use uffs_mft::{IntoLazy, col, lit};

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    info!(
        count = drives.len(),
        "Searching drives sequentially (memory-efficient mode)"
    );

    let multi_progress = create_multi_progress();
    let mut filtered_results: Vec<uffs_mft::DataFrame> = Vec::new();
    let mut total_matches = 0usize;

    // Process drives sequentially to limit memory usage
    for &drive_char in drives {
        // Create progress bar for this drive (wrapped in Arc for closure sharing)
        let pb: Option<std::sync::Arc<ProgressBar>> = multi_progress
            .as_ref()
            .map(|mp| std::sync::Arc::new(add_drive_progress(mp, drive_char)));

        // Read this drive
        let reader = match MftReader::open(drive_char).await {
            Ok(r) => r,
            Err(e) => {
                if let Some(ref p) = pb {
                    p.finish_with_message(format!("Error: {e}"));
                }
                info!(drive = %drive_char, error = %e, "Skipping drive due to error");
                continue;
            }
        };

        let pb_clone = pb.clone();
        let df = match reader
            .read_with_progress(move |progress| {
                if let Some(ref p) = pb_clone {
                    if let Some(total) = progress.total_records {
                        p.set_length(progress.bytes_read.max(total));
                    }
                    p.set_position(progress.bytes_read);
                }
            })
            .await
        {
            Ok(df) => df,
            Err(e) => {
                if let Some(ref p) = pb {
                    p.finish_with_message(format!("Error: {e}"));
                }
                info!(drive = %drive_char, error = %e, "Skipping drive due to read error");
                continue;
            }
        };

        let records_read = df.height();
        if let Some(ref p) = pb {
            p.finish();
        }

        // Apply filters immediately to reduce memory
        let filtered = execute_query(df, filters)?;
        let matches = filtered.height();
        total_matches += matches;

        info!(
            drive = %drive_char,
            records = records_read,
            matches = matches,
            "Drive processed"
        );

        if matches > 0 {
            // Add drive column and store filtered results
            let df_with_drive = filtered
                .lazy()
                .with_column(lit(format!("{drive_char}:")).alias("drive"))
                .collect()
                .context("Failed to add drive column")?;
            filtered_results.push(df_with_drive);
        }
    }

    if filtered_results.is_empty() {
        // Return empty DataFrame with correct schema
        bail!("No matching files found across {} drives", drives.len());
    }

    // Concatenate filtered results (much smaller than full data)
    let mut result = filtered_results.remove(0);
    for df in filtered_results {
        result = result.vstack(&df).context("Failed to merge results")?;
    }

    // Reorder columns to put "drive" first
    let column_names: Vec<String> = result
        .get_column_names()
        .into_iter()
        .filter(|c| c.as_str() != "drive")
        .map(|c| c.to_string())
        .collect();
    let columns: Vec<_> = std::iter::once("drive".to_string())
        .chain(column_names)
        .map(|s| col(&s))
        .collect();

    let result = result
        .lazy()
        .select(columns)
        .collect()
        .context("Failed to reorder columns")?;

    info!(
        total_matches = total_matches,
        drives = drives.len(),
        "Multi-drive search complete"
    );

    Ok(result)
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
#[allow(clippy::unused_async, clippy::single_call_fn)]
async fn search_multi_drive_filtered(
    _drives: &[char],
    _filters: &QueryFilters<'_>,
) -> Result<uffs_mft::DataFrame> {
    bail!("Multi-drive search is only supported on Windows")
}

/// Build an index from a drive's MFT.
///
/// Supports both single drive (`--drive C`) and multiple drives (`--drives
/// C,D,E`). When multiple drives are specified, they are read concurrently and
/// merged into a single `DataFrame` with a `drive` column.
// CLI command handler - separate function for testability and maintainability.
#[allow(clippy::shadow_unrelated, clippy::single_call_fn)]
pub async fn index(
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
    output: &Path,
) -> Result<()> {
    // Determine which drives to index
    let drive_list: Vec<char> = match (single_drive, multi_drives) {
        (Some(drv), None) => vec![drv],
        (None, Some(drvs)) => drvs,
        (None, None) => {
            anyhow::bail!("Either --drive or --drives must be specified");
        }
        (Some(_), Some(_)) => {
            // This shouldn't happen due to clap's conflicts_with, but handle it anyway
            anyhow::bail!("Cannot specify both --drive and --drives");
        }
    };

    if drive_list.is_empty() {
        anyhow::bail!("No drives specified");
    }

    // Single drive: use the original simple path
    if let Some(&drive_letter) = drive_list.first() {
        if drive_list.len() == 1 {
            info!(drive = %drive_letter, "Indexing drive");

            let reader = MftReader::open(drive_letter)
                .await
                .with_context(|| format!("Failed to open drive {drive_letter}:"))?;

            // Create progress bar (None if disabled via UFFS_NO_PROGRESS=1)
            let progress_disabled = std::env::var("UFFS_NO_PROGRESS")
                .map(|val| val == "1" || val.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            let progress_bar: Option<ProgressBar> = if progress_disabled {
                None
            } else {
                let bar = ProgressBar::new(0);
                let template = format!(
                    "{{spinner:.cyan}} [{drive_letter}:] [{{elapsed_precise}}] {{bar:40.cyan/blue}} {{bytes}}/{{total_bytes}} 📖 reading MFT..."
                );
                bar.set_style(
                    ProgressStyle::default_bar()
                        .template(&template)
                        .unwrap_or_else(|_| ProgressStyle::default_bar())
                        .progress_chars("━━╸"),
                );
                Some(bar)
            };

            // Read MFT with progress callback
            let mut df = reader
                .read_with_progress(move |progress: MftProgress| {
                    if let Some(bar) = &progress_bar {
                        if let Some(total) = progress.total_records {
                            bar.set_length(progress.bytes_read.max(total));
                        }
                        bar.set_position(progress.bytes_read);
                    }
                })
                .await?;

            info!(records = df.height(), "Read records");

            MftReader::save_parquet(&mut df, output)
                .with_context(|| format!("Failed to save index to {}", output.display()))?;

            info!(path = %output.display(), "Index saved");
            return Ok(());
        }
    }

    // Multiple drives: use MultiDriveMftReader
    index_multi_drive(&drive_list, output).await
}

/// Index multiple drives concurrently.
#[cfg(windows)]
async fn index_multi_drive(drives: &[char], output: &Path) -> Result<()> {
    use uffs_mft::MultiDriveMftReader;

    let drive_str: String = drives
        .iter()
        .map(|c| format!("{c}:"))
        .collect::<Vec<_>>()
        .join(", ");
    info!(drives = %drive_str, "Indexing drives");

    let reader = MultiDriveMftReader::new(drives.to_vec());

    // Create a multi-progress bar for each drive (if not disabled)
    let mp = create_multi_progress();
    let progress_bars: Option<std::sync::Arc<std::collections::HashMap<char, ProgressBar>>> =
        mp.as_ref().map(|m| {
            let mut pbs = std::collections::HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(m, drive_char));
            }
            std::sync::Arc::new(pbs)
        });

    let pbs = progress_bars.clone();

    // Read all drives with progress
    let mut df = reader
        .read_with_progress(move |drive, progress| {
            if let Some(ref bars) = pbs {
                if let Some(pb) = bars.get(&drive) {
                    if let Some(total) = progress.total_records {
                        pb.set_length(progress.bytes_read.max(total));
                    }
                    pb.set_position(progress.bytes_read);
                }
            }
        })
        .await
        .context("Failed to read MFTs from drives")?;

    // Finish all progress bars
    if let Some(ref bars) = progress_bars {
        for pb in bars.values() {
            pb.finish();
        }
    }

    info!(
        records = df.height(),
        drives = drives.len(),
        "Read records from drives"
    );

    MftReader::save_parquet(&mut df, output)
        .with_context(|| format!("Failed to save index to {}", output.display()))?;

    info!(path = %output.display(), "Index saved");

    Ok(())
}

/// Index multiple drives (non-Windows stub).
#[cfg(not(windows))]
// Platform-specific stub must match Windows signature; called once per platform is expected.
#[allow(clippy::unused_async, clippy::single_call_fn)]
async fn index_multi_drive(_drives: &[char], _output: &Path) -> Result<()> {
    anyhow::bail!("Multi-drive indexing is only supported on Windows")
}

/// Show information about an index file.
///
/// # Errors
///
/// Returns an error if:
/// - The index file cannot be loaded
/// - Writing to stdout fails
// CLI command handler - separate function for testability and maintainability.
#[allow(clippy::single_call_fn)]
pub fn info(path: &Path) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load index: {}", path.display()))?;

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "Index: {}", path.display())?;
    writeln!(stdout, "Records: {}", df.height())?;
    writeln!(stdout, "Columns: {}", df.width())?;
    writeln!(stdout)?;
    writeln!(stdout, "Schema:")?;
    let schema = df.schema();
    for (name, dtype) in schema.iter() {
        writeln!(stdout, "  {name}: {dtype}")?;
    }

    Ok(())
}

/// Show statistics about files in an index.
///
/// # Errors
///
/// Returns an error if:
/// - The index file cannot be loaded
/// - Query execution fails
/// - Writing to stdout fails
// CLI command handler - separate function for testability and maintainability.
#[allow(clippy::single_call_fn)]
pub fn stats(path: &Path, top: u32) -> Result<()> {
    let df = MftReader::load_parquet(path)
        .with_context(|| format!("Failed to load index: {}", path.display()))?;

    let total_records = df.height();

    // Count files vs directories
    let files = MftQuery::new(df.clone()).files_only().collect()?;
    let dirs = MftQuery::new(df.clone()).directories_only().collect()?;

    let file_count = files.height();
    let dir_count = dirs.height();

    // Calculate total size
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

    // Top N largest files
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

/// Format file size in human-readable format.
#[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Save raw MFT bytes to a file for offline analysis.
#[cfg(windows)]
pub async fn save_raw(
    drive: char,
    output: &Path,
    compress: bool,
    compression_level: i32,
) -> Result<()> {
    info!(drive = %drive, "Reading raw MFT from drive");

    let reader = MftReader::open(drive)
        .await
        .with_context(|| format!("Failed to open drive {drive}:"))?;

    // Create progress bar (None if disabled)
    let pb = create_save_raw_progress();

    let options = SaveRawOptions {
        compress,
        compression_level,
    };

    let header = reader
        .save_raw_to_file(output, &options)
        .await
        .with_context(|| format!("Failed to save raw MFT to {}", output.display()))?;

    if let Some(ref p) = pb {
        p.finish_and_clear();
    }

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout)?;
    writeln!(stdout, "=== Raw MFT Saved ===")?;
    writeln!(stdout, "Output:          {}", output.display())?;
    writeln!(stdout, "Records:         {}", header.record_count)?;
    writeln!(stdout, "Record size:     {} bytes", header.record_size)?;
    writeln!(
        stdout,
        "Original size:   {}",
        format_size(header.original_size)
    )?;
    if header.is_compressed() {
        writeln!(
            stdout,
            "Compressed size: {}",
            format_size(header.compressed_size)
        )?;
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
        writeln!(stdout, "Compression:     {ratio:.1}%")?;
    } else {
        writeln!(stdout, "Compression:     none")?;
    }

    Ok(())
}

/// Save raw MFT bytes - non-Windows stub.
#[cfg(not(windows))]
// Platform-specific stub must match Windows signature; called once per platform is expected.
#[allow(clippy::unused_async, clippy::single_call_fn)]
pub async fn save_raw(
    _drive: char,
    _output: &Path,
    _compress: bool,
    _compression_level: i32,
) -> Result<()> {
    anyhow::bail!("Raw MFT saving is only supported on Windows");
}

/// Load raw MFT from a saved file.
///
/// # Errors
///
/// Returns an error if:
/// - The raw MFT file cannot be loaded
/// - Writing to stdout fails
/// - On non-Windows: always fails (NTFS parsing not supported)
// CLI command handler - separate function for testability and maintainability.
#[allow(clippy::single_call_fn)]
pub fn load_raw(input: &Path, output: Option<&Path>, info_only: bool) -> Result<()> {
    // Load header first
    let header = load_raw_mft_header(input)
        .with_context(|| format!("Failed to load raw MFT header from {}", input.display()))?;

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "=== Raw MFT File Info ===")?;
    writeln!(stdout, "File:            {}", input.display())?;
    writeln!(stdout, "Version:         {}", header.version)?;
    writeln!(stdout, "Records:         {}", header.record_count)?;
    writeln!(stdout, "Record size:     {} bytes", header.record_size)?;
    writeln!(
        stdout,
        "Original size:   {}",
        format_size(header.original_size)
    )?;
    if header.is_compressed() {
        writeln!(
            stdout,
            "Compressed size: {}",
            format_size(header.compressed_size)
        )?;
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let ratio = header.compressed_size as f64 / header.original_size as f64 * 100.0_f64;
        writeln!(stdout, "Compression:     {ratio:.1}%")?;
    } else {
        writeln!(stdout, "Compression:     none")?;
    }
    // Drop stdout lock before potentially long operations
    drop(stdout);

    if info_only {
        return Ok(());
    }

    // Parse and export
    #[cfg(windows)]
    {
        let output = output.context("--output is required when not using --info-only")?;

        info!("Parsing MFT records");

        let df = MftReader::load_raw_to_dataframe(input)
            .with_context(|| format!("Failed to parse raw MFT from {}", input.display()))?;

        info!(records = df.height(), "Parsed records");

        // Determine output format from extension
        let ext = output
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("parquet");

        match ext {
            "csv" => {
                let mut file = File::create(output)?;
                export_csv(&df, &mut file)?;
                info!(path = %output.display(), "Exported to CSV");
            }
            _ => {
                let mut df = df;
                MftReader::save_parquet(&mut df, output)?;
                info!(path = %output.display(), "Exported to Parquet");
            }
        }

        Ok(())
    }

    #[cfg(not(windows))]
    {
        // Silence unused variable warning on non-Windows
        let _: Option<&Path> = output;
        anyhow::bail!("Raw MFT parsing is only supported on Windows (requires NTFS parsing)");
    }
}
