//! Search command implementation.

extern crate alloc;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};
use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;

use super::output::{can_write_native_results, write_results};
use super::raw_io::{QueryFilters, load_and_filter_data, load_and_filter_from_mft_file};
#[cfg(windows)]
use super::raw_io::{load_and_filter_data_index, load_and_filter_data_index_multi};

/// Per-drive search helpers shared by multi-drive command paths.
#[cfg(windows)]
mod drive_search;
/// Parallel multi-drive DataFrame helpers.
#[cfg(windows)]
mod multi_drive;
/// Streaming multi-drive helpers.
#[cfg(windows)]
mod streaming;

#[cfg(windows)]
pub(crate) use self::multi_drive::search_multi_drive_filtered;
#[cfg(windows)]
use self::streaming::search_streaming;

/// Maximum number of drive-level CLI search tasks to run concurrently.
#[cfg(any(windows, test))]
pub(super) const MAX_CONCURRENT_SEARCH_DRIVE_TASKS: usize = 4;

/// Returns the bounded drive-level task budget for CLI multi-drive searches.
#[cfg(any(windows, test))]
pub(super) fn search_drive_task_budget(total_drives: usize) -> usize {
    if total_drives == 0 {
        return 0;
    }

    let hardware_budget = std::thread::available_parallelism().map_or(
        MAX_CONCURRENT_SEARCH_DRIVE_TASKS,
        core::num::NonZeroUsize::get,
    );

    total_drives
        .min(hardware_budget.max(1))
        .min(MAX_CONCURRENT_SEARCH_DRIVE_TASKS)
}

/// Result of search dispatch - streaming completed or `DataFrame` needs output.
enum SearchDispatchResult {
    /// Streaming output was written directly - search is complete.
    StreamingComplete,
    /// `DataFrame` results ready for output processing.
    DataFrame(uffs_mft::DataFrame),
}

/// Full search configuration - all parameters needed for any search path.
#[expect(clippy::struct_excessive_bools, reason = "mirrors CLI parameters")]
#[expect(dead_code, reason = "fields used conditionally on Windows")]
struct SearchConfig<'a> {
    /// Search pattern.
    pattern: &'a str,
    /// Single drive letter override.
    single_drive: Option<char>,
    /// Multiple drive letters.
    multi_drives: Option<Vec<char>>,
    /// Index file path.
    index: Option<PathBuf>,
    /// MFT file paths.
    mft_file: Vec<PathBuf>,
    /// Query filters.
    filters: QueryFilters<'a>,
    /// Effective case sensitivity.
    effective_case_sensitive: bool,
    /// Profile mode.
    profile: bool,
    /// Debug tree mode.
    debug_tree: bool,
    /// Benchmark mode (no output).
    benchmark: bool,
    /// Disable bitmap optimization.
    no_bitmap: bool,
    /// Disable cache.
    no_cache: bool,
    /// Attribute filter string.
    attr_filter: Option<&'a str>,
    /// Date filters.
    newer: Option<&'a str>,
    /// Date filters.
    older: Option<&'a str>,
    /// Date filters.
    newer_created: Option<&'a str>,
    /// Date filters.
    older_created: Option<&'a str>,
    /// Date filters.
    newer_accessed: Option<&'a str>,
    /// Date filters.
    older_accessed: Option<&'a str>,
    /// Exclude patterns.
    exclude: Option<&'a str>,
    /// Sort column.
    sort: Option<&'a str>,
    /// Sort descending.
    sort_desc: bool,
    /// Output format.
    format: &'a str,
    /// Output path.
    out: &'a str,
    /// Output configuration.
    output_config: OutputConfig,
    /// Output targets (drive letters).
    output_targets: Vec<char>,
    /// Whether this is a full-scan.
    is_full_scan: bool,
    /// Query mode.
    query_mode: &'a str,
    /// Chaos seed for testing.
    chaos_seed: Option<u64>,
    /// Reserved allocation for size queries.
    reserved_allocated: Option<u64>,
    /// Start time for profiling.
    start_time: std::time::Instant,
}

/// Configuration for multi-file MFT streaming.
struct MultiFileStreamConfig<'a> {
    /// MFT file paths to load.
    mft_files: &'a [PathBuf],
    /// Drive letters for each file.
    drive_letters: Vec<char>,
    /// Compiled pattern for filtering (None = full scan).
    compiled_pattern: Option<uffs_core::index_search::IndexPattern>,
    /// Output format string.
    format: &'a str,
    /// Output target path.
    out: &'a str,
    /// Output configuration.
    output_config: &'a OutputConfig,
    /// Output targets for footer.
    output_targets: &'a [char],
    /// Pattern string for footer.
    pattern: &'a str,
    /// Whether to use case-sensitive matching.
    case_sensitive: bool,
    /// Whether pattern is path-aware.
    is_path_pattern: bool,
    /// Record filter for attribute/date filtering.
    rec_filter: crate::commands::output::StreamingRecordFilter,
    /// Debug tree flag.
    debug_tree: bool,
    /// Chaos seed for testing.
    chaos_seed: Option<u64>,
    /// Reserved allocation for size-aware queries.
    reserved_allocated: Option<u64>,
}

/// Spawn parallel MFT file loaders.
///
/// Returns the loader thread handle and a receiver for loaded indexes.
fn spawn_parallel_loaders(
    file_drive_pairs: Vec<(PathBuf, char)>,
    debug_tree: bool,
    chaos_seed: Option<u64>,
    reserved_allocated: Option<u64>,
) -> (
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Receiver<(char, uffs_mft::MftIndex, u128)>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<(char, uffs_mft::MftIndex, u128)>();

    let handle = std::thread::spawn(move || {
        std::thread::scope(|scope| {
            for (path, drive) in &file_drive_pairs {
                let sender = tx.clone();
                scope.spawn(move || {
                    let result = super::raw_io::load_index_from_mft_file(
                        path,
                        Some(*drive),
                        debug_tree,
                        chaos_seed,
                        reserved_allocated,
                    );
                    match result {
                        Ok(loaded) => drop(sender.send((*drive, loaded.index, loaded.load_ms))),
                        Err(load_err) => tracing::warn!(
                            drive = %drive, path = %path.display(), error = %load_err,
                            "Failed to load MFT file"
                        ),
                    }
                });
            }
        });
    });

    (handle, rx)
}

