//! CLI command implementations.
//!
//! This module provides the core command implementations for the UFFS CLI.
//! All public functions are async where I/O is involved and return
//! `anyhow::Result`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(windows)]
use std::sync::{Arc, Mutex};

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

/// Streaming output writer for multi-drive search.
///
/// Supports CSV (header + rows) and NDJSON (one JSON object per line) formats.
/// Writes results as each drive completes for immediate user feedback.
#[cfg(windows)]
struct StreamingWriter<W: Write> {
    writer: Mutex<W>,
    format: StreamingFormat,
    header_written: AtomicBool,
    rows_written: AtomicUsize,
    limit: u32,
}

/// Output format for streaming writer.
#[cfg(windows)]
#[derive(Clone, Copy)]
enum StreamingFormat {
    Csv,
    Json,
}

#[cfg(windows)]
impl<W: Write> StreamingWriter<W> {
    fn new(writer: W, format: &str, limit: u32) -> Self {
        let fmt = match format.to_lowercase().as_str() {
            "json" => StreamingFormat::Json,
            _ => StreamingFormat::Csv,
        };
        Self {
            writer: Mutex::new(writer),
            format: fmt,
            header_written: AtomicBool::new(false),
            rows_written: AtomicUsize::new(0),
            limit,
        }
    }

    /// Write a DataFrame batch. Returns number of rows written.
    fn write_batch(&self, df: &uffs_mft::DataFrame) -> Result<usize> {
        if df.height() == 0 {
            return Ok(0);
        }

        // Check if we've hit the limit
        if self.limit > 0 {
            let current = self.rows_written.load(Ordering::Relaxed);
            if current >= self.limit as usize {
                return Ok(0);
            }
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock error: {e}"))?;

        match self.format {
            StreamingFormat::Csv => self.write_csv_batch(&mut *writer, df),
            StreamingFormat::Json => self.write_json_batch(&mut *writer, df),
        }
    }

    fn write_csv_batch(&self, writer: &mut W, df: &uffs_mft::DataFrame) -> Result<usize> {
        let col_names: Vec<_> = df.get_column_names();

        // Write header only once
        if !self.header_written.swap(true, Ordering::SeqCst) {
            for (i, name) in col_names.iter().enumerate() {
                if i > 0 {
                    write!(writer, ",")?;
                }
                write!(writer, "\"{name}\"")?;
            }
            writeln!(writer)?;
        }

        // Cache column references to avoid repeated lookups
        let columns: Vec<_> = col_names
            .iter()
            .filter_map(|name| df.column(name).ok())
            .collect();

        let mut rows_written = 0;
        let height = df.height();
        let mut line_buf = String::with_capacity(256);

        for row_idx in 0..height {
            // Check limit
            if self.limit > 0 {
                let current = self.rows_written.fetch_add(1, Ordering::Relaxed);
                if current >= self.limit as usize {
                    break;
                }
            } else {
                self.rows_written.fetch_add(1, Ordering::Relaxed);
            }

            // Reuse buffer for each line
            line_buf.clear();
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    line_buf.push(',');
                }
                line_buf.push_str(&format_cell_value(col, row_idx));
            }
            writeln!(writer, "{line_buf}")?;
            rows_written += 1;
        }

