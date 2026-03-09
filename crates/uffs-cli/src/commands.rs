//! CLI command implementations.
//!
//! This module provides the core command implementations for the UFFS CLI.
//! All public functions are async where I/O is involved and return
//! `anyhow::Result`.

// CLI command modules have many single-call functions by design (one per command/subcommand)
#![expect(
    clippy::single_call_fn,
    reason = "CLI command functions are called once from dispatch"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short names (d, v) conventional in closures"
)]
#![expect(
    clippy::print_stderr,
    reason = "CLI outputs user-facing messages to stderr"
)]

/// Binary string table tripwire for parity harness verification.
/// This constant is embedded in the binary and can be found with:
/// `strings uffs.exe | grep TRIPWIRE`
///
/// The `touch_tripwire()` function ensures the constant is not optimized away.
pub const TRIPWIRE: &str = concat!("TRIPWIRE_UFFS_CPP_TREE_FIX_v", env!("CARGO_PKG_VERSION"));

/// Touch the tripwire to ensure it's not optimized away by the compiler.
/// Call this from `main()` or early in the program.
#[inline(never)]
#[expect(
    clippy::single_call_fn,
    reason = "intentionally called once from main to prevent compiler optimization"
)]
pub fn touch_tripwire() {
    core::hint::black_box(TRIPWIRE);
}

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
use uffs_core::QueryMode;
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
    output_config: OutputConfig,
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
    fn new(writer: W, format: &str, limit: u32, output_config: OutputConfig) -> Self {
        let fmt = match format.to_lowercase().as_str() {
            "json" => StreamingFormat::Json,
            _ => StreamingFormat::Csv,
        };
        Self {
            writer: Mutex::new(writer),
            format: fmt,
            output_config,
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
        let height = df.height();
        if height == 0 {
            return Ok(0);
        }

        // Determine if we should write header (only first batch)
        let write_header = !self.header_written.swap(true, Ordering::SeqCst);

        // Apply limit if set
        let rows_to_write = if self.limit > 0 {
            let current = self.rows_written.load(Ordering::Relaxed);
            let remaining = (self.limit as usize).saturating_sub(current);
            if remaining == 0 {
                return Ok(0);
            }
            remaining.min(height)
        } else {
            height
        };

        // Slice DataFrame if we need fewer rows
        let df_slice = if rows_to_write < height {
            df.slice(0, rows_to_write)
        } else {
            df.clone()
        };

        // Use OutputConfig for proper formatting with C++ column names
        let mut config = self.output_config.clone();
        config.header = write_header;

        // Write using OutputConfig (handles column names, formatting, etc.)
        // Use &mut *writer to create a fresh reborrow so we can still use writer for
        // flush()
        config
            .write(&df_slice, &mut *writer)
            .map_err(|e| anyhow::anyhow!("Write error: {e}"))?;

        // Update rows written count
        self.rows_written
            .fetch_add(rows_to_write, Ordering::Relaxed);

        writer.flush()?;
        Ok(rows_to_write)
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

/// Determine if we should use the fast `MftIndex` query path.
///
/// Returns `true` if:
/// - `QueryMode::ForceIndex` is set, OR
/// - `QueryMode::Auto` AND query is simple (no parquet index, single drive, no
///   tree columns)
///
/// Returns `false` if:
/// - `QueryMode::ForceDataFrame` is set, OR
/// - Query requires features only available in `DataFrame` path
#[expect(
    clippy::single_call_fn,
    reason = "extracted for clarity and testability"
)]
fn should_use_index_path(
    mode: QueryMode,
    parquet_index: Option<&PathBuf>,
    multi_drives: Option<&Vec<char>>,
) -> bool {
    match mode {
        QueryMode::ForceIndex => {
            // User explicitly requested index path
            // Warn if features are incompatible
            if parquet_index.is_some() {
                info!("⚠️ --query-mode=index ignored: using parquet index file");
                return false;
            }
            if multi_drives.is_some() {
                info!("⚠️ --query-mode=index: multi-drive not yet supported, using single drive");
            }
            true
        }
        QueryMode::ForceDataFrame => {
            // User explicitly requested DataFrame path
            false
        }
        QueryMode::Auto => {
            // Auto mode: use index path by default (fast, cached)
            // Conditions that require DataFrame path:
            // 1. Loading from parquet index file (already a DataFrame)
            if parquet_index.is_some() {
                return false;
            }
            // Tree columns are now available via MftIndex path (no need to force DataFrame)
            // Use fast cached index path (works for single and multi-drive)
            true
        }
    }
}

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
#[expect(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    reason = "CLI entry point passes through all parsed args"
)]
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing output to stderr"
)]
#[expect(
    clippy::too_many_lines,
    reason = "top-level search orchestrator — splitting further would obscure control flow"
)]
#[expect(
    clippy::single_call_fn,
    reason = "public CLI entry point called from main dispatch"
)]
pub async fn search(
    pattern: &str,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
    index: Option<PathBuf>,
    mft_file: Option<PathBuf>,
    files_only: bool,
    dirs_only: bool,
    hide_system: bool,
    profile: bool,
    debug_tree: bool,
    benchmark: bool,
    no_bitmap: bool,
    no_cache: bool,
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
    query_mode: &str,
    tz_offset: Option<i32>,
) -> Result<()> {
    // Start timing for "Finished in X s" output (C++ compatibility)
    let start_time = std::time::Instant::now();

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
        hide_system,
        min_size,
        max_size,
        limit,
    };

    // Parse query mode
    let mode = QueryMode::from_str_opt(query_mode).unwrap_or_else(|| {
        info!(query_mode, "Unknown query mode, defaulting to auto");
        QueryMode::Auto
    });
    info!(?mode, "Query execution mode");

    // Tripwire for parity harness verification (Fix #6: don't break CSV)
    // Log to stderr and tracing instead of embedding in CSV output.
    // Also embedded in binary string table via TRIPWIRE constant.
    let tripwire = format!("TRIPWIRE_UFFS_CPP_TREE_FIX_v{}", env!("CARGO_PKG_VERSION"));
    // Log tripwire to tracing (appears in log files)
    tracing::info!("[TRIPWIRE] {}", tripwire);
    // Also emit to stderr for easy verification
    eprintln!("[TRIPWIRE] {tripwire}");

    let mut output_config = OutputConfig::new()
        .with_columns(columns)
        .with_separator(sep)
        .with_quote(quotes)
        .with_header(header)
        .with_pos(pos)
        .with_neg(neg);
    if let Some(hours) = tz_offset {
        output_config = output_config.with_tz_offset_hours(hours);
        info!(hours, "Timezone offset overridden via --tz-offset");
    }

    // Pass needs_paths so path resolution happens BEFORE filtering loses parent
    // directories (skip path resolution in benchmark mode for speed)
    let needs_paths = !benchmark && output_config.needs_path_column();

    // Decide which query path to use based on mode and query complexity
    // Index path is the default (fast, cached) - DataFrame path is fallback
    let use_index_path = should_use_index_path(mode, index.as_ref(), multi_drives.as_ref());

    // Determine drives to search (needed for C++ compatible "Drives?" output)
    #[cfg(windows)]
    let drives_to_search: Vec<char> = if let Some(ref drives) = multi_drives {
        drives.clone()
    } else if let Some(drive) = single_drive.or_else(|| filters.parsed.drive()) {
        vec![drive]
    } else {
        // No drive specified - search ALL available NTFS drives
        uffs_mft::detect_ntfs_drives()
    };

    // Determine drives for C++ compatible footer in output file
    // (computed before data loading since multi_drives may be consumed)
    let footer_drives: Vec<char> = single_drive
        .map(|d| vec![d])
        .or_else(|| multi_drives.clone())
        .or_else(|| filters.parsed.drive().map(|d| vec![d]))
        .unwrap_or_default();

    // Handle raw MFT file input (cross-platform debugging)
    let mut results = if let Some(mft_path) = mft_file.as_ref() {
        info!(path = %mft_path.display(), "📂 Loading from raw MFT file");
        load_and_filter_from_mft_file(
            mft_path,
            single_drive,
            &filters,
            needs_paths,
            profile,
            debug_tree,
        )?
    } else if use_index_path {
        info!("🚀 Using fast cached MftIndex query path");
        #[cfg(windows)]
        {
            // Use pre-computed drives_to_search
            if drives_to_search.is_empty() {
                bail!("No NTFS drives found on this system");
            }

            if drives_to_search.len() == 1 {
                // Single drive - use existing function
                load_and_filter_data_index(
                    Some(drives_to_search[0]),
                    &filters,
                    needs_paths,
                    profile,
                    no_cache,
                )
                .await?
            } else {
                // Multi-drive with cached index path
                load_and_filter_data_index_multi(
                    &drives_to_search,
                    &filters,
                    needs_paths,
                    profile,
                    no_cache,
                )
                .await?
            }
        }
        #[cfg(not(windows))]
        {
            _ = no_cache;
            bail!("Index query mode is only available on Windows");
        }
    } else {
        info!("📊 Using DataFrame query path");
        // Streaming mode for multi-drive DataFrame searches (Windows only)
        #[cfg(windows)]
        if !benchmark {
            let needs_streaming = index.is_none()
                && (multi_drives.is_some()
                    || (single_drive.is_none() && filters.parsed.drive().is_none()));

            if needs_streaming {
                // Streaming mode: output results as each drive completes
                let result = search_streaming(
                    multi_drives.clone(),
                    single_drive,
                    &filters,
                    format,
                    out,
                    &output_config,
                    no_bitmap,
                )
                .await;

                // C++ compatibility: Print "Drives?" and "MMMmmm" to stdout AFTER the data
                let elapsed = start_time.elapsed();
                let secs = elapsed.as_secs();

                #[expect(
                    clippy::print_stdout,
                    reason = "C++ compatibility requires stdout output"
                )]
                if !drives_to_search.is_empty() {
                    let drive_list: String = drives_to_search
                        .iter()
                        .map(|d| format!("{d}:"))
                        .collect::<Vec<_>>()
                        .join("|");
                    // C++ format: "\nDrives? \t{count}\t{drive_list}\n\n"
                    println!("\nDrives? \t{}\t{drive_list}\n", drives_to_search.len());
                }

                #[expect(
                    clippy::print_stdout,
                    reason = "C++ compatibility requires stdout output"
                )]
                if secs <= 1 {
                    println!(
                        "MMMmmm that was FAST ... maybe your searchstring was wrong?\t{pattern}\n\
                         Search path. E.g. 'C:/' or 'C:\\Prog**' "
                    );
                }

                // C++ compatibility: "Finished in X s" (to stderr)
                eprintln!("\nFinished \tin {secs} s\n");
                return result;
            }
        }

        load_and_filter_data(
            index,
            multi_drives,
            single_drive,
            &filters,
            needs_paths,
            profile,
            no_bitmap,
        )
        .await?
    };

    // Compute tree columns only if specifically requested (skip in benchmark mode)
    // Note: Tree columns are already included when using MftIndex path (via
    // results_to_dataframe) This code only runs for DataFrame path (parquet
    // index or --force-dataframe)
    let t_tree = std::time::Instant::now();
    if !benchmark && output_config.needs_tree_columns() {
        let tree_cols = output_config.get_tree_columns();
        // Check if tree columns already exist (from MftIndex path)
        let missing_cols: Vec<_> = tree_cols
            .iter()
            .filter(|col| results.column(col.column_name()).is_err())
            .copied()
            .collect();

        if !missing_cols.is_empty() {
            info!(columns = missing_cols.len(), "Computing tree metrics");
            results = add_tree_columns(&results, &missing_cols)
                .context("Failed to compute tree columns")?;
        }
    }
    let tree_ms = t_tree.elapsed().as_millis();

    // Output results (skip in benchmark mode)
    let t_output = std::time::Instant::now();
    if !benchmark {
        write_results(&results, format, out, &output_config, &footer_drives)?;
    }
    let output_ms = t_output.elapsed().as_millis();

    // Print timing (C++ compatibility: "Finished in X s")
    let elapsed = start_time.elapsed();

    if benchmark {
        // Benchmark mode: print summary without output overhead
        let row_count = results.height();
        let total_ms = elapsed.as_millis();
        let secs = elapsed.as_secs_f64();
        eprintln!("=== BENCHMARK MODE (no output) ===");
        eprintln!("  Records found:   {row_count:>10}");
        eprintln!("  Total time:      {total_ms:>10} ms ({secs:.2} s)");
        // Throughput calculation intentionally uses floating-point
        #[expect(
            clippy::cast_precision_loss,
            reason = "row_count as f64 is fine for display-only throughput"
        )]
        #[expect(
            clippy::float_arithmetic,
            reason = "throughput calculation for human-readable benchmark output"
        )]
        let throughput = row_count as f64 / secs;
        eprintln!("  Throughput:      {throughput:>10.0} records/sec");
    } else if profile {
        let row_count = results.height();
        let total_ms = elapsed.as_millis();
        eprintln!("=== PROFILE: Output ===");
        eprintln!("  Tree columns:    {tree_ms:>6} ms");
        eprintln!("  Output/write:    {output_ms:>6} ms  ({row_count} rows)");
        eprintln!("=== TOTAL: {total_ms} ms ===");
    }

    // C++ compatibility: Print "Drives?" and "MMMmmm" to stdout AFTER the data
    // These go to stdout (not stderr) to match C++ behavior
    let secs = elapsed.as_secs();

    #[cfg(windows)]
    #[expect(
        clippy::print_stdout,
        reason = "C++ compatibility requires stdout output"
    )]
    if !drives_to_search.is_empty() {
        let drive_list: String = drives_to_search
            .iter()
            .map(|d| format!("{d}:"))
            .collect::<Vec<_>>()
            .join("|");
        // C++ format: "\nDrives? \t{count}\t{drive_list}\n\n"
        println!("\nDrives? \t{}\t{drive_list}\n", drives_to_search.len());
    }

    // C++ compatibility: "MMMmmm that was FAST" message when elapsed <= 1 second
    // (to stdout)
    #[expect(
        clippy::print_stdout,
        reason = "C++ compatibility requires stdout output"
    )]
    if secs <= 1 {
        println!(
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t{pattern}\n\
             Search path. E.g. 'C:/' or 'C:\\Prog**' "
        );
    }

    // C++ compatibility: "Finished in X s" (to stderr)
    eprintln!("\nFinished \tin {secs} s\n");

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
    output_config: &OutputConfig,
    no_bitmap: bool,
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
        search_multi_drive_streaming(&drives, filters, format, stdout, output_config, no_bitmap)
            .await
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let writer = BufWriter::new(file);
        search_multi_drive_streaming(&drives, filters, format, writer, output_config, no_bitmap)
            .await?;
        info!(file = out, "Results written to file");
        Ok(())
    }
}