/// Execute multi-file MFT streaming search.
///
/// Loads multiple MFT files in parallel and streams output as each completes.
fn run_multi_file_streaming(config: &MultiFileStreamConfig<'_>) -> Result<usize> {
    let file_drive_pairs: Vec<_> = config
        .mft_files
        .iter()
        .zip(config.drive_letters.iter())
        .map(|(path, &drv)| (path.clone(), drv))
        .collect();

    let (loader_handle, rx) = spawn_parallel_loaders(
        file_drive_pairs,
        config.debug_tree,
        config.chaos_seed,
        config.reserved_allocated,
    );

    // Build C++ pattern for footer.
    let cpp_pattern = format!(
        ">{}",
        config
            .drive_letters
            .iter()
            .map(|drv| format!("{drv}:{}", config.pattern.replace('*', ".*")))
            .collect::<Vec<_>>()
            .join("|")
    );

    // Streaming closure for each drive.
    let rec_filter = &config.rec_filter;
    let compiled_pattern = &config.compiled_pattern;
    let output_config = config.output_config;

    let stream_drive =
        |mft_index: &uffs_mft::MftIndex, writer: &mut dyn std::io::Write| -> Result<usize> {
            let ext_indices: Option<Vec<u32>> = compiled_pattern.as_ref().and_then(|_pat| {
                let ext_index = mft_index.extension_index.as_ref()?;
                let ext = extract_trailing_extension(config.pattern)?;
                let ext_lower = ext.to_ascii_lowercase();
                let ext_id = mft_index.extensions.map.get(ext_lower.as_str())?;
                Some(ext_index.get_records(*ext_id).to_vec())
            });
            crate::commands::output::write_index_streaming_with_filter(
                mft_index,
                compiled_pattern.as_ref(),
                ext_indices.as_deref(),
                config.case_sensitive,
                config.is_path_pattern,
                rec_filter,
                writer,
                "",
                output_config,
                &crate::commands::output::CppFooterContext::empty(),
            )
        };

    // Stream results to output.
    let cols = crate::commands::output::selected_output_columns(output_config);
    let row_count = write_with_closure(config.out, |writer| {
        crate::commands::output::write_native_header_pub(writer, output_config, cols)?;
        let mut total = 0_usize;
        for (drive, received_index, load_ms) in rx {
            info!(drive = %drive, load_ms, records = received_index.len(), "📊 streaming drive");
            total += stream_drive(&received_index, writer)?;
        }
        if config.format == "custom" {
            let footer = crate::commands::output::CppFooterContext {
                output_targets: config.output_targets,
                pattern: &cpp_pattern,
                row_count: total,
            };
            crate::commands::output::write_cpp_footer_pub(writer, &footer)?;
        }
        writer.flush()?;
        Ok(total)
    })?;

    loader_handle
        .join()
        .map_err(|_panic| anyhow::anyhow!("Loader thread panicked"))?;

    Ok(row_count)
}

/// Write streaming output to console or file using a closure.
fn write_with_closure<F>(out: &str, write_fn: F) -> Result<usize>
where
    F: FnOnce(&mut dyn std::io::Write) -> Result<usize>,
{
    let is_console = out.is_empty() || out == "-";
    if is_console {
        let stdout = std::io::stdout();
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout.lock());
        write_fn(&mut buf_writer)
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let rows = write_fn(&mut buf_writer)?;
        info!(file = out, "Results written to file");
        Ok(rows)
    }
}

/// Configuration for single-file streaming operations.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors CLI parameter structure"
)]
struct SingleFileStreamConfig<'a> {
    /// Path to the MFT file.
    mft_path: &'a std::path::Path,
    /// Pattern string for filtering.
    pattern: &'a str,
    /// Explicit drive letter override.
    single_drive: Option<char>,
    /// Whether to use case-sensitive matching.
    effective_case_sensitive: bool,
    /// Query filters (`files_only`, size, etc).
    filters: &'a QueryFilters<'a>,
    /// Attribute filter string.
    attr_filter: Option<&'a str>,
    /// Date filters.
    newer: Option<&'a str>,
    /// Date filters.
    older: Option<&'a str>,
    /// Date filters.
    newer_created: Option<&'a str>,
    /// Date filters.
    older_created: Option<&'a str>,
    /// Date filters.
    newer_accessed: Option<&'a str>,
    /// Date filters.
    older_accessed: Option<&'a str>,
    /// Exclude patterns.
    exclude: Option<&'a str>,
    /// Sort column.
    sort: Option<&'a str>,
    /// Sort descending.
    sort_desc: bool,
    /// Whether this is a full-scan (no filtering).
    is_full_scan: bool,
    /// Output format.
    format: &'a str,
    /// Output path.
    out: &'a str,
    /// Output configuration.
    output_config: &'a OutputConfig,
    /// Output targets (drive letters).
    output_targets: &'a [char],
    /// Show profiling info.
    profile: bool,
    /// Debug tree flag.
    debug_tree: bool,
    /// Chaos seed for testing.
    chaos_seed: Option<u64>,
    /// Reserved allocation for size queries.
    reserved_allocated: Option<u64>,
    /// Start time for profiling.
    start_time: std::time::Instant,
}

/// Configuration for streaming from a preloaded MFT index.
#[cfg(windows)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors CLI parameter structure"
)]
struct IndexStreamConfig<'a> {
    /// Preloaded MFT index.
    index: &'a uffs_mft::MftIndex,
    /// Load time in milliseconds.
    load_ms: u128,
    /// Pattern string for filtering.
    pattern: &'a str,
    /// Whether to use case-sensitive matching.
    case_sensitive: bool,
    /// Query filters.
    filters: &'a QueryFilters<'a>,
    /// Attribute filter string.
    attr_filter: Option<&'a str>,
    /// Date filters.
    newer: Option<&'a str>,
    /// Date filters.
    older: Option<&'a str>,
    /// Date filters.
    newer_created: Option<&'a str>,
    /// Date filters.
    older_created: Option<&'a str>,
    /// Date filters.
    newer_accessed: Option<&'a str>,
    /// Date filters.
    older_accessed: Option<&'a str>,
    /// Exclude patterns.
    exclude: Option<&'a str>,
    /// Sort column.
    sort: Option<&'a str>,
    /// Sort descending.
    sort_desc: bool,
    /// Whether this is a full-scan.
    is_full_scan: bool,
    /// Output format.
    format: &'a str,
    /// Output path.
    out: &'a str,
    /// Output configuration.
    output_config: &'a OutputConfig,
    /// Output targets (drive letters).
    output_targets: &'a [char],
}