        writer.flush()?;
        Ok(rows_written)
    }

    fn write_json_batch(&self, writer: &mut W, df: &uffs_mft::DataFrame) -> Result<usize> {
        let col_names: Vec<_> = df.get_column_names();

        // Cache column references to avoid repeated lookups
        let columns: Vec<_> = col_names
            .iter()
            .filter_map(|name| df.column(name).ok().map(|col| (*name, col)))
            .collect();

        let mut rows_written = 0;
        let height = df.height();
        let mut obj = String::with_capacity(512);

        for row_idx in 0..height {
            // Check limit
            if self.limit > 0 {
                let current = self.rows_written.fetch_add(1, Ordering::Relaxed);
                if current >= self.limit as usize {
                    break;
                }
            } else {
                self.rows_written.fetch_add(1, Ordering::Relaxed);
            }

            // Reuse buffer for each JSON object
            obj.clear();
            obj.push('{');
            for (i, (col_name, col)) in columns.iter().enumerate() {
                if i > 0 {
                    obj.push_str(", ");
                }
                obj.push('"');
                obj.push_str(col_name);
                obj.push_str("\": ");
                obj.push_str(&format_json_value(col, row_idx));
            }
            obj.push('}');
            writeln!(writer, "{obj}")?;
            rows_written += 1;
        }

        writer.flush()?;
        Ok(rows_written)
    }

    /// Check if we've hit the output limit.
    fn limit_reached(&self) -> bool {
        if self.limit == 0 {
            return false;
        }
        self.rows_written.load(Ordering::Relaxed) >= self.limit as usize
    }

    /// Get total rows written.
    fn total_rows(&self) -> usize {
        self.rows_written.load(Ordering::Relaxed)
    }
}

/// Format a cell value for CSV output.
#[cfg(windows)]
fn format_cell_value(col: &uffs_polars::Column, row_idx: usize) -> String {
    use uffs_polars::{AnyValue, TimeUnit};

    let val = col.get(row_idx);
    match val {
        Ok(AnyValue::Null) => String::new(),
        Ok(AnyValue::String(s)) => format!("\"{}\"", s.replace('"', "\"\"")),
        Ok(AnyValue::Boolean(b)) => if b { "1" } else { "0" }.to_string(),
        Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) => {
            // Convert microseconds to datetime string
            let secs = ts / 1_000_000;
            let micros = (ts % 1_000_000) as u32;
            if let Some(dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
                format!("\"{}\"", dt.format("%Y-%m-%d %H:%M:%S"))
            } else {
                String::new()
            }
        }
        Ok(v) => v.to_string(),
        Err(_) => String::new(),
    }
}

/// Format a cell value for JSON output.
#[cfg(windows)]
fn format_json_value(col: &uffs_polars::Column, row_idx: usize) -> String {
    use uffs_polars::{AnyValue, TimeUnit};

    let val = col.get(row_idx);
    match val {
        Ok(AnyValue::Null) => "null".to_string(),
        Ok(AnyValue::String(s)) => format!("\"{}\"", s.replace('"', "\\\"").replace('\n', "\\n")),
        Ok(AnyValue::Boolean(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) => {
            let secs = ts / 1_000_000;
            let micros = (ts % 1_000_000) as u32;
            if let Some(dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
                format!("\"{}\"", dt.format("%Y-%m-%d %H:%M:%S"))
            } else {
                "null".to_string()
            }
        }
        Ok(AnyValue::UInt8(n)) => n.to_string(),
        Ok(AnyValue::UInt16(n)) => n.to_string(),
        Ok(AnyValue::UInt32(n)) => n.to_string(),
        Ok(AnyValue::UInt64(n)) => n.to_string(),
        Ok(AnyValue::Int8(n)) => n.to_string(),
        Ok(AnyValue::Int16(n)) => n.to_string(),
        Ok(AnyValue::Int32(n)) => n.to_string(),
        Ok(AnyValue::Int64(n)) => n.to_string(),
        Ok(AnyValue::Float32(n)) => n.to_string(),
        Ok(AnyValue::Float64(n)) => n.to_string(),
        Ok(v) => format!("\"{}\"", v.to_string().replace('"', "\\\"")),
        Err(_) => "null".to_string(),
    }
}

use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;
use uffs_core::{MftQuery, export_csv, export_json, export_table};
use uffs_mft::{MftProgress, MftReader};

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
    index: Option<PathBuf>,
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

    // Build output configuration
    let output_config = OutputConfig::new()
        .with_columns(columns)
        .with_separator(sep)
        .with_quote(quotes)
        .with_header(header)
        .with_pos(pos)
        .with_neg(neg);

    // Streaming mode for multi-drive searches (Windows only)
    #[cfg(windows)]
    {
        let needs_streaming = index.is_none()
            && (multi_drives.is_some()
                || (single_drive.is_none() && filters.parsed.drive().is_none()));

        if needs_streaming {
            // Streaming mode: output results as each drive completes
            return search_streaming(
                multi_drives,
                single_drive,
                &filters,
                format,
                out,
                &output_config,
            )
            .await;
        }
    }

    // Non-streaming mode: load all data, then output
    // Pass needs_paths so path resolution happens BEFORE filtering loses parent
    // directories
    let needs_paths = output_config.needs_path_column();
    let mut results =
        load_and_filter_data(index, multi_drives, single_drive, &filters, needs_paths).await?;

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

/// Streaming search for multi-drive queries.
///
/// Outputs results as each drive completes, providing immediate feedback.
/// Uses CSV or NDJSON format for streaming compatibility.
#[cfg(windows)]
async fn search_streaming(
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    format: &str,
    out: &str,
    _output_config: &OutputConfig,
) -> Result<()> {
    // Determine drives to search
    let drives: Vec<char> = if let Some(drives) = multi_drives {
        drives
    } else if let Some(drive) = single_drive.or_else(|| filters.parsed.drive()) {
        vec![drive]
    } else {
        // All drives
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
        info!(drives = ?all_drives, count = all_drives.len(), "Searching all NTFS drives");
        all_drives
    };

    // Create streaming writer
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout = std::io::stdout();
        search_multi_drive_streaming(&drives, filters, format, stdout).await
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let writer = BufWriter::new(file);
        search_multi_drive_streaming(&drives, filters, format, writer).await?;
        info!(file = out, "Results written to file");
        Ok(())
    }
}