/// Load and filter search data from a raw MFT file (cross-platform debugging).
///
/// This function loads a previously saved raw MFT file and processes it
/// exactly like a live MFT read, enabling debugging on any platform.
/// Same pipeline as Windows live read - only the load source differs.
#[expect(clippy::single_call_fn, reason = "extracted from search() for clarity")]
fn load_and_filter_from_mft_file(
    mft_path: &Path,
    drive_letter: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    debug_tree: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_mft::{LoadRawOptions, MftReader};

    let volume = drive_letter.unwrap_or('X');
    info!(volume = %volume, path = %mft_path.display(), "Loading raw MFT file");

    // Load raw MFT into MftIndex (same as live read, just from file)
    let t_load = std::time::Instant::now();
    let options = LoadRawOptions {
        volume_letter: Some(volume),
        ..Default::default()
    };

    // If debug_tree is enabled, use the debug loading path
    let index = if debug_tree {
        load_raw_mft_with_debug(mft_path, &options)?
    } else {
        MftReader::load_raw_to_index_with_options(mft_path, &options)
            .with_context(|| format!("Failed to load raw MFT: {}", mft_path.display()))?
    };
    let load_ms = t_load.elapsed().as_millis();

    // Execute query on index (same as Windows live path)
    let t_query = std::time::Instant::now();
    let results = execute_index_query(&index, filters, needs_paths)?;
    let query_ms = t_query.elapsed().as_millis();

    if profile {
        let total_ms = load_ms + query_ms;
        eprintln!("=== RAW MFT FILE TIMING ===");
        eprintln!(
            "  Load from file:  {load_ms:>6} ms  ({} records)",
            index.len()
        );
        eprintln!(
            "  Query/filter:    {query_ms:>6} ms  ({} matches)",
            results.height()
        );
        eprintln!("  TOTAL:           {total_ms:>6} ms");
    }

    Ok(results)
}