/// Stream results from a preloaded MFT index.
#[cfg(windows)]
fn run_index_streaming(config: &IndexStreamConfig<'_>) -> Result<usize> {
    let cpp_pattern = format!(
        ">{}:{}",
        config.index.volume,
        config.pattern.replace('*', ".*")
    );

    if config.is_full_scan {
        info!(
            drive = %config.index.volume,
            "📂 STREAMING direct-from-index (full scan)"
        );
        write_streaming_output(
            config.index,
            config.format,
            config.out,
            config.output_config,
            config.output_targets,
            &cpp_pattern,
        )
    } else {
        let compiled = uffs_core::compile_parsed_pattern(config.filters.parsed)?;
        let ext_indices = try_get_extension_indices(config.index, config.filters);
        info!(
            drive = %config.index.volume,
            has_ext_index = ext_indices.is_some(),
            "📂 STREAMING with pattern filter"
        );
        let rec_filter = build_record_filter(
            config.filters,
            config.attr_filter,
            config.newer,
            config.older,
            config.newer_created,
            config.older_created,
            config.newer_accessed,
            config.older_accessed,
            config.exclude,
            config.sort,
            config.sort_desc,
        );
        write_streaming_output_with_filter(
            config.index,
            &compiled,
            ext_indices.as_deref(),
            config.case_sensitive,
            config.filters.parsed.is_path_pattern(),
            &rec_filter,
            config.format,
            config.out,
            config.output_config,
            config.output_targets,
            &cpp_pattern,
        )
    }
}

/// Execute live multi-drive streaming search (Windows only).
///
/// Loads indexes from multiple drives in parallel and streams output
/// as each drive completes.
#[cfg(windows)]
#[expect(clippy::too_many_arguments, reason = "mirrors CLI parameter structure")]
async fn run_live_multi_drive_streaming(
    drives: &[char],
    pattern: &str,
    case_sensitive: bool,
    filters: &QueryFilters<'_>,
    attr_filter: Option<&str>,
    newer: Option<&str>,
    older: Option<&str>,
    newer_created: Option<&str>,
    older_created: Option<&str>,
    newer_accessed: Option<&str>,
    older_accessed: Option<&str>,
    exclude: Option<&str>,
    sort: Option<&str>,
    sort_desc: bool,
    is_full_scan: bool,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    no_cache: bool,
) -> Result<()> {
    info!(
        drives = ?drives,
        "📂 LIVE MULTI-DRIVE STREAMING (parallel load, shared output)"
    );

    // Load all indexes in parallel (IOCP reads overlap).
    let mut join_set = tokio::task::JoinSet::new();
    for &drive_letter in drives {
        let nc = no_cache;
        join_set.spawn(async move {
            super::raw_io::load_live_index(drive_letter, nc)
                .await
                .map(|(idx, ms)| (drive_letter, idx, ms))
        });
    }

    // C++ architecture: output each drive AS SOON AS it finishes loading.
    let cpp_pattern = format!(
        ">{}",
        drives
            .iter()
            .map(|drive| format!("{drive}:{}", pattern.replace('*', ".*")))
            .collect::<Vec<_>>()
            .join("|")
    );
    let t_output = std::time::Instant::now();

    // Use a tokio channel so the send is async and never blocks a tokio
    // worker thread (the old sync_channel could stall the runtime when the
    // writer was busy).
    let (tx, rx) =
        tokio::sync::mpsc::channel::<(char, uffs_mft::MftIndex, u128)>(drives.len().max(2));

    // Compile pattern once for the writer thread.
    let compiled_pattern = if is_full_scan {
        None
    } else {
        Some(uffs_core::compile_parsed_pattern(filters.parsed)?)
    };

    // Clone/own values for the writer thread.
    let output_config_clone = output_config.clone();
    let format_owned = format.to_owned();
    let output_targets_clone = output_targets.to_vec();
    let cpp_pattern_clone = cpp_pattern.clone();
    let out_owned = out.to_owned();
    let pattern_owned = pattern.to_owned();
    let is_pp = filters.parsed.is_path_pattern();
    let rec_filter = build_record_filter(
        filters,
        attr_filter,
        newer,
        older,
        newer_created,
        older_created,
        newer_accessed,
        older_accessed,
        exclude,
        sort,
        sort_desc,
    );

    let writer_handle = std::thread::spawn(move || -> Result<usize> {
        run_multi_drive_writer(
            rx,
            &compiled_pattern,
            &pattern_owned,
            case_sensitive,
            is_pp,
            &rec_filter,
            &format_owned,
            &out_owned,
            &output_config_clone,
            &output_targets_clone,
            &cpp_pattern_clone,
        )
    });

    // As each drive finishes loading, send its index to the writer thread.
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(tuple)) => {
                info!(drive = %tuple.0, load_ms = tuple.2, records = tuple.1.len(), "📊 drive ready");
                drop(tx.send(tuple).await);
            }
            Ok(Err(err)) => info!(error = %err, "Drive load failed (continuing)"),
            Err(err) => info!(error = %err, "Task join error (continuing)"),
        }
    }
    drop(tx); // Signal writer thread that all drives are done.

    let total_rows = writer_handle
        .join()
        .map_err(|_panic| anyhow::anyhow!("Writer thread panicked"))??;

    let output_ms = t_output.elapsed().as_millis();
    info!(
        output_ms,
        total_rows, "📊 LIVE multi-drive streaming complete"
    );
    Ok(())
}