/// Load and filter search data from index file, multiple drives, single drive,
/// or all NTFS drives.
///
/// For multi-drive searches, applies filters per-drive to reduce memory usage.
/// This prevents OOM errors when searching many drives with millions of files.
///
/// # Arguments
///
/// * `needs_paths` - If true, resolves full paths using `FastPathResolver`
///   built from FULL MFT data BEFORE filtering. This ensures parent directories
///   are available for path resolution.
#[allow(clippy::single_call_fn)] // Extracted to reduce search() line count below clippy::too_many_lines limit
async fn load_and_filter_data(
    index: Option<PathBuf>,
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    if let Some(index_path) = index {
        // Load from pre-built index and filter
        // Note: For index files, path resolution uses the filtered data
        // which may have incomplete paths if the index was built from filtered data
        let df = MftReader::load_parquet(&index_path)
            .with_context(|| format!("Failed to load index: {}", index_path.display()))?;
        return execute_query(df, filters);
    }

    if let Some(drives) = multi_drives {
        // Multi-drive search with per-drive filtering (memory efficient)
        return search_multi_drive_filtered(&drives, filters, needs_paths).await;
    }

    // Check for single drive: CLI flag overrides pattern-embedded drive
    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    if let Some(drive_letter) = effective_drive {
        // Single drive search with proper path resolution
        let reader = MftReader::open(drive_letter)
            .await
            .with_context(|| format!("Failed to open drive {drive_letter}:"))?;
        let full_df = reader.read_all().await?;

        // Build path resolver from FULL data BEFORE filtering
        let mut path_resolver = if needs_paths {
            Some(
                uffs_core::FastPathResolver::build(&full_df, drive_letter)
                    .context("Failed to build path resolver")?,
            )
        } else {
            None
        };

        // Apply filters
        let mut filtered = execute_query(full_df, filters)?;

        // Add paths using the pre-built resolver (auto-selects parallel for large
        // DataFrames)
        if let Some(ref mut resolver) = path_resolver {
            filtered = resolver
                .add_path_column_auto(&filtered)
                .context("Failed to add path column")?;
        }

        return Ok(filtered);
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
        search_multi_drive_filtered(&all_drives, filters, needs_paths).await
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

    // Apply limit (0 = unlimited) and execute
    if filters.limit > 0 {
        query = query.limit(filters.limit);
    }
    Ok(query.collect()?)
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

/// Owned version of `QueryFilters` for parallel tasks.
///
/// This struct owns all its data so it can be sent across thread boundaries.
#[cfg(windows)]
#[derive(Clone)]
struct OwnedQueryFilters {
    /// Parsed search pattern (glob, regex, or literal).
    parsed: ParsedPattern,
    /// Extension filter string (e.g., "pictures,mp4,pdf").
    ext_filter: Option<String>,
    /// Only return files (not directories).
    files_only: bool,
    /// Only return directories (not files).
    dirs_only: bool,
    /// Minimum file size filter.
    min_size: Option<u64>,
    /// Maximum file size filter.
    max_size: Option<u64>,
}

#[cfg(windows)]
impl OwnedQueryFilters {
    /// Create owned filters from borrowed filters.
    fn from_borrowed(filters: &QueryFilters<'_>) -> Self {
        Self {
            parsed: filters.parsed.clone(),
            ext_filter: filters.ext_filter.map(String::from),
            files_only: filters.files_only,
            dirs_only: filters.dirs_only,
            min_size: filters.min_size,
            max_size: filters.max_size,
        }
    }

    /// Execute query with these filters.
    fn execute(&self, df: uffs_mft::DataFrame) -> Result<uffs_mft::DataFrame> {
        let mut query = MftQuery::new(df);

        // Apply pattern filter
        query = query.pattern(&self.parsed)?;

        // Apply extension filter if specified
        if let Some(ext_str) = &self.ext_filter {
            let parsed_ext_filter = ExtensionFilter::parse(ext_str)
                .map_err(|err| anyhow::anyhow!("Invalid extension filter: {err}"))?;
            query = query.extension_filter(&parsed_ext_filter);
        }

        // Apply type filters
        if self.files_only {
            query = query.files_only();
        } else if self.dirs_only {
            query = query.directories_only();
        }

        // Apply size filters
        if let Some(min) = self.min_size {
            query = query.min_size(min);
        }
        if let Some(max) = self.max_size {
            query = query.max_size(max);
        }

        // Don't apply limit per-drive - limit is applied to final merged result
        Ok(query.collect()?)
    }
}

/// Result from a single drive read operation.
#[cfg(windows)]
struct DriveResult {
    /// Drive letter that was read.
    drive: char,
    /// Filtered `DataFrame` with matching results (None if no matches or
    /// error).
    /// **Note:** Paths are already resolved using the full MFT data.
    df: Option<uffs_mft::DataFrame>,
    /// Total records read from the MFT.
    records_read: usize,
    /// Number of records matching the filters.
    matches: usize,
    /// Error message if the drive read failed.
    error: Option<String>,
    /// Whether paths were resolved (for logging).
    paths_resolved: bool,
}

/// Search multiple drives in parallel with per-drive filtering.
///
/// This approach spawns all drive reads concurrently using tokio tasks,
/// then collects and merges results as they complete. This maximizes I/O
/// parallelism across multiple drives.
///
/// # Path Resolution
///
/// When `needs_paths` is true, builds a FastPathResolver from the FULL MFT data
/// BEFORE filtering. This ensures parent directories are available for path
/// resolution, fixing the `<unknown>` path bug.
#[cfg(windows)]
async fn search_multi_drive_filtered(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use uffs_mft::{IntoLazy, col, lit};

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    info!(
        count = drives.len(),
        needs_paths = needs_paths,
        "Searching drives in PARALLEL (blazing fast mode)"
    );

    // Create owned filters that can be sent to tasks
    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));

    // Create multi-progress bar container
    let multi_progress = create_multi_progress();

    // Create progress bars for all drives upfront (wrapped in Arc for sharing)
    let progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>> =
        multi_progress.as_ref().map(|mp| {
            let mut pbs = std::collections::HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(mp, drive_char));
            }
            Arc::new(pbs)
        });

    // Channel for receiving results from drive tasks
    let (tx, mut rx) = mpsc::channel::<DriveResult>(drives.len());

    // Spawn all drive reads concurrently
    for &drive_char in drives {
        let tx = tx.clone();
        let filters = Arc::clone(&owned_filters);
        let pbs = progress_bars.clone();

        tokio::spawn(async move {
            let pb = pbs.as_ref().and_then(|p| p.get(&drive_char));

            // Open the drive
            let reader = match MftReader::open(drive_char).await {
                Ok(r) => r,
                Err(e) => {
                    if let Some(p) = pb {
                        p.finish_with_message(format!("Error: {e}"));
                    }
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read: 0,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            // Read with progress callback
            let pb_clone = pbs.clone();
            let full_df = reader
                .read_with_progress(move |progress| {
                    if let Some(ref pbs) = pb_clone {
                        if let Some(p) = pbs.get(&drive_char) {
                            if let Some(total) = progress.total_records {
                                p.set_length(progress.bytes_read.max(total));
                            }
                            p.set_position(progress.bytes_read);
                        }
                    }
                })
                .await;

            let full_df = match full_df {
                Ok(df) => df,
                Err(e) => {
                    if let Some(p) = pb {
                        p.finish_with_message(format!("Error: {e}"));
                    }
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read: 0,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            let records_read = full_df.height();
            if let Some(p) = pb {
                p.finish();
            }

            // Build path resolver from FULL data BEFORE filtering
            // This is the key fix for the <unknown> path bug!
            let mut path_resolver = if needs_paths {
                match uffs_core::FastPathResolver::build(&full_df, drive_char) {
                    Ok(resolver) => Some(resolver),
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches: 0,
                                error: Some(format!("Failed to build path resolver: {e}")),
                                paths_resolved: false,
                            })
                            .await;
                        return;
                    }
                }
            } else {
                None
            };

            // Apply filters
            let filtered = match filters.execute(full_df) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            let matches = filtered.height();

            // Add paths using the pre-built resolver (auto-selects parallel for large
            // DataFrames)
            let with_paths = if let Some(ref mut resolver) = path_resolver {
                match resolver.add_path_column_auto(&filtered) {
                    Ok(df) => df,
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(format!("Failed to add paths: {e}")),
                                paths_resolved: false,
                            })
                            .await;
                        return;
                    }
                }
            } else {
                filtered
            };

            // Add drive column
            let df_with_drive = if matches > 0 {
                match with_paths
                    .lazy()
                    .with_column(lit(format!("{drive_char}:")).alias("drive"))
                    .collect()
                {
                    Ok(df) => Some(df),
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(e.to_string()),
                                paths_resolved: path_resolver.is_some(),
                            })
                            .await;
                        return;
                    }
                }
            } else {
                None
            };

            let _ = tx
                .send(DriveResult {
                    drive: drive_char,
                    df: df_with_drive,
                    records_read,
                    matches,
                    error: None,
                    paths_resolved: path_resolver.is_some(),
                })
                .await;
        });
    }

    // Drop our sender so the channel closes when all tasks complete
    drop(tx);

    // Collect results as they arrive
    let mut filtered_results: Vec<uffs_mft::DataFrame> = Vec::new();
    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while let Some(result) = rx.recv().await {
        drives_processed += 1;

        if let Some(error) = result.error {
            info!(
                drive = %result.drive,
                error = %error,
                "Drive failed"
            );
            continue;
        }

        total_matches += result.matches;

        info!(
            drive = %result.drive,
            records = result.records_read,
            matches = result.matches,
            progress = format!("{}/{}", drives_processed, drives.len()),
            "Drive completed"
        );

        if let Some(df) = result.df {
            filtered_results.push(df);
        }
    }

    if filtered_results.is_empty() {
        bail!("No matching files found across {} drives", drives.len());
    }

    // Merge all results
    let mut merged = filtered_results.remove(0);
    for df in filtered_results {
        merged = merged.vstack(&df).context("Failed to merge results")?;
    }

    // Reorder columns to put "drive" first
    let column_names: Vec<String> = merged
        .get_column_names()
        .into_iter()
        .filter(|c| c.as_str() != "drive")
        .map(|c| c.to_string())
        .collect();
    let columns: Vec<_> = std::iter::once("drive".to_string())
        .chain(column_names)
        .map(|s| col(&s))
        .collect();

    let mut lazy_result = merged.lazy().select(columns);

    // Apply limit to final merged result (0 = unlimited)
    if filters.limit > 0 {
        lazy_result = lazy_result.limit(filters.limit);
    }

    let result = lazy_result.collect().context("Failed to reorder columns")?;

    info!(
        total_matches = total_matches,
        drives = drives.len(),
        "Parallel multi-drive search complete"
    );

    Ok(result)
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
#[allow(clippy::unused_async, clippy::single_call_fn)]
async fn search_multi_drive_filtered(
    _drives: &[char],
    _filters: &QueryFilters<'_>,
    _needs_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    bail!("Multi-drive search is only supported on Windows")
}