/// Load raw MFT with debug output for tree metrics.
#[expect(
    clippy::cast_possible_truncation,
    reason = "name count and shown counter values are small enough for u16/u32"
)]
#[expect(
    clippy::print_stdout,
    reason = "intentional debug output for tree metrics investigation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "extracted for debug-specific MFT loading path"
)]
fn load_raw_mft_with_debug(
    mft_path: &Path,
    options: &uffs_mft::LoadRawOptions,
) -> Result<uffs_mft::MftIndex> {
    use uffs_mft::MftIndex;
    use uffs_mft::parse::{
        ParseOptions, ParseResult, apply_fixup, parse_record, parse_record_forensic,
    };

    println!("=== LOADING RAW MFT WITH DEBUG ===");
    println!("Path: {}", mft_path.display());

    let raw = uffs_mft::raw::load_raw_mft(mft_path, options)?;
    println!("Raw MFT loaded: {} records", raw.header.record_count);

    // Parse all records into ParsedRecord format
    let capacity = usize::try_from(raw.header.record_count).unwrap_or(0);
    let mut parsed_records = Vec::with_capacity(capacity);

    let parse_options = if options.forensic {
        ParseOptions::FORENSIC
    } else {
        ParseOptions::DEFAULT
    };

    let mut hardlink_count = 0_usize;
    let mut max_name_count = 0_u16;

    for (frs, record_data) in raw.iter_records() {
        let mut record_buf = record_data.to_vec();
        let fixup_ok = apply_fixup(&mut record_buf);

        if options.forensic {
            let result = parse_record_forensic(&record_buf, frs, &parse_options, !fixup_ok);
            if let ParseResult::Base(parsed) = result {
                if parsed.names.len() > 1 {
                    hardlink_count += 1;
                    max_name_count = max_name_count.max(parsed.names.len() as u16);
                }
                parsed_records.push(parsed);
            }
        } else {
            if !fixup_ok {
                continue;
            }
            if let Some(parsed) = parse_record(&record_buf, frs) {
                if parsed.names.len() > 1 {
                    hardlink_count += 1;
                    max_name_count = max_name_count.max(parsed.names.len() as u16);
                }
                parsed_records.push(parsed);
            }
        }
    }

    println!("Parsed {} records", parsed_records.len());
    println!("Records with multiple names (hardlinks): {hardlink_count}");
    println!("Max name_count: {max_name_count}");

    // Show sample hardlinks
    println!();
    println!("=== SAMPLE HARDLINKS (first 10) ===");
    let mut shown = 0_u32;
    for parsed in &parsed_records {
        if parsed.names.len() > 1 && shown < 10_u32 {
            println!(
                "  FRS {}: name_count={}, size={}",
                parsed.frs,
                parsed.names.len(),
                parsed.size
            );
            for (idx, name) in parsed.names.iter().enumerate() {
                println!(
                    "    [{idx}] parent_frs={}, name={}",
                    name.parent_frs, name.name
                );
            }
            shown += 1_u32;
        }
    }

    // Build MftIndex (this computes tree metrics normally)
    println!();
    println!("Building MftIndex...");
    let mut index = MftIndex::from_parsed_records(raw.header.volume_letter, parsed_records);

    println!(
        "Index built: {} records, {} children entries",
        index.len(),
        index.children_count()
    );

    // Recompute tree metrics with debug output
    index.compute_tree_metrics_debug();

    Ok(index)
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
/// * `profile` - If true, prints detailed timing breakdown to stderr.
/// * `no_bitmap` - If true, disables MFT bitmap optimization (reads all
///   records).
#[expect(
    clippy::single_call_fn,
    reason = "extracted from search() to reduce line count"
)]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
async fn load_and_filter_data(
    index: Option<PathBuf>,
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_bitmap: bool,
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
        return search_multi_drive_filtered(&drives, filters, needs_paths, no_bitmap).await;
    }

    // Check for single drive: CLI flag overrides pattern-embedded drive
    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    if let Some(drive_letter) = effective_drive {
        // Single drive search with proper path resolution
        let t_read = std::time::Instant::now();
        tracing::trace!(drive = %drive_letter, "search_dataframe: before load_or_build_dataframe_cached");

        // Use cached DataFrame path for performance (Windows only)
        #[cfg(windows)]
        let full_df =
            uffs_mft::load_or_build_dataframe_cached(drive_letter, uffs_mft::INDEX_TTL_SECONDS)
                .await
                .with_context(|| format!("Failed to read MFT for drive {drive_letter}:"))?;

        tracing::trace!(drive = %drive_letter, "search_dataframe: after load_or_build_dataframe_cached");

        // Non-Windows: read directly (no caching)
        // Note: MftReader::open returns error on non-Windows (PlatformNotSupported)
        #[cfg(not(windows))]
        let full_df = {
            let reader = MftReader::open(drive_letter)
                .with_context(|| format!("Failed to open drive {drive_letter}:"))?
                .with_use_bitmap(!no_bitmap);
            reader.read_all()?
        };

        let read_ms = t_read.elapsed().as_millis();
        let total_records = full_df.height();

        // Build path resolver from FULL data BEFORE filtering
        let t_resolver = std::time::Instant::now();
        let path_resolver = if needs_paths {
            Some(
                uffs_core::FastPathResolver::build(&full_df, drive_letter)
                    .context("Failed to build path resolver")?,
            )
        } else {
            None
        };
        let resolver_ms = t_resolver.elapsed().as_millis();

        // Apply filters
        let t_filter = std::time::Instant::now();
        let mut filtered = execute_query(full_df, filters)?;
        let filter_ms = t_filter.elapsed().as_millis();
        let filtered_count = filtered.height();

        // Add paths using the pre-built resolver with directory suffix (C++ parity)
        let t_paths = std::time::Instant::now();
        if let Some(resolver) = &path_resolver {
            filtered = resolver
                .add_path_column_with_dir_suffix(&filtered)
                .context("Failed to add path column")?;
            // Add path_only column (directory portion of path)
            filtered = uffs_core::add_path_only_column(&filtered)
                .context("Failed to add path_only column")?;
        }
        let paths_ms = t_paths.elapsed().as_millis();

        if profile {
            let total_ms = read_ms + resolver_ms + filter_ms + paths_ms;
            eprintln!("=== PROFILE: Drive {drive_letter}: ===");
            eprintln!("  MFT read (cached): {read_ms:>6} ms  ({total_records} records)");
            eprintln!("  Path resolver:     {resolver_ms:>6} ms");
            eprintln!("  Query/filter:      {filter_ms:>6} ms  ({filtered_count} matches)");
            eprintln!("  Path resolution:   {paths_ms:>6} ms");
            eprintln!("  TOTAL:             {total_ms:>6} ms");
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
        search_multi_drive_filtered(&all_drives, filters, needs_paths, no_bitmap).await
    }
    #[cfg(not(windows))]
    {
        bail!(
            "No drive specified. Use --drive, --drives, --index, or include drive in pattern (e.g., c:/pro*)"
        )
    }
}