/// Writer thread for multi-drive streaming (Windows only).
#[cfg(windows)]
#[expect(
    clippy::too_many_arguments,
    reason = "called from async context with owned values"
)]
fn run_multi_drive_writer(
    mut rx: tokio::sync::mpsc::Receiver<(char, uffs_mft::MftIndex, u128)>,
    compiled_pattern: &Option<uffs_core::index_search::IndexPattern>,
    pattern: &str,
    case_sensitive: bool,
    is_path_pattern: bool,
    rec_filter: &crate::commands::output::StreamingRecordFilter,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    cpp_pattern: &str,
) -> Result<usize> {
    use std::io::Write as _;

    let stream_drive =
        |index: &uffs_mft::MftIndex, writer: &mut dyn std::io::Write| -> Result<usize> {
            let ext_indices: Option<Vec<u32>> = compiled_pattern.as_ref().and_then(|_pat| {
                let ext_index = index.extension_index.as_ref()?;
                let ext = extract_trailing_extension(pattern)?;
                let ext_lower = ext.to_ascii_lowercase();
                let ext_id = index.extensions.map.get(ext_lower.as_str())?;
                Some(ext_index.get_records(*ext_id).to_vec())
            });
            crate::commands::output::write_index_streaming_with_filter(
                index,
                compiled_pattern.as_ref(),
                ext_indices.as_deref(),
                case_sensitive,
                is_path_pattern,
                rec_filter,
                writer,
                "",
                output_config,
                &crate::commands::output::CppFooterContext::empty(),
            )
        };

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );
    let cols = crate::commands::output::selected_output_columns(output_config);

    let mut total_rows = 0_usize;
    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        crate::commands::output::write_native_header_pub(&mut buf_writer, output_config, cols)?;
        while let Some((drive, index, load_ms)) = rx.blocking_recv() {
            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
            total_rows += stream_drive(&index, &mut buf_writer)?;
        }
        if format == "custom" {
            let footer = crate::commands::output::CppFooterContext {
                output_targets,
                pattern: cpp_pattern,
                row_count: total_rows,
            };
            crate::commands::output::write_cpp_footer_pub(&mut buf_writer, &footer)?;
        }
        buf_writer.flush()?;
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        crate::commands::output::write_native_header_pub(&mut buf_writer, output_config, cols)?;
        while let Some((drive, index, load_ms)) = rx.blocking_recv() {
            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
            total_rows += stream_drive(&index, &mut buf_writer)?;
        }
        if format == "custom" {
            let footer = crate::commands::output::CppFooterContext {
                output_targets,
                pattern: cpp_pattern,
                row_count: total_rows,
            };
            crate::commands::output::write_cpp_footer_pub(&mut buf_writer, &footer)?;
        }
        buf_writer.flush()?;
        info!(file = out, "Results written to file");
    }
    Ok(total_rows)
}

/// Execute single-file MFT streaming search.
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing profile output"
)]
fn run_single_file_streaming(config: &SingleFileStreamConfig<'_>) -> Result<()> {
    let effective_drive = config
        .single_drive
        .or_else(|| Some(infer_drive_from_filename(config.mft_path)));

    let native_index = super::raw_io::load_index_from_mft_file(
        config.mft_path,
        effective_drive,
        config.debug_tree,
        config.chaos_seed,
        config.reserved_allocated,
    )?;

    let t_output = std::time::Instant::now();
    let cpp_pattern = format!(
        ">{}:{}",
        native_index.index.volume,
        config.pattern.replace('*', ".*")
    );

    let row_count = if config.is_full_scan {
        info!(
            path = %config.mft_path.display(),
            format = config.format,
            "📂 Loading raw MFT file via STREAMING direct-from-index path (full scan)"
        );
        write_streaming_output(
            &native_index.index,
            config.format,
            config.out,
            config.output_config,
            config.output_targets,
            &cpp_pattern,
        )?
    } else {
        info!(
            path = %config.mft_path.display(),
            format = config.format,
            "📂 Loading raw MFT file via STREAMING filtered path"
        );
        let compiled = uffs_core::compile_parsed_pattern(config.filters.parsed)?;
        let ext_indices = try_get_extension_indices(&native_index.index, config.filters);
        let rec_filter = build_record_filter(
            config.filters,
            config.attr_filter,
            config.newer,
            config.older,
            config.newer_created,
            config.older_created,
            config.newer_accessed,
            config.older_accessed,
            config.exclude,
            config.sort,
            config.sort_desc,
        );
        write_streaming_output_with_filter(
            &native_index.index,
            &compiled,
            ext_indices.as_deref(),
            config.effective_case_sensitive,
            config.filters.parsed.is_path_pattern(),
            &rec_filter,
            config.format,
            config.out,
            config.output_config,
            config.output_targets,
            &cpp_pattern,
        )?
    };

    let output_ms = t_output.elapsed().as_millis();

    if config.profile {
        eprintln!("=== RAW MFT FILE TIMING (streaming) ===");
        eprintln!(
            "  Load from file:  {:>6} ms  ({} records)",
            native_index.load_ms,
            native_index.index.len()
        );
        if config.is_full_scan {
            eprintln!("  Query/filter:    skipped (streaming)");
        }
        eprintln!("  Output/write:    {output_ms:>6} ms  ({row_count} rows)");
        eprintln!(
            "=== TOTAL: {} ms ===",
            config.start_time.elapsed().as_millis()
        );
    }

    info!(count = row_count, "Search complete (streaming)");
    Ok(())
}

/// Dispatch search to the appropriate execution path.
///
/// Returns `StreamingComplete` if output was written directly (early return),
/// or `DataFrame` if results need standard output processing.
async fn dispatch_search(config: &SearchConfig<'_>) -> Result<SearchDispatchResult> {
    // Multi-file streaming path (cross-platform).
    if config.mft_file.len() > 1
        && !config.benchmark
        && can_write_native_results(config.format, &config.output_config)
    {
        run_multi_file_dispatch(config)?;
        return Ok(SearchDispatchResult::StreamingComplete);
    }

    // Single-file streaming path (cross-platform).
    if let Some(mft_path) = config.mft_file.first() {
        if !config.benchmark && can_write_native_results(config.format, &config.output_config) {
            run_single_file_dispatch(config, mft_path)?;
            return Ok(SearchDispatchResult::StreamingComplete);
        }
    }

    // Windows LIVE paths.
    #[cfg(windows)]
    {
        if let Some(result) = dispatch_windows_live(config).await? {
            return Ok(result);
        }
    }

    // Fallback: DataFrame path.
    let df = run_dataframe_search(config).await?;
    Ok(SearchDispatchResult::DataFrame(df))
}