/// Search multiple drives in parallel with streaming output.
///
/// Outputs results as each drive completes, providing immediate feedback.
/// No progress bars - the streaming output IS the progress indicator.
#[cfg(windows)]
async fn search_multi_drive_streaming<W: Write + Send + 'static>(
    drives: &[char],
    filters: &QueryFilters<'_>,
    format: &str,
    writer: W,
) -> Result<()> {
    use tokio::sync::mpsc;
    use uffs_mft::{IntoLazy, col, lit};

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    info!(
        count = drives.len(),
        "Streaming search across drives (results appear as each drive completes)"
    );

    // Create owned filters that can be sent to tasks
    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));

    // Create streaming writer (shared across all results)
    let streaming_writer = Arc::new(StreamingWriter::new(writer, format, filters.limit));

    // Channel for receiving results from drive tasks
    let (tx, mut rx) = mpsc::channel::<DriveResult>(drives.len());

    // Spawn all drive reads concurrently
    for &drive_char in drives {
        let tx = tx.clone();
        let filters = Arc::clone(&owned_filters);

        tokio::spawn(async move {
            // Open the drive
            let reader = match MftReader::open(drive_char).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read: 0,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            // Read without progress callback (streaming output is the progress)
            let df = reader.read_all().await;

            let df = match df {
                Ok(df) => df,
                Err(e) => {
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read: 0,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            let records_read = df.height();

            // Apply filters
            let filtered = match filters.execute(df) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx
                        .send(DriveResult {
                            drive: drive_char,
                            df: None,
                            records_read,
                            matches: 0,
                            error: Some(e.to_string()),
                            paths_resolved: false,
                        })
                        .await;
                    return;
                }
            };

            let matches = filtered.height();

            // Add drive column
            let df_with_drive = if matches > 0 {
                match filtered
                    .lazy()
                    .with_column(lit(format!("{drive_char}:")).alias("drive"))
                    .collect()
                {
                    Ok(df) => Some(df),
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(e.to_string()),
                                paths_resolved: false,
                            })
                            .await;
                        return;
                    }
                }
            } else {
                None
            };

            let _ = tx
                .send(DriveResult {
                    drive: drive_char,
                    df: df_with_drive,
                    records_read,
                    matches,
                    error: None,
                    paths_resolved: false,
                })
                .await;
        });
    }

    // Drop our sender so the channel closes when all tasks complete
    drop(tx);

    // Stream results as they arrive
    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while let Some(result) = rx.recv().await {
        drives_processed += 1;

        if let Some(error) = result.error {
            // Log errors to stderr, not stdout (which has streaming data)
            eprintln!("[{}:] Error: {}", result.drive, error);
            continue;
        }

        total_matches += result.matches;

        // Stream output immediately
        if let Some(ref df) = result.df {
            // Reorder columns to put "drive" first
            let column_names: Vec<String> = df
                .get_column_names()
                .into_iter()
                .filter(|c| c.as_str() != "drive")
                .map(|c| c.to_string())
                .collect();
            let columns: Vec<_> = std::iter::once("drive".to_string())
                .chain(column_names)
                .map(|s| col(&s))
                .collect();

            if let Ok(reordered) = df.clone().lazy().select(columns).collect() {
                if let Err(e) = streaming_writer.write_batch(&reordered) {
                    eprintln!("[{}:] Write error: {}", result.drive, e);
                }
            }
        }

        // Check if we've hit the limit
        if streaming_writer.limit_reached() {
            info!(
                limit = filters.limit,
                "Output limit reached, stopping early"
            );
            break;
        }

        info!(
            drive = %result.drive,
            records = result.records_read,
            matches = result.matches,
            progress = format!("{}/{}", drives_processed, drives.len()),
            "Drive completed"
        );
    }

    info!(
        total_matches = total_matches,
        rows_output = streaming_writer.total_rows(),
        drives = drives.len(),
        "Streaming search complete"
    );

    Ok(())
}