/// Load and filter data using fast `MftIndex` path (no `DataFrame` conversion
/// during search).
///
/// This is the fast path for simple queries. Uses cached `MftIndex` when
/// available (unless `no_cache` is true).
#[cfg(windows)]
#[expect(clippy::single_call_fn, reason = "extracted from search() for clarity")]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
async fn load_and_filter_data_index(
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_cache: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_mft::INDEX_TTL_SECONDS;

    // Get effective drive
    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    let drive_letter = effective_drive.ok_or_else(|| {
        anyhow::anyhow!(
            "Index query mode requires a specific drive. Use --drive or include drive in pattern."
        )
    })?;

    let t_load = std::time::Instant::now();

    let reader = MftReader::open(drive_letter)
        .with_context(|| format!("Failed to open drive {drive_letter}:"))?;

    // Use cached read by default, fresh read if --no-cache
    let index = if no_cache {
        info!(drive = %drive_letter, "🔄 --no-cache: reading MFT fresh");
        reader.read_all_index().await?
    } else {
        reader.read_index_cached(INDEX_TTL_SECONDS).await?
    };
    let load_ms = t_load.elapsed().as_millis();

    // Execute query on index
    let t_query = std::time::Instant::now();
    let results = execute_index_query(&index, filters, needs_paths)?;
    let query_ms = t_query.elapsed().as_millis();

    if profile {
        let total_ms = load_ms + query_ms;
        eprintln!("=== PROFILE: Drive {drive_letter} (Index Path): ===");
        eprintln!(
            "  Index load:      {load_ms:>6} ms  ({} records)",
            index.len()
        );
        eprintln!(
            "  Query/filter:    {query_ms:>6} ms  ({} matches)",
            results.height()
        );
        eprintln!("  TOTAL:           {total_ms:>6} ms");
    }

    Ok(results)
}