/// Dispatch multi-file streaming search.
fn run_multi_file_dispatch(config: &SearchConfig<'_>) -> Result<()> {
    let drive_letters: Vec<char> = if let Some(drives) = &config.multi_drives {
        if drives.len() != config.mft_file.len() {
            bail!(
                "Number of --drives ({}) must match number of --mft-file ({}).",
                drives.len(),
                config.mft_file.len()
            );
        }
        drives.clone()
    } else {
        config
            .mft_file
            .iter()
            .map(|path| infer_drive_from_filename(path))
            .collect()
    };

    info!(
        files = config.mft_file.len(),
        drives = ?drive_letters,
        "📂 MULTI-FILE STREAMING (cross-platform multi-drive)"
    );

    let compiled_pattern = if config.is_full_scan {
        None
    } else {
        Some(uffs_core::compile_parsed_pattern(config.filters.parsed)?)
    };

    let rec_filter = build_record_filter(
        &config.filters,
        config.attr_filter,
        config.newer,
        config.older,
        config.newer_created,
        config.older_created,
        config.newer_accessed,
        config.older_accessed,
        config.exclude,
        config.sort,
        config.sort_desc,
    );

    let stream_config = MultiFileStreamConfig {
        mft_files: &config.mft_file,
        drive_letters,
        compiled_pattern,
        format: config.format,
        out: config.out,
        output_config: &config.output_config,
        output_targets: &config.output_targets,
        pattern: config.pattern,
        case_sensitive: config.effective_case_sensitive,
        is_path_pattern: config.filters.parsed.is_path_pattern(),
        rec_filter,
        debug_tree: config.debug_tree,
        chaos_seed: config.chaos_seed,
        reserved_allocated: config.reserved_allocated,
    };

    let t_output = std::time::Instant::now();
    let total_rows = run_multi_file_streaming(&stream_config)?;
    let output_ms = t_output.elapsed().as_millis();
    info!(output_ms, total_rows, "📊 multi-file streaming complete");
    Ok(())
}

/// Dispatch single-file streaming search.
fn run_single_file_dispatch(config: &SearchConfig<'_>, mft_path: &std::path::Path) -> Result<()> {
    let stream_config = SingleFileStreamConfig {
        mft_path,
        pattern: config.pattern,
        single_drive: config.single_drive,
        effective_case_sensitive: config.effective_case_sensitive,
        filters: &config.filters,
        attr_filter: config.attr_filter,
        newer: config.newer,
        older: config.older,
        newer_created: config.newer_created,
        older_created: config.older_created,
        newer_accessed: config.older_accessed,
        older_accessed: config.older_accessed,
        exclude: config.exclude,
        sort: config.sort,
        sort_desc: config.sort_desc,
        is_full_scan: config.is_full_scan,
        format: config.format,
        out: config.out,
        output_config: &config.output_config,
        output_targets: &config.output_targets,
        profile: config.profile,
        debug_tree: config.debug_tree,
        chaos_seed: config.chaos_seed,
        reserved_allocated: config.reserved_allocated,
        start_time: config.start_time,
    };
    run_single_file_streaming(&stream_config)
}

/// Dispatch Windows LIVE search paths.
#[cfg(windows)]
async fn dispatch_windows_live(config: &SearchConfig<'_>) -> Result<Option<SearchDispatchResult>> {
    let drives_to_search: Vec<char> = config
        .single_drive
        .map(|drive| vec![drive])
        .or_else(|| config.multi_drives.clone())
        .or_else(|| config.filters.parsed.drive().map(|drive| vec![drive]))
        .unwrap_or_else(uffs_mft::detect_ntfs_drives);

    if drives_to_search.is_empty() {
        bail!("No NTFS drives found to search");
    }

    // Single-drive streaming.
    if drives_to_search.len() == 1
        && !config.benchmark
        && can_write_native_results(config.format, &config.output_config)
    {
        run_live_single_drive(config, drives_to_search[0]).await?;
        return Ok(Some(SearchDispatchResult::StreamingComplete));
    }

    // Multi-drive streaming.
    if drives_to_search.len() > 1
        && !config.benchmark
        && can_write_native_results(config.format, &config.output_config)
    {
        run_live_multi_drive_streaming(
            &drives_to_search,
            config.pattern,
            config.effective_case_sensitive,
            &config.filters,
            config.attr_filter,
            config.newer,
            config.older,
            config.newer_created,
            config.older_created,
            config.newer_accessed,
            config.older_accessed,
            config.exclude,
            config.sort,
            config.sort_desc,
            config.is_full_scan,
            config.format,
            config.out,
            &config.output_config,
            &config.output_targets,
            config.no_cache,
        )
        .await?;
        return Ok(Some(SearchDispatchResult::StreamingComplete));
    }

    // No streaming match - fall through to DataFrame path.
    Ok(None)
}

/// Execute Windows LIVE single-drive streaming.
#[cfg(windows)]
async fn run_live_single_drive(config: &SearchConfig<'_>, drive_letter: char) -> Result<()> {
    debug!(
        ms = config.start_time.elapsed().as_millis(),
        "[TIMING] LIVE: loading index"
    );
    let (index, load_ms) = super::raw_io::load_live_index(drive_letter, config.no_cache).await?;
    debug!(
        ms = config.start_time.elapsed().as_millis(),
        load_ms, "[TIMING] LIVE: index loaded"
    );

    let stream_config = IndexStreamConfig {
        index: &index,
        load_ms,
        pattern: config.pattern,
        case_sensitive: config.effective_case_sensitive,
        filters: &config.filters,
        attr_filter: config.attr_filter,
        newer: config.newer,
        older: config.older,
        newer_created: config.newer_created,
        older_created: config.older_created,
        newer_accessed: config.newer_accessed,
        older_accessed: config.older_accessed,
        exclude: config.exclude,
        sort: config.sort,
        sort_desc: config.sort_desc,
        is_full_scan: config.is_full_scan,
        format: config.format,
        out: config.out,
        output_config: &config.output_config,
        output_targets: &config.output_targets,
    };

    let t_output = std::time::Instant::now();
    let row_count = run_index_streaming(&stream_config)?;
    let output_ms = t_output.elapsed().as_millis();
    info!(load_ms, output_ms, row_count, "📊 LIVE streaming complete");
    Ok(())
}

/// Execute `DataFrame` search (fallback path).
async fn run_dataframe_search(config: &SearchConfig<'_>) -> Result<uffs_mft::DataFrame> {
    // This is the fallback when streaming is not available.
    // Uses the index/DataFrame path with load_and_filter_* helpers.

    // For --mft-file: load and query via existing helper.
    if let Some(mft_path) = config.mft_file.first() {
        return load_and_filter_from_mft_file(
            mft_path,
            config.single_drive,
            &config.filters,
            config.output_config.needs_path_column(),
            config.profile,
            config.debug_tree,
            config.chaos_seed,
        );
    }

    // For --index file or Windows LIVE: use load_and_filter_data.
    load_and_filter_data(
        config.index.clone(),
        config.multi_drives.clone(),
        config.single_drive,
        &config.filters,
        config.output_config.needs_path_column(),
        config.profile,
        config.no_bitmap,
    )
    .await
}