/// Build an index from drive MFT(s).
///
/// Supports both single drive (`--drive C`) and multiple drives (`--drives
/// C,D,E`). When multiple drives are specified, they are read concurrently and
/// merged into a single `DataFrame` with a `drive` column.
///
/// If no drives are specified, indexes ALL available NTFS drives.
// Public API entry point - called from main.rs command dispatch
#[allow(clippy::single_call_fn)]
pub async fn index(
    output_path: PathBuf,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
) -> Result<()> {
    // Ensure output has an extension (default to .parquet)
    let output = if output_path.extension().is_some() {
        output_path
    } else {
        output_path.with_extension("parquet")
    };

    // Determine which drives to index
    let drive_list: Vec<char> = match (single_drive, multi_drives) {
        (Some(drv), None) => vec![drv],
        (None, Some(drvs)) => drvs,
        (None, None) => {
            // No drives specified - index ALL available NTFS drives
            #[cfg(windows)]
            {
                if !uffs_mft::is_elevated() {
                    anyhow::bail!(
                        "Administrator privileges required.\n\n\
                         UFFS reads the NTFS Master File Table directly, which requires elevated access.\n\n\
                         Solutions:\n\
                         1. Run PowerShell/Terminal as Administrator\n\
                         2. Specify a drive explicitly: uffs index --drive C output.parquet"
                    );
                }
                let all_drives = uffs_mft::detect_ntfs_drives();
                if all_drives.is_empty() {
                    anyhow::bail!("No NTFS drives found on this system");
                }
                info!(drives = ?all_drives, count = all_drives.len(), "No drive specified - indexing all NTFS drives");
                all_drives
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!(
                    "No drive specified. Use --drive or --drives to specify which drive(s) to index."
                );
            }
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

            MftReader::save_parquet(&mut df, &output)
                .with_context(|| format!("Failed to save index to {}", output.display()))?;

            info!(path = %output.display(), "Index saved");
            return Ok(());
        }
    }

    // Multiple drives: use MultiDriveMftReader
    index_multi_drive(&drive_list, &output).await
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
    let progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>> =
        mp.as_ref().map(|m| {
            let mut pbs = std::collections::HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(m, drive_char));
            }
            Arc::new(pbs)
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
#[allow(clippy::single_call_fn)]
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
#[allow(clippy::single_call_fn, clippy::too_many_lines)]
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

/// Format a number with comma separators.
fn format_number(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, ch) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
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