/// Load and filter data using fast `MftIndex` path for multiple drives.
///
/// Searches each drive in parallel using cached indices (unless `no_cache` is
/// true), then combines results.
#[cfg(windows)]
#[expect(
    clippy::single_call_fn,
    reason = "extracted for multi-drive parallel search"
)]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
async fn load_and_filter_data_index_multi(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_cache: bool,
) -> Result<uffs_mft::DataFrame> {
    use std::sync::Arc;

    use tokio::task::JoinSet;
    use uffs_mft::INDEX_TTL_SECONDS;

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    info!(
        count = drives.len(),
        drives = ?drives,
        no_cache,
        "Searching drives in PARALLEL"
    );

    let t_total = std::time::Instant::now();

    // Create owned filters for async tasks
    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));

    // Spawn tasks for each drive
    let mut join_set: JoinSet<Result<(char, uffs_mft::DataFrame, u128, u128, usize)>> =
        JoinSet::new();

    for &drive in drives {
        let filters = Arc::clone(&owned_filters);

        join_set.spawn(async move {
            let t_load = std::time::Instant::now();

            let reader = MftReader::open(drive)
                .with_context(|| format!("Failed to open drive {drive}:"))?;

            // Use cached read by default, fresh read if --no-cache
            let index = if no_cache {
                info!(drive = %drive, "🔄 --no-cache: reading MFT fresh");
                reader.read_all_index().await?
            } else {
                reader.read_index_cached(INDEX_TTL_SECONDS).await?
            };
            let load_ms = t_load.elapsed().as_millis();
            let record_count = index.len();

            // Execute query on index
            let t_query = std::time::Instant::now();
            let borrowed_filters = QueryFilters {
                parsed: &filters.parsed,
                ext_filter: filters.ext_filter.as_deref(),
                files_only: filters.files_only,
                dirs_only: filters.dirs_only,
                hide_system: filters.hide_system,
                min_size: filters.min_size,
                max_size: filters.max_size,
                limit: filters.limit,
            };
            let results = execute_index_query(&index, &borrowed_filters, needs_paths)?;
            let query_ms = t_query.elapsed().as_millis();

            Ok((drive, results, load_ms, query_ms, record_count))
        });
    }

    // Collect results from all drives
    let mut all_results: Vec<uffs_mft::DataFrame> = Vec::with_capacity(drives.len());
    let mut total_records = 0usize;
    let mut total_matches = 0usize;

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok((drive, df, load_ms, query_ms, record_count))) => {
                let matches = df.height();
                if profile {
                    eprintln!(
                        "  Drive {drive}: {record_count} records, {matches} matches (load: {load_ms}ms, query: {query_ms}ms)"
                    );
                }
                total_records += record_count;
                total_matches += matches;
                if matches > 0 {
                    all_results.push(df);
                }
            }
            Ok(Err(e)) => {
                info!(error = %e, "Drive search failed (continuing with other drives)");
            }
            Err(e) => {
                info!(error = %e, "Task join error (continuing with other drives)");
            }
        }
    }

    let total_ms = t_total.elapsed().as_millis();

    if profile {
        eprintln!("=== PROFILE: Multi-drive Index Path ===");
        eprintln!("  Drives:          {:>6}", drives.len());
        eprintln!("  Total records:   {total_records:>6}");
        eprintln!("  Total matches:   {total_matches:>6}");
        eprintln!("  TOTAL time:      {total_ms:>6} ms");
    }

    // Combine all results
    if all_results.is_empty() {
        // Return empty DataFrame with correct schema
        return Ok(uffs_mft::DataFrame::empty());
    }

    if all_results.len() == 1 {
        return Ok(all_results.remove(0));
    }

    // Vertical concatenation of all DataFrames
    // Convert DataFrames to LazyFrames for concat, then collect back
    use uffs_polars::IntoLazy;
    let lazy_frames: Vec<uffs_polars::LazyFrame> =
        all_results.into_iter().map(|df| df.lazy()).collect();
    let combined = uffs_polars::concat(&lazy_frames, uffs_polars::UnionArgs::default())
        .context("Failed to combine results from multiple drives")?
        .collect()
        .context("Failed to collect combined results")?;

    // Apply limit if specified
    let final_result = if filters.limit > 0 && combined.height() > filters.limit as usize {
        combined.head(Some(filters.limit as usize))
    } else {
        combined
    };

    info!(
        total_matches = final_result.height(),
        drives = drives.len(),
        "Multi-drive cached index search complete"
    );

    Ok(final_result)
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
    /// Hide system files (files starting with $).
    hide_system: bool,
    /// Minimum file size filter.
    min_size: Option<u64>,
    /// Maximum file size filter.
    max_size: Option<u64>,
    /// Maximum number of results to return.
    limit: u32,
}