/// Build search configuration from CLI parameters.
#[expect(clippy::too_many_arguments, reason = "mirrors CLI parameters")]
#[expect(clippy::fn_params_excessive_bools, reason = "mirrors CLI parameters")]
fn build_search_config<'a>(
    pattern: &'a str,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
    index: Option<PathBuf>,
    mft_file: Vec<PathBuf>,
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
    format: &'a str,
    case_sensitive: bool,
    smart_case: bool,
    attr_filter: Option<&'a str>,
    newer: Option<&'a str>,
    older: Option<&'a str>,
    newer_created: Option<&'a str>,
    older_created: Option<&'a str>,
    newer_accessed: Option<&'a str>,
    older_accessed: Option<&'a str>,
    exclude: Option<&'a str>,
    word: bool,
    sort: Option<&'a str>,
    sort_desc: bool,
    ext_filter: Option<&'a str>,
    out: &'a str,
    columns: &'a str,
    sep: &'a str,
    quotes: &'a str,
    header: bool,
    pos: &'a str,
    neg: &'a str,
    query_mode: &'a str,
    tz_offset: Option<i32>,
    chaos_seed: Option<u64>,
    reserved_allocated: Option<u64>,
    start_time: std::time::Instant,
) -> Result<SearchConfig<'a>> {
    // Smart case: if enabled and pattern has any uppercase letter,
    // automatically enable case-sensitive matching.
    let effective_case_sensitive =
        case_sensitive || (smart_case && pattern.chars().any(|ch| ch.is_ascii_uppercase()));

    // Whole word: wrap pattern in \b...\b regex.
    let effective_pattern: alloc::borrow::Cow<'_, str> = if word {
        alloc::borrow::Cow::Owned(format!(">\\b{pattern}\\b"))
    } else {
        alloc::borrow::Cow::Borrowed(pattern)
    };

    let parsed = ParsedPattern::parse(&effective_pattern)
        .with_context(|| format!("Invalid pattern: {pattern}"))?
        .with_case_sensitive(effective_case_sensitive);

    let filters = QueryFilters {
        parsed: Box::leak(Box::new(parsed)),
        ext_filter,
        files_only,
        dirs_only,
        hide_system,
        min_size,
        max_size,
        limit,
    };

    let mut output_config = OutputConfig::new()
        .with_columns(columns)
        .with_separator(sep)
        .with_quote(quotes)
        .with_header(header)
        .with_pos(pos)
        .with_neg(neg);
    if let Some(hours) = tz_offset {
        output_config = output_config.with_tz_offset_hours(hours);
    }

    let output_targets =
        compute_output_targets(single_drive, multi_drives.as_ref(), filters.parsed.drive());
    let is_full_scan = is_full_scan_query(&filters);

    Ok(SearchConfig {
        pattern,
        single_drive,
        multi_drives,
        index,
        mft_file,
        filters,
        effective_case_sensitive,
        profile,
        debug_tree,
        benchmark,
        no_bitmap,
        no_cache,
        attr_filter,
        newer,
        older,
        newer_created,
        older_created,
        newer_accessed,
        older_accessed,
        exclude,
        sort,
        sort_desc,
        format,
        out,
        output_config,
        output_targets,
        is_full_scan,
        query_mode,
        chaos_seed,
        reserved_allocated,
        start_time,
    })
}

/// Finalize `DataFrame` output with tree columns and output writing.
fn finalize_dataframe_output(
    mut results: uffs_mft::DataFrame,
    config: &SearchConfig<'_>,
) -> Result<()> {
    let t_tree = std::time::Instant::now();
    if !config.benchmark && config.output_config.needs_tree_columns() {
        let tree_cols = config.output_config.get_tree_columns();
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

    let elapsed = config.start_time.elapsed();
    let t_output = std::time::Instant::now();
    if !config.benchmark {
        write_results(
            &results,
            config.format,
            config.out,
            &config.output_config,
            &config.output_targets,
            elapsed,
            config.pattern,
        )?;
    }
    let output_ms = t_output.elapsed().as_millis();

    if config.benchmark {
        print_benchmark_stats(&results, elapsed);
    } else if config.profile {
        print_profile_stats(&results, tree_ms, output_ms, elapsed);
    }

    info!(count = results.height(), "Search complete");
    Ok(())
}

/// Print benchmark statistics.
#[expect(clippy::print_stderr, reason = "intentional user-facing output")]
fn print_benchmark_stats(results: &uffs_mft::DataFrame, elapsed: core::time::Duration) {
    let row_count = results.height();
    let total_ms = elapsed.as_millis();
    let secs = elapsed.as_secs_f64();
    eprintln!("=== BENCHMARK MODE (no output) ===");
    eprintln!("  Records found:   {row_count:>10}");
    eprintln!("  Total time:      {total_ms:>10} ms ({secs:.2} s)");
    #[expect(
        clippy::cast_precision_loss,
        reason = "row_count as f64 is fine for display"
    )]
    #[expect(
        clippy::float_arithmetic,
        reason = "throughput calculation for display"
    )]
    let throughput = row_count as f64 / secs;
    eprintln!("  Throughput:      {throughput:>10.0} records/sec");
}

/// Print profiling statistics.
#[expect(clippy::print_stderr, reason = "intentional user-facing output")]
fn print_profile_stats(
    results: &uffs_mft::DataFrame,
    tree_ms: u128,
    output_ms: u128,
    elapsed: core::time::Duration,
) {
    let row_count = results.height();
    let total_ms = elapsed.as_millis();
    eprintln!("=== PROFILE: Output ===");
    eprintln!("  Tree columns:    {tree_ms:>6} ms");
    eprintln!("  Output/write:    {output_ms:>6} ms  ({row_count} rows)");
    eprintln!("=== TOTAL: {total_ms} ms ===");
}

/// Search for files matching a pattern.
///
/// Supports:
/// - Drive prefix in pattern: `c:/pro*` extracts drive C
/// - REGEX patterns: `>C:\Temp.*` (starts with `>`)
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
    clippy::single_call_fn,
    reason = "public CLI entry point called from main dispatch"
)]
pub async fn search(
    pattern: &str,
    single_drive: Option<char>,
    multi_drives: Option<Vec<char>>,
    index: Option<PathBuf>,
    mft_file: Vec<PathBuf>,
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
    smart_case: bool,
    attr_filter: Option<&str>,
    newer: Option<&str>,
    older: Option<&str>,
    newer_created: Option<&str>,
    older_created: Option<&str>,
    newer_accessed: Option<&str>,
    older_accessed: Option<&str>,
    exclude: Option<&str>,
    word: bool,
    sort: Option<&str>,
    sort_desc: bool,
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
    chaos_seed: Option<u64>,
    reserved_allocated: Option<u64>,
) -> Result<()> {
    let start_time = std::time::Instant::now();
    debug!("[TIMING] search() entered at 0ms");

    // Build configuration from CLI parameters.
    let config = build_search_config(
        pattern,
        single_drive,
        multi_drives,
        index,
        mft_file,
        files_only,
        dirs_only,
        hide_system,
        profile,
        debug_tree,
        benchmark,
        no_bitmap,
        no_cache,
        min_size,
        max_size,
        limit,
        format,
        case_sensitive,
        smart_case,
        attr_filter,
        newer,
        older,
        newer_created,
        older_created,
        newer_accessed,
        older_accessed,
        exclude,
        word,
        sort,
        sort_desc,
        ext_filter,
        out,
        columns,
        sep,
        quotes,
        header,
        pos,
        neg,
        query_mode,
        tz_offset,
        chaos_seed,
        reserved_allocated,
        start_time,
    )?;

    // Dispatch to appropriate search path.
    let result = dispatch_search(&config).await?;

    // Handle result.
    match result {
        SearchDispatchResult::StreamingComplete => Ok(()),
        SearchDispatchResult::DataFrame(df) => finalize_dataframe_output(df, &config),
    }
}

/// Shared helper: write streaming output from an `MftIndex` to file or console.
///
/// Used by both `--mft-file` and Windows LIVE full-scan paths (DRY).
fn write_streaming_output(
    index: &uffs_mft::MftIndex,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    cpp_pattern: &str,
) -> Result<usize> {
    use std::io::Write as _;

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        // Wrap stdout in BufWriter to avoid per-line WriteFile syscalls.
        // Without this, 2.2M write_all calls each trigger a syscall
        // through the OS pipe, making redirected stdout 15× slower than
        // C's printf (which buffers in the C runtime).
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        let footer_ctx = crate::commands::output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let result = crate::commands::output::write_index_streaming(
            index,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        );
        writer.flush()?;
        result
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let footer_ctx = crate::commands::output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let count = crate::commands::output::write_index_streaming(
            index,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        )?;
        writer.flush()?;
        info!(file = out, "Results written to file");
        Ok(count)
    }
}

/// Try to get record indices from the extension index for simple suffix
/// patterns.
///
/// Returns `Some(Vec<u32>)` for patterns like `*.rs`, `*.txt` where the
/// extension index provides O(matches) lookup.  Returns `None` for complex
/// patterns that need full-scan matching.
fn try_get_extension_indices(
    index: &uffs_mft::MftIndex,
    filters: &QueryFilters<'_>,
) -> Option<Vec<u32>> {
    // Only works for simple glob suffix patterns with no other filters.
    if filters.files_only
        || filters.dirs_only
        || filters.hide_system
        || filters.ext_filter.is_some()
        || filters.min_size.is_some()
        || filters.max_size.is_some()
        || filters.limit > 0
    {
        return None;
    }

    let pattern = filters.parsed.pattern();

    // Extract a trailing literal extension from ANY pattern.
    // Examples:
    //   "*.txt"         → ext = "txt"
    //   "*hallo*.txt"   → ext = "txt"
    //   "foo*.rs"       → ext = "rs"
    //   "*.tar.gz"      → None (multi-dot)
    //   "*hallo*"       → None (no extension)
    //   "nice"          → None (no dot)
    let ext = extract_trailing_extension(pattern)?;

    let ext_index = index.extension_index.as_ref()?;
    let ext_lower = ext.to_ascii_lowercase();
    let ext_id = index.extensions.map.get(ext_lower.as_str())?;
    Some(ext_index.get_records(*ext_id).to_vec())
}

/// Build a `StreamingRecordFilter` from `QueryFilters` + extra CLI params.
#[expect(clippy::too_many_arguments, reason = "collects all filter CLI params")]
fn build_record_filter(
    filters: &QueryFilters<'_>,
    attr_filter: Option<&str>,
    newer: Option<&str>,
    older: Option<&str>,
    newer_created: Option<&str>,
    older_created: Option<&str>,
    newer_accessed: Option<&str>,
    older_accessed: Option<&str>,
    exclude: Option<&str>,
    sort: Option<&str>,
    sort_desc: bool,
) -> crate::commands::output::StreamingRecordFilter {
    let exclude_pattern = exclude.and_then(|excl| uffs_core::compile_index_pattern(excl).ok());

    crate::commands::output::StreamingRecordFilter {
        files_only: filters.files_only,
        dirs_only: filters.dirs_only,
        hide_system: filters.hide_system,
        min_size: filters.min_size,
        max_size: filters.max_size,
        attr_filters: attr_filter
            .map(crate::commands::output::parse_attr_filter)
            .unwrap_or_default(),
        newer_modified: newer.and_then(crate::commands::output::parse_age_filter),
        older_modified: older.and_then(crate::commands::output::parse_age_filter),
        newer_created: newer_created.and_then(crate::commands::output::parse_age_filter),
        older_created: older_created.and_then(crate::commands::output::parse_age_filter),
        newer_accessed: newer_accessed.and_then(crate::commands::output::parse_age_filter),
        older_accessed: older_accessed.and_then(crate::commands::output::parse_age_filter),
        exclude_pattern,
        limit: filters.limit as usize,
        sort_spec: sort
            .map(crate::commands::output::parse_sort_spec)
            .unwrap_or_default(),
        sort_desc,
    }
}