/// Build and execute the MFT query with all filters applied.
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

    // Apply hide_system filter (exclude $-prefixed files AND FRS < 16 metadata)
    if filters.hide_system {
        query = query.hide_system();
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

/// Execute query using fast `IndexQuery` path (no `DataFrame` conversion).
///
/// This is the fast path for simple queries. Returns results as a `DataFrame`
/// for compatibility with the output pipeline.
fn execute_index_query(
    index: &uffs_mft::MftIndex,
    filters: &QueryFilters<'_>,
    resolve_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_core::{IndexQuery, TypeFilter, compile_parsed_pattern};

    let mut query = IndexQuery::new(index);

    // Apply pattern filter
    let pattern = compile_parsed_pattern(filters.parsed);
    query = query.with_pattern_result(pattern);

    // Apply extension filter if specified (extensions are handled via pattern)
    if let Some(ext_str) = filters.ext_filter {
        let parsed_ext_filter = ExtensionFilter::parse(ext_str)
            .map_err(|err| anyhow::anyhow!("Invalid extension filter: {err}"))?;
        let exts: Vec<&str> = parsed_ext_filter
            .extensions()
            .iter()
            .map(String::as_str)
            .collect();
        query = query.extensions(&exts);
    }

    // Apply type filters
    if filters.files_only {
        query = query.with_type_filter(TypeFilter::FilesOnly);
    } else if filters.dirs_only {
        query = query.with_type_filter(TypeFilter::DirsOnly);
    }

    // Apply size filters
    if let Some(min) = filters.min_size {
        query = query.min_size(min);
    }
    if let Some(max) = filters.max_size {
        query = query.max_size(max);
    }

    // Apply limit (0 = unlimited)
    if filters.limit > 0 {
        query = query.limit(filters.limit as usize);
    }

    // Apply case sensitivity
    query = query.case_sensitive(filters.parsed.is_case_sensitive());

    // Apply path resolution
    query = query.with_resolve_paths(resolve_paths);

    // Execute and convert to DataFrame
    let results = query.collect();
    results_to_dataframe(index, &results, resolve_paths)
}

/// Convert `IndexQuery` results to a `DataFrame` for output compatibility.
///
/// **TEMPORARY**: This function exists only for compatibility with the current
/// output pipeline which expects a `DataFrame`. The proper solution is to
/// output directly from `SearchResults` without `DataFrame` conversion.
///
/// TODO: Remove this function and output directly from `SearchResults` +
/// `MftIndex`.
// TEMPORARY: print_stderr for debugging nested tokio runtime panic (issue #XXX)
#[expect(
    clippy::single_call_fn,
    reason = "temporary conversion layer — will be removed when output pipeline supports SearchResults directly"
)]
#[expect(
    clippy::too_many_lines,
    reason = "builds full C++ parity schema with 30+ columns"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short names (e.g. df) conventional in DataFrame-heavy code"
)]
#[expect(
    clippy::option_if_let_else,
    reason = "if-let chains are clearer for record lookup fallback"
)]
fn results_to_dataframe(
    index: &uffs_mft::MftIndex,
    results: &[uffs_core::SearchResult],
    _resolve_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

    let height = results.len();

    // Build ALL columns from results + MftIndex for C++ parity
    let mut frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut parent_frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut names: Vec<String> = Vec::with_capacity(height);
    let mut file_types: Vec<String> = Vec::with_capacity(height);
    let mut paths: Vec<String> = Vec::with_capacity(height);
    let mut sizes: Vec<u64> = Vec::with_capacity(height);
    let mut allocated_sizes: Vec<u64> = Vec::with_capacity(height);
    let mut created_times: Vec<i64> = Vec::with_capacity(height);
    let mut modified_times: Vec<i64> = Vec::with_capacity(height);
    let mut accessed_times: Vec<i64> = Vec::with_capacity(height);
    let mut mft_changed_times: Vec<i64> = Vec::with_capacity(height);
    let mut is_dirs: Vec<bool> = Vec::with_capacity(height);
    let mut is_readonly: Vec<bool> = Vec::with_capacity(height);
    let mut is_hidden: Vec<bool> = Vec::with_capacity(height);
    let mut is_system: Vec<bool> = Vec::with_capacity(height);
    let mut is_archive: Vec<bool> = Vec::with_capacity(height);
    let mut is_compressed: Vec<bool> = Vec::with_capacity(height);
    let mut is_encrypted: Vec<bool> = Vec::with_capacity(height);
    let mut is_sparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_reparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_offline: Vec<bool> = Vec::with_capacity(height);
    let mut is_not_indexed: Vec<bool> = Vec::with_capacity(height);
    let mut is_temporary: Vec<bool> = Vec::with_capacity(height);
    let mut is_integrity: Vec<bool> = Vec::with_capacity(height);
    let mut is_no_scrub: Vec<bool> = Vec::with_capacity(height);
    let mut is_pinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_unpinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_virtual: Vec<bool> = Vec::with_capacity(height);
    let mut flags_values: Vec<u32> = Vec::with_capacity(height);

    // Tree metrics (pre-computed in MftIndex)
    let mut descendants_values: Vec<u32> = Vec::with_capacity(height);
    let mut treesize_values: Vec<u64> = Vec::with_capacity(height);
    let mut tree_allocated_values: Vec<u64> = Vec::with_capacity(height);
    // Stream name column (for ADS detection in apply_directory_treesize)
    let mut stream_names: Vec<String> = Vec::with_capacity(height);

    for result in results {
        // Look up the full record from the index to get all attributes
        let record = index.find(result.frs);

        frs_values.push(result.frs);
        parent_frs_values.push(result.parent_frs);
        names.push(result.name.clone());
        paths.push(result.path.clone().unwrap_or_default());
        sizes.push(result.size);
        stream_names.push(result.stream_name.clone());

        // File type (extension) - lookup from index's ExtensionTable or extract from
        // name
        let file_type = if let Some(rec) = record {
            let ext_id = rec.first_name.name.extension_id();
            index
                .extensions
                .get_extension(ext_id)
                .unwrap_or("")
                .to_owned()
        } else {
            // Fallback: extract extension from name
            result
                .name
                .rfind('.')
                .and_then(|pos| {
                    if pos > 0 && pos < result.name.len() - 1 {
                        result.name.get(pos + 1..)
                    } else {
                        None
                    }
                })
                .map(str::to_lowercase)
                .unwrap_or_default()
        };
        file_types.push(file_type);

        if let Some(rec) = record {
            // Populate from record's StandardInfo
            // Use allocated_size from SearchResult (populated from stream's
            // SizeInfo.allocated)
            allocated_sizes.push(result.allocated_size);
            created_times.push(rec.stdinfo.created);
            modified_times.push(rec.stdinfo.modified);
            accessed_times.push(rec.stdinfo.accessed);
            mft_changed_times.push(rec.stdinfo.mft_changed);
            is_dirs.push(rec.is_directory());
            is_readonly.push(rec.stdinfo.is_readonly());
            is_hidden.push(rec.stdinfo.is_hidden());
            is_system.push(rec.stdinfo.is_system());
            is_archive.push(rec.stdinfo.is_archive());
            is_compressed.push(rec.stdinfo.is_compressed());
            is_encrypted.push(rec.stdinfo.is_encrypted());
            is_sparse.push(rec.stdinfo.is_sparse());
            is_reparse.push(rec.stdinfo.is_reparse());
            is_offline.push(rec.stdinfo.is_offline());
            is_not_indexed.push(rec.stdinfo.is_not_indexed());
            is_temporary.push(rec.stdinfo.is_temporary());
            is_integrity.push(rec.stdinfo.is_integrity_stream());
            is_no_scrub.push(rec.stdinfo.is_no_scrub_data());
            is_pinned.push(rec.stdinfo.is_pinned());
            is_unpinned.push(rec.stdinfo.is_unpinned());
            is_virtual.push(rec.stdinfo.is_virtual());
            // Convert internal flags back to Windows FILE_ATTRIBUTE_* format for C++ parity
            flags_values.push(rec.stdinfo.to_attributes());
        } else {
            // Record not found - use defaults
            allocated_sizes.push(0);
            created_times.push(0);
            modified_times.push(0);
            accessed_times.push(0);
            mft_changed_times.push(0);
            is_dirs.push(result.is_directory);
            is_readonly.push(false);
            is_hidden.push(false);
            is_system.push(false);
            is_archive.push(false);
            is_compressed.push(false);
            is_encrypted.push(false);
            is_sparse.push(false);
            is_reparse.push(false);
            is_offline.push(false);
            is_not_indexed.push(false);
            is_temporary.push(false);
            is_integrity.push(false);
            is_no_scrub.push(false);
            is_pinned.push(false);
            is_unpinned.push(false);
            is_virtual.push(false);
            flags_values.push(0);
        }

        // Tree metrics handling with C++ parity fixes (Fix #1, #2, #3):
        // - Fix #1: Root row (FRS=5) must use the record's tree metrics
        //   (authoritative).
        // - Fix #2: Reparse directories (junctions/symlinks) must NOT be "file-ified" -
        //   they are still directories and should use computed tree metrics. C++ treats
        //   junctions as directory leaves with Desc=1, not files with Desc=0.
        // - Fix #3: Use the same tree_metrics() method as OFFLINE path for consistency.
        //
        // For directories (including reparse), always prefer the record's tree metrics
        // to ensure we get the computed values from cpp_tree, not potentially stale
        // values from SearchResult.
        // C++ parity: ADS entries (stream_index > 0) have
        // descendants/treesize/tree_allocated = 0. Only the default stream
        // (stream_index == 0) gets tree metrics.
        let (desc, tsize, talloc) = if result.stream_index > 0 {
            // ADS stream: no tree metrics
            (0_u32, 0_u64, 0_u64)
        } else if let Some(rec) = record {
            // Use tree_metrics() as the single source of truth (Fix #3)
            // This method returns the correct values for both directories and files,
            // ensuring LIVE and OFFLINE paths produce identical output.
            rec.tree_metrics()
        } else {
            // No record found - fall back to SearchResult values
            (result.descendants, result.treesize, result.tree_allocated)
        };
        descendants_values.push(desc);
        treesize_values.push(tsize);
        tree_allocated_values.push(talloc);
    }

    // Create DataFrame with full schema matching MftIndex::to_dataframe()
    let columns = vec![
        Series::new("frs".into(), frs_values).into_column(),
        Series::new("parent_frs".into(), parent_frs_values).into_column(),
        Series::new("name".into(), names).into_column(),
        Series::new("type".into(), file_types).into_column(),
        Series::new("path".into(), paths).into_column(),
        Series::new("size".into(), sizes).into_column(),
        Series::new("allocated_size".into(), allocated_sizes).into_column(),
        Series::new("created".into(), created_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast created column: {e}"))?
            .into_column(),
        Series::new("modified".into(), modified_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast modified column: {e}"))?
            .into_column(),
        Series::new("accessed".into(), accessed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast accessed column: {e}"))?
            .into_column(),
        Series::new("mft_changed".into(), mft_changed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast mft_changed column: {e}"))?
            .into_column(),
        Series::new("is_directory".into(), is_dirs).into_column(),
        Series::new("is_readonly".into(), is_readonly).into_column(),
        Series::new("is_hidden".into(), is_hidden).into_column(),
        Series::new("is_system".into(), is_system).into_column(),
        Series::new("is_archive".into(), is_archive).into_column(),
        Series::new("is_compressed".into(), is_compressed).into_column(),
        Series::new("is_encrypted".into(), is_encrypted).into_column(),
        Series::new("is_sparse".into(), is_sparse).into_column(),
        Series::new("is_reparse".into(), is_reparse).into_column(),
        Series::new("is_offline".into(), is_offline).into_column(),
        Series::new("is_not_indexed".into(), is_not_indexed).into_column(),
        Series::new("is_temporary".into(), is_temporary).into_column(),
        Series::new("is_integrity_stream".into(), is_integrity).into_column(),
        Series::new("is_no_scrub_data".into(), is_no_scrub).into_column(),
        Series::new("is_pinned".into(), is_pinned).into_column(),
        Series::new("is_unpinned".into(), is_unpinned).into_column(),
        Series::new("is_virtual".into(), is_virtual).into_column(),
        Series::new("flags".into(), flags_values).into_column(),
        // Tree metrics (pre-computed in MftIndex, no need to recompute!)
        Series::new("descendants".into(), descendants_values).into_column(),
        Series::new("treesize".into(), treesize_values).into_column(),
        Series::new("tree_allocated".into(), tree_allocated_values).into_column(),
        // Stream name (for ADS detection in apply_directory_treesize)
        Series::new("stream_name".into(), stream_names).into_column(),
    ];

    let mut df = uffs_mft::DataFrame::new_infer_height(columns)
        .map_err(|err| anyhow::anyhow!("Failed to create DataFrame: {err}"))?;

    // Tree metrics are already computed in MftIndex and included in the columns
    // above! No need to recompute them here - this was the missed optimization.

    // Replace size and allocated_size columns with tree metrics for directories
    // (C++ parity) For directories: size = treesize, allocated_size =
    // tree_allocated For files: keep original size and allocated_size
    //
    // NOTE: apply_directory_treesize uses polars .lazy().collect() which with
    // the new_streaming feature triggers tokio internally. We use block_in_place
    // to allow this blocking operation within the async context.
    df = tokio::task::block_in_place(|| uffs_core::apply_directory_treesize(&df))
        .map_err(|err| anyhow::anyhow!("Failed to apply directory treesize: {err}"))?;

    // Add path_only column (directory portion of path)
    df = uffs_core::add_path_only_column(&df)
        .map_err(|err| anyhow::anyhow!("Failed to add path_only column: {err}"))?;

    Ok(df)
}

/// Write search results to console or file.
///
/// When `drives` is non-empty, appends a C++ compatible footer after the data:
/// `\r\n\r\nDrives? \t{count}\t{drive_list}\r\n\r\n`
#[expect(
    clippy::single_call_fn,
    reason = "extracted to reduce search() line count below clippy::too_many_lines limit"
)]
fn write_results(
    results: &uffs_mft::DataFrame,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    drives: &[char],
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
        let mut writer = BufWriter::new(file);

        match format {
            "json" => export_json(results, &mut writer)?,
            "csv" => export_csv(results, &mut writer)?,
            _ => output_config.write(results, &mut writer)?,
        }

        // Append C++ compatible footer: "Drives?" line with CRLF line endings
        if !drives.is_empty() {
            let drive_list: String = drives
                .iter()
                .map(|drive| format!("{drive}:"))
                .collect::<Vec<_>>()
                .join("|");
            write!(
                writer,
                "\r\n\r\nDrives? \t{}\t{drive_list}\r\n\r\n",
                drives.len()
            )?;
            writer.flush()?;
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
    /// Hide system files (files starting with $).
    hide_system: bool,
    /// Minimum file size filter.
    min_size: Option<u64>,
    /// Maximum file size filter.
    max_size: Option<u64>,
    /// Maximum number of results to return.
    limit: u32,
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
            hide_system: filters.hide_system,
            min_size: filters.min_size,
            max_size: filters.max_size,
            limit: filters.limit,
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

        // Apply hide_system filter (exclude $-prefixed files AND FRS < 16 metadata)
        if self.hide_system {
            query = query.hide_system();
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
///
/// # Arguments
///
/// * `no_bitmap` - If true, disables MFT bitmap optimization (reads all
///   records).
#[cfg(windows)]
async fn search_multi_drive_filtered(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    no_bitmap: bool,
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
        let use_bitmap = !no_bitmap; // Capture for the spawned task

        tokio::spawn(async move {
            let pb = pbs.as_ref().and_then(|p| p.get(&drive_char));

            // Use cached DataFrame path for performance
            // Progress bar will complete quickly on cache hit (which is good!)
            let full_df =
                uffs_mft::load_or_build_dataframe_cached(drive_char, uffs_mft::INDEX_TTL_SECONDS)
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

            // Suppress unused variable warning
            let _ = use_bitmap;

            let records_read = full_df.height();
            if let Some(p) = pb {
                p.finish();
            }

            // Build path resolver from FULL data BEFORE filtering
            // This is the key fix for the <unknown> path bug!
            let path_resolver = if needs_paths {
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

            // Add paths using the pre-built resolver with directory suffix (C++ parity)
            let with_paths = if let Some(resolver) = &path_resolver {
                match resolver.add_path_column_with_dir_suffix(&filtered) {
                    Ok(df) => {
                        // Add path_only column (directory portion of path)
                        match uffs_core::add_path_only_column(&df) {
                            Ok(df_with_path_only) => {
                                // Apply treesize transformation for directories (C++ parity)
                                match uffs_core::apply_directory_treesize(&df_with_path_only) {
                                    Ok(df_with_treesize) => df_with_treesize,
                                    Err(e) => {
                                        let _ = tx
                                            .send(DriveResult {
                                                drive: drive_char,
                                                df: None,
                                                records_read,
                                                matches,
                                                error: Some(format!(
                                                    "Failed to apply treesize: {e}"
                                                )),
                                                paths_resolved: false,
                                            })
                                            .await;
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(DriveResult {
                                        drive: drive_char,
                                        df: None,
                                        records_read,
                                        matches,
                                        error: Some(format!("Failed to add path_only: {e}")),
                                        paths_resolved: false,
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
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
                // No path resolver - still apply treesize transformation
                match uffs_core::apply_directory_treesize(&filtered) {
                    Ok(df) => df,
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(format!("Failed to apply treesize: {e}")),
                                paths_resolved: false,
                            })
                            .await;
                        return;
                    }
                }
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
            paths_resolved = result.paths_resolved,
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
#[expect(
    clippy::unused_async,
    reason = "must match async signature of Windows implementation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "platform stub — matches Windows counterpart"
)]
async fn search_multi_drive_filtered(
    _drives: &[char],
    _filters: &QueryFilters<'_>,
    _needs_paths: bool,
    _no_bitmap: bool,
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
    output_config: &OutputConfig,
    no_bitmap: bool,
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
    let streaming_writer = Arc::new(StreamingWriter::new(
        writer,
        format,
        filters.limit,
        output_config.clone(),
    ));

    // Channel for receiving results from drive tasks
    let (tx, mut rx) = mpsc::channel::<DriveResult>(drives.len());

    // Spawn all drive reads concurrently
    for &drive_char in drives {
        let tx = tx.clone();
        let filters = Arc::clone(&owned_filters);
        let use_bitmap = !no_bitmap; // Capture for the spawned task

        tokio::spawn(async move {
            // Use cached DataFrame path for performance
            let df =
                uffs_mft::load_or_build_dataframe_cached(drive_char, uffs_mft::INDEX_TTL_SECONDS)
                    .await;

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

            // Suppress unused variable warning
            let _ = use_bitmap;

            let records_read = df.height();

            // Build path resolver from FULL data BEFORE filtering
            // This is critical for resolving paths correctly!
            let path_resolver = match uffs_core::FastPathResolver::build(&df, drive_char) {
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
            };

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

            // Add paths using the pre-built resolver with directory suffix (C++ parity)
            let with_paths = if let Some(resolver) = &path_resolver {
                match resolver.add_path_column_with_dir_suffix(&filtered) {
                    Ok(df) => {
                        // Add path_only column (directory portion of path)
                        match uffs_core::add_path_only_column(&df) {
                            Ok(df_with_path_only) => {
                                // Apply treesize transformation for directories (C++ parity)
                                match uffs_core::apply_directory_treesize(&df_with_path_only) {
                                    Ok(df_with_treesize) => df_with_treesize,
                                    Err(e) => {
                                        let _ = tx
                                            .send(DriveResult {
                                                drive: drive_char,
                                                df: None,
                                                records_read,
                                                matches,
                                                error: Some(format!(
                                                    "Failed to apply treesize: {e}"
                                                )),
                                                paths_resolved: false,
                                            })
                                            .await;
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(DriveResult {
                                        drive: drive_char,
                                        df: None,
                                        records_read,
                                        matches,
                                        error: Some(format!("Failed to add path_only: {e}")),
                                        paths_resolved: false,
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
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
                // No path resolver - still apply treesize transformation
                match uffs_core::apply_directory_treesize(&filtered) {
                    Ok(df) => df,
                    Err(e) => {
                        let _ = tx
                            .send(DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(format!("Failed to apply treesize: {e}")),
                                paths_resolved: false,
                            })
                            .await;
                        return;
                    }
                }
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
                    paths_resolved: path_resolver.is_some(),
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
#[expect(
    clippy::single_call_fn,
    reason = "public CLI command handler called from main dispatch"
)]
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
                .with_context(|| format!("Failed to open drive {drive_letter}:"))?;

            // Create progress bar (None if disabled via UFFS_NO_PROGRESS=1)
            let progress_disabled = std::env::var("UFFS_NO_PROGRESS")
                .is_ok_and(|val| val == "1" || val.eq_ignore_ascii_case("true"));

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
            let mut df = reader.read_with_progress(move |progress: MftProgress| {
                if let Some(bar) = &progress_bar {
                    if let Some(total) = progress.total_records {
                        bar.set_length(progress.bytes_read.max(total));
                    }
                    bar.set_position(progress.bytes_read);
                }
            })?;

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
#[expect(
    clippy::unused_async,
    reason = "must match async signature of Windows implementation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "platform stub — matches Windows counterpart"
)]
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
#[expect(
    clippy::cast_precision_loss,
    reason = "u64 to f64 is acceptable for human-readable size display"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "division for human-readable size formatting"
)]
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
#[expect(
    clippy::single_call_fn,
    reason = "public CLI command handler called from main dispatch"
)]
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