/// Extract a trailing literal file extension from a glob/regex pattern.
///
/// Returns the extension (without dot) if the pattern ends with a literal
/// `.ext` where `ext` contains no wildcards, dots, or special chars.
///
/// # Examples
/// - `"*.txt"` → `Some("txt")`
/// - `"*hallo*.txt"` → `Some("txt")`
/// - `"foo*.rs"` → `Some("rs")`
/// - `"*.tar.gz"` → `None` (ext contains dot)
/// - `"*hallo*"` → `None` (no extension)
/// - `"nice"` → `None` (no dot)
/// - `"*.tx?"` → `None` (wildcard in ext)
fn extract_trailing_extension(pattern: &str) -> Option<&str> {
    // Find the last dot in the pattern.
    let dot_pos = pattern.rfind('.')?;
    let ext = pattern.get(dot_pos + 1..)?;

    // Extension must be non-empty and contain no wildcards or dots.
    if ext.is_empty()
        || ext.contains('*')
        || ext.contains('?')
        || ext.contains('.')
        || ext.contains('[')
    {
        return None;
    }

    Some(ext)
}

/// Shared helper: write streaming output with pattern filter to file or
/// console.
///
/// Unified path for ALL filtered patterns — compiles the pattern and
/// optionally uses the extension index for O(matches) scan.
#[expect(
    clippy::too_many_arguments,
    reason = "unified streaming writer needs pattern, indices, case, path-mode, filter, format, output config, targets, footer pattern"
)]
fn write_streaming_output_with_filter(
    index: &uffs_mft::MftIndex,
    pattern: &uffs_core::IndexPattern,
    record_indices: Option<&[u32]>,
    case_sensitive: bool,
    is_path_pattern: bool,
    record_filter: &crate::commands::output::StreamingRecordFilter,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    cpp_pattern: &str,
) -> Result<usize> {
    use std::io::Write as _;

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        let footer_ctx = crate::commands::output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let result = crate::commands::output::write_index_streaming_with_filter(
            index,
            Some(pattern),
            record_indices,
            case_sensitive,
            is_path_pattern,
            record_filter,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        );
        writer.flush()?;
        result
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        let footer_ctx = crate::commands::output::CppFooterContext {
            output_targets,
            pattern: cpp_pattern,
            row_count: 0,
        };
        let count = crate::commands::output::write_index_streaming_with_filter(
            index,
            Some(pattern),
            record_indices,
            case_sensitive,
            is_path_pattern,
            record_filter,
            &mut writer,
            format,
            output_config,
            &footer_ctx,
        )?;
        writer.flush()?;
        info!(file = out, "Results written to file");
        Ok(count)
    }
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
pub(super) async fn search_multi_drive_filtered(
    _drives: &[char],
    _filters: &QueryFilters<'_>,
    _needs_paths: bool,
    _no_bitmap: bool,
) -> Result<uffs_mft::DataFrame> {
    bail!("Multi-drive search is only supported on Windows")
}

/// Compute the list of output targets (drive letters) for results.
fn compute_output_targets(
    single_drive: Option<char>,
    multi_drives: Option<&Vec<char>>,
    pattern_drive: Option<char>,
) -> Vec<char> {
    single_drive
        .map(|drive| vec![drive])
        .or_else(|| multi_drives.cloned())
        .or_else(|| pattern_drive.map(|drive| vec![drive]))
        .unwrap_or_default()
}

/// Check if the query is a full-scan (no filtering).
///
/// A full-scan means all files are returned without filtering,
/// which allows bypassing `SearchResult` allocation in streaming paths.
fn is_full_scan_query(filters: &QueryFilters<'_>) -> bool {
    !filters.files_only
        && !filters.dirs_only
        && !filters.hide_system
        && filters.ext_filter.is_none()
        && filters.min_size.is_none()
        && filters.max_size.is_none()
        && filters.limit == 0
        && is_any_match_pattern(filters.parsed.pattern())
}

/// Check if the pattern matches all files (i.e., `*`, `**`, `**/*`, or empty).
fn is_any_match_pattern(pattern: &str) -> bool {
    matches!(pattern, "*" | "**" | "**/*" | "")
}

/// Infer a drive letter from an MFT filename.
///
/// If the filename starts with a single ASCII letter followed by a
/// non-letter (e.g., `C.bin`, `C_mft.bin`, `D-drive.mft`), returns
/// that letter uppercased.  Otherwise returns `'X'` as fallback.
fn infer_drive_from_filename(path: &std::path::Path) -> char {
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut chars = stem.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii_alphabetic() {
            // Second char must be non-alphabetic (or end of string)
            match chars.next() {
                None | Some('.' | '_' | '-' | ' ') => {
                    return first.to_ascii_uppercase();
                }
                _ => {}
            }
        }
    }
    'X'
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        MAX_CONCURRENT_SEARCH_DRIVE_TASKS, infer_drive_from_filename, search_drive_task_budget,
    };

    #[test]
    fn search_drive_task_budget_handles_empty_input() {
        assert_eq!(search_drive_task_budget(0), 0);
    }

    #[test]
    fn search_drive_task_budget_never_exceeds_drive_count() {
        assert_eq!(search_drive_task_budget(1), 1);
        assert!(search_drive_task_budget(3) <= 3);
    }

    #[test]
    fn search_drive_task_budget_caps_drive_fan_out() {
        assert!(
            search_drive_task_budget(MAX_CONCURRENT_SEARCH_DRIVE_TASKS + 8)
                <= MAX_CONCURRENT_SEARCH_DRIVE_TASKS
        );
    }

    #[test]
    fn infer_drive_from_common_mft_filenames() {
        assert_eq!(infer_drive_from_filename(Path::new("C.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("c.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("D_mft.bin")), 'D');
        assert_eq!(infer_drive_from_filename(Path::new("f-drive.mft")), 'F');
        assert_eq!(infer_drive_from_filename(Path::new("G mft.raw")), 'G');
    }

    #[test]
    fn infer_drive_falls_back_to_x_for_ambiguous_names() {
        assert_eq!(infer_drive_from_filename(Path::new("raw.bin")), 'X');
        assert_eq!(infer_drive_from_filename(Path::new("backup_mft.bin")), 'X');
        assert_eq!(infer_drive_from_filename(Path::new("12345.bin")), 'X');
    }

    #[test]
    fn infer_drive_handles_full_paths() {
        assert_eq!(infer_drive_from_filename(Path::new("/tmp/C.bin")), 'C');
        assert_eq!(infer_drive_from_filename(Path::new("/data/D_mft.raw")), 'D');
    }
}
