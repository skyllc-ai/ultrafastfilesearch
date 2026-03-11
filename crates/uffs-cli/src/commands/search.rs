//! Search command implementation.
//! Exception: This module exceeds 800 lines because the end-to-end search
//! command flow remains consolidated pending a command-surface split outside
//! Wave 3C.

#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::{BufWriter, Write};
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};
#[cfg(windows)]
use indicatif::ProgressBar;
#[cfg(windows)]
use tracing::debug;
use tracing::info;
use uffs_core::QueryMode;
use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;

#[cfg(windows)]
use super::output::StreamingWriter;
use super::output::write_results;
#[cfg(windows)]
use super::raw_io::{
    OwnedQueryFilters, load_and_filter_data_index, load_and_filter_data_index_multi,
};
use super::raw_io::{QueryFilters, load_and_filter_data, load_and_filter_from_mft_file};
#[cfg(windows)]
use super::{add_drive_progress, create_multi_progress};

/// Maximum number of drive-level CLI search tasks to run concurrently.
///
/// This mirrors the approved Wave 2A cap-4 drive budget so the CLI does not
/// multiply already-parallel reader work across too many volumes at once.
#[cfg(any(windows, test))]
const MAX_CONCURRENT_SEARCH_DRIVE_TASKS: usize = 4;

/// Returns the bounded drive-level task budget for CLI multi-drive searches.
#[cfg(any(windows, test))]
#[must_use]
fn search_drive_task_budget(total_drives: usize) -> usize {
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

/// Determine if we should use the fast `MftIndex` query path.
///
/// Returns `true` if:
/// - `QueryMode::ForceIndex` is set, OR
/// - `QueryMode::Auto` is set and query can use the fast path
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
            if parquet_index.is_some() {
                info!(
                    requested_mode = ?mode,
                    has_parquet_index = true,
                    decision = "dataframe",
                    reason = "parquet_index_provided",
                    "Ignoring forced index mode because a parquet index file was provided"
                );
                return false;
            }
            if multi_drives.is_some() {
                info!(
                    requested_mode = ?mode,
                    drive_scope = "multiple",
                    decision = "index",
                    "Forced index mode selected for multi-drive search"
                );
            }
            true
        }
        QueryMode::ForceDataFrame => false,
        QueryMode::Auto => {
            if parquet_index.is_some() {
                info!(
                    requested_mode = ?mode,
                    has_parquet_index = true,
                    decision = "dataframe",
                    reason = "parquet_index_provided",
                    "Auto mode selected the DataFrame/parquet path"
                );
                return false;
            }
            true
        }
    }
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
    let start_time = std::time::Instant::now();

    let parsed = ParsedPattern::parse(pattern)
        .with_context(|| format!("Invalid pattern: {pattern}"))?
        .with_case_sensitive(case_sensitive);

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

    let mode = QueryMode::from_str_opt(query_mode).unwrap_or_else(|| {
        info!(query_mode, "Unknown query mode, defaulting to auto");
        QueryMode::Auto
    });
    info!(?mode, "Query execution mode");

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

    let needs_paths = !benchmark && output_config.needs_path_column();
    let use_index_path = should_use_index_path(mode, index.as_ref(), multi_drives.as_ref());
    let execution_path = if mft_file.is_some() {
        "raw_mft_file"
    } else if use_index_path {
        "mft_index"
    } else {
        "dataframe"
    };

    #[cfg(windows)]
    let streaming_candidate = !benchmark
        && !use_index_path
        && index.is_none()
        && (multi_drives.is_some() || (single_drive.is_none() && filters.parsed.drive().is_none()));

    #[cfg(windows)]
    let drives_to_search: Vec<char> = if let Some(ref drives) = multi_drives {
        drives.clone()
    } else if let Some(drive) = single_drive.or_else(|| filters.parsed.drive()) {
        vec![drive]
    } else {
        uffs_mft::detect_ntfs_drives()
    };

    let output_targets: Vec<char> = single_drive
        .map(|drive| vec![drive])
        .or_else(|| multi_drives.clone())
        .or_else(|| filters.parsed.drive().map(|drive| vec![drive]))
        .unwrap_or_default();

    #[cfg(windows)]
    let drive_selection_source = if multi_drives.is_some() {
        "--drives"
    } else if single_drive.is_some() {
        "--drive"
    } else if filters.parsed.drive().is_some() {
        "pattern"
    } else {
        "detect_ntfs_drives"
    };

    #[cfg(windows)]
    let requested_multi_drive_count = multi_drives.as_ref().map_or(0, |drives| drives.len());

    #[cfg(windows)]
    info!(
        requested_mode = ?mode,
        execution_path,
        benchmark,
        profile,
        needs_paths,
        has_parquet_index = index.is_some(),
        has_raw_mft_file = mft_file.is_some(),
        output_format = format,
        output_target = out,
        output_targets = ?output_targets,
        limit,
        no_cache,
        no_bitmap,
        streaming_candidate,
        "Resolved search orchestration plan"
    );

    #[cfg(not(windows))]
    info!(
        requested_mode = ?mode,
        execution_path,
        benchmark,
        profile,
        needs_paths,
        has_parquet_index = index.is_some(),
        has_raw_mft_file = mft_file.is_some(),
        output_format = format,
        output_target = out,
        output_targets = ?output_targets,
        limit,
        no_cache,
        no_bitmap,
        "Resolved search orchestration plan"
    );

    #[cfg(windows)]
    info!(
        drive_source = drive_selection_source,
        requested_single_drive = ?single_drive,
        requested_multi_drive_count,
        pattern_drive = ?filters.parsed.drive(),
        selected_drives = ?drives_to_search,
        selected_drive_count = drives_to_search.len(),
        "Resolved search drive set"
    );

    let mut results = if let Some(mft_path) = mft_file.as_ref() {
        info!(
            execution_path = "raw_mft_file",
            path = %mft_path.display(),
            needs_paths,
            debug_tree,
            "Loading search source from raw MFT file"
        );
        load_and_filter_from_mft_file(
            mft_path,
            single_drive,
            &filters,
            needs_paths,
            profile,
            debug_tree,
        )?
    } else if use_index_path {
        info!(
            execution_path = "mft_index",
            cache_enabled = !no_cache,
            needs_paths,
            "Using MftIndex query path"
        );
        #[cfg(windows)]
        {
            if drives_to_search.is_empty() {
                bail!("No NTFS drives found on this system");
            }

            if drives_to_search.len() == 1 {
                info!(
                    drive = %drives_to_search[0],
                    wait_strategy = "single_drive_index_query",
                    cache_enabled = !no_cache,
                    "Dispatching single-drive index search"
                );
                load_and_filter_data_index(
                    Some(drives_to_search[0]),
                    &filters,
                    needs_paths,
                    profile,
                    no_cache,
                )
                .await?
            } else {
                info!(
                    drives = ?drives_to_search,
                    drive_count = drives_to_search.len(),
                    wait_strategy = "multi_drive_index_join_set",
                    cache_enabled = !no_cache,
                    "Dispatching multi-drive index search"
                );
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
        info!(
            execution_path = "dataframe",
            needs_paths, "Using DataFrame query path"
        );
        #[cfg(windows)]
        if !benchmark {
            let needs_streaming = streaming_candidate;

            if needs_streaming {
                info!(
                    wait_strategy = "streaming_multi_drive_join_set",
                    limit, "Routing DataFrame search through streaming orchestration"
                );
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

    let t_tree = std::time::Instant::now();
    if !benchmark && output_config.needs_tree_columns() {
        let tree_cols = output_config.get_tree_columns();
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

    let t_output = std::time::Instant::now();
    if !benchmark {
        write_results(&results, format, out, &output_config, &output_targets)?;
    }
    let output_ms = t_output.elapsed().as_millis();

    let elapsed = start_time.elapsed();

    if benchmark {
        let row_count = results.height();
        let total_ms = elapsed.as_millis();
        let secs = elapsed.as_secs_f64();
        eprintln!("=== BENCHMARK MODE (no output) ===");
        eprintln!("  Records found:   {row_count:>10}");
        eprintln!("  Total time:      {total_ms:>10} ms ({secs:.2} s)");
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
    let drive_selection_source = if multi_drives.is_some() {
        "--drives"
    } else if single_drive.is_some() {
        "--drive"
    } else if filters.parsed.drive().is_some() {
        "pattern"
    } else {
        "detect_ntfs_drives"
    };

    let drives: Vec<char> = if let Some(drives) = multi_drives {
        drives
    } else if let Some(drive) = single_drive.or_else(|| filters.parsed.drive()) {
        vec![drive]
    } else {
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

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    info!(
        drive_source = drive_selection_source,
        drives = ?drives,
        drive_count = drives.len(),
        output_mode = if is_console { "console" } else { "file" },
        output_target = out,
        output_format = format,
        limit = filters.limit,
        "Resolved streaming search orchestration"
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

/// Execute the per-drive CLI search pipeline for a single drive.
///
/// The task checks the shared cancellation flag between major phases so the
/// orchestration layer can stop downstream work once streaming output has
/// reached its limit.
#[cfg(windows)]
async fn run_drive_search_task(
    drive_char: char,
    filters: Arc<OwnedQueryFilters>,
    progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>>,
    needs_paths: bool,
    no_bitmap: bool,
    cancelled: Arc<AtomicBool>,
) -> DriveResult {
    let pb = progress_bars
        .as_ref()
        .and_then(|bars| bars.get(&drive_char));

    if cancelled.load(Ordering::Relaxed) {
        debug!(
            drive = %drive_char,
            cancellation_phase = "before_cache_load",
            "Skipping drive search task because streaming orchestration is cancelled"
        );
        if let Some(progress_bar) = pb {
            progress_bar.finish_with_message("Cancelled");
        }
        return DriveResult {
            drive: drive_char,
            df: None,
            records_read: 0,
            matches: 0,
            error: None,
            paths_resolved: false,
        };
    }

    let _ = no_bitmap;

    debug!(
        drive = %drive_char,
        needs_paths,
        progress_bar = pb.is_some(),
        cache_mode = "dataframe_cache_with_ttl",
        ttl_seconds = uffs_mft::INDEX_TTL_SECONDS,
        wait_strategy = "await_cached_dataframe_load",
        "Starting per-drive search task"
    );

    let full_df =
        uffs_mft::load_or_build_dataframe_cached(drive_char, uffs_mft::INDEX_TTL_SECONDS).await;

    let full_df = match full_df {
        Ok(df) => df,
        Err(error) => {
            if let Some(progress_bar) = pb {
                progress_bar.finish_with_message(format!("Error: {error}"));
            }
            return DriveResult {
                drive: drive_char,
                df: None,
                records_read: 0,
                matches: 0,
                error: Some(error.to_string()),
                paths_resolved: false,
            };
        }
    };

    let records_read = full_df.height();
    debug!(
        drive = %drive_char,
        records_read,
        wait_strategy = "cached_dataframe_load_complete",
        "Loaded drive snapshot for search task"
    );
    if let Some(progress_bar) = pb {
        progress_bar.finish();
    }

    if cancelled.load(Ordering::Relaxed) {
        debug!(
            drive = %drive_char,
            records_read,
            cancellation_phase = "after_cache_load",
            "Cancelling drive search task after cached DataFrame load"
        );
        return DriveResult {
            drive: drive_char,
            df: None,
            records_read,
            matches: 0,
            error: None,
            paths_resolved: false,
        };
    }

    let path_resolver = if needs_paths {
        debug!(
            drive = %drive_char,
            records_read,
            "Building path resolver from full drive snapshot before filtering"
        );
        match uffs_core::FastPathResolver::build(&full_df, drive_char) {
            Ok(resolver) => Some(resolver),
            Err(error) => {
                return DriveResult {
                    drive: drive_char,
                    df: None,
                    records_read,
                    matches: 0,
                    error: Some(format!("Failed to build path resolver: {error}")),
                    paths_resolved: false,
                };
            }
        }
    } else {
        None
    };

    if cancelled.load(Ordering::Relaxed) {
        debug!(
            drive = %drive_char,
            records_read,
            paths_resolved = path_resolver.is_some(),
            cancellation_phase = "after_path_resolution",
            "Cancelling drive search task after path-resolution setup"
        );
        return DriveResult {
            drive: drive_char,
            df: None,
            records_read,
            matches: 0,
            error: None,
            paths_resolved: path_resolver.is_some(),
        };
    }

    let filtered = match filters.execute(full_df) {
        Ok(filtered) => filtered,
        Err(error) => {
            return DriveResult {
                drive: drive_char,
                df: None,
                records_read,
                matches: 0,
                error: Some(error.to_string()),
                paths_resolved: false,
            };
        }
    };

    let matches = filtered.height();
    debug!(
        drive = %drive_char,
        records_read,
        matches,
        paths_resolved = path_resolver.is_some(),
        "Drive filtering phase completed"
    );

    let with_paths = if let Some(resolver) = &path_resolver {
        match resolver.add_path_column_with_dir_suffix(&filtered) {
            Ok(df) => match uffs_core::add_path_only_column(&df) {
                Ok(df_with_path_only) => {
                    match uffs_core::apply_directory_treesize(&df_with_path_only) {
                        Ok(df_with_treesize) => df_with_treesize,
                        Err(error) => {
                            return DriveResult {
                                drive: drive_char,
                                df: None,
                                records_read,
                                matches,
                                error: Some(format!("Failed to apply treesize: {error}")),
                                paths_resolved: false,
                            };
                        }
                    }
                }
                Err(error) => {
                    return DriveResult {
                        drive: drive_char,
                        df: None,
                        records_read,
                        matches,
                        error: Some(format!("Failed to add path_only: {error}")),
                        paths_resolved: false,
                    };
                }
            },
            Err(error) => {
                return DriveResult {
                    drive: drive_char,
                    df: None,
                    records_read,
                    matches,
                    error: Some(format!("Failed to add paths: {error}")),
                    paths_resolved: false,
                };
            }
        }
    } else {
        match uffs_core::apply_directory_treesize(&filtered) {
            Ok(df) => df,
            Err(error) => {
                return DriveResult {
                    drive: drive_char,
                    df: None,
                    records_read,
                    matches,
                    error: Some(format!("Failed to apply treesize: {error}")),
                    paths_resolved: false,
                };
            }
        }
    };

    if cancelled.load(Ordering::Relaxed) {
        return DriveResult {
            drive: drive_char,
            df: None,
            records_read,
            matches,
            error: None,
            paths_resolved: path_resolver.is_some(),
        };
    }

    let df_with_drive = if matches > 0 {
        match with_paths
            .lazy()
            .with_column(uffs_mft::lit(format!("{drive_char}:")).alias("drive"))
            .collect()
        {
            Ok(df) => Some(df),
            Err(error) => {
                return DriveResult {
                    drive: drive_char,
                    df: None,
                    records_read,
                    matches,
                    error: Some(error.to_string()),
                    paths_resolved: path_resolver.is_some(),
                };
            }
        }
    } else {
        None
    };

    DriveResult {
        drive: drive_char,
        df: df_with_drive,
        records_read,
        matches,
        error: None,
        paths_resolved: path_resolver.is_some(),
    }
}

/// Spawn a single per-drive CLI search task into the provided `JoinSet`.
#[cfg(windows)]
fn spawn_drive_search_task(
    join_set: &mut tokio::task::JoinSet<DriveResult>,
    drive_char: char,
    filters: Arc<OwnedQueryFilters>,
    progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>>,
    needs_paths: bool,
    no_bitmap: bool,
    cancelled: Arc<AtomicBool>,
) {
    join_set.spawn(run_drive_search_task(
        drive_char,
        filters,
        progress_bars,
        needs_paths,
        no_bitmap,
        cancelled,
    ));
}

/// Search multiple drives with bounded per-drive filtering concurrency.
///
/// This keeps only a limited number of drive tasks in flight at once, then
/// collects and merges results as each task completes.
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
pub(super) async fn search_multi_drive_filtered(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    no_bitmap: bool,
) -> Result<uffs_mft::DataFrame> {
    use tokio::task::JoinSet;
    use uffs_mft::{IntoLazy, col};

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    let budget = search_drive_task_budget(drives.len());

    info!(
        drives = ?drives,
        count = drives.len(),
        budget,
        needs_paths,
        no_bitmap,
        "Searching drives with bounded multi-drive orchestration"
    );

    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));
    let cancelled = Arc::new(AtomicBool::new(false));
    let multi_progress = create_multi_progress();

    let progress_bars: Option<Arc<std::collections::HashMap<char, ProgressBar>>> =
        multi_progress.as_ref().map(|mp| {
            let mut pbs = std::collections::HashMap::new();
            for &drive_char in drives {
                pbs.insert(drive_char, add_drive_progress(mp, drive_char));
            }
            Arc::new(pbs)
        });

    let mut pending_drives = drives.iter().copied();
    let mut join_set: JoinSet<DriveResult> = JoinSet::new();
    let mut drives_dispatched = 0usize;

    for _ in 0..budget {
        if let Some(drive_char) = pending_drives.next() {
            drives_dispatched += 1;
            debug!(
                drive = %drive_char,
                dispatch_reason = "initial",
                drives_dispatched,
                drive_count = drives.len(),
                "Queued drive search task"
            );
            spawn_drive_search_task(
                &mut join_set,
                drive_char,
                Arc::clone(&owned_filters),
                progress_bars.clone(),
                needs_paths,
                no_bitmap,
                Arc::clone(&cancelled),
            );
        }
    }

    let mut filtered_results: Vec<uffs_mft::DataFrame> = Vec::new();
    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while !join_set.is_empty() {
        debug!(
            drives_processed,
            drives_dispatched,
            drive_count = drives.len(),
            in_flight = drives_dispatched.saturating_sub(drives_processed),
            wait_strategy = "join_next",
            "Waiting for next multi-drive search result"
        );

        let Some(join_result) = join_set.join_next().await else {
            break;
        };

        drives_processed += 1;

        let result = match join_result {
            Ok(result) => result,
            Err(join_err) => {
                info!(
                    error = %join_err,
                    drives_processed,
                    drives_dispatched,
                    "Drive task join failed"
                );
                DriveResult {
                    drive: '?',
                    df: None,
                    records_read: 0,
                    matches: 0,
                    error: Some(format!("Task failed: {join_err}")),
                    paths_resolved: false,
                }
            }
        };

        if let Some(error) = result.error {
            info!(drive = %result.drive, error = %error, "Drive failed");
        } else {
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

        if let Some(drive_char) = pending_drives.next() {
            drives_dispatched += 1;
            debug!(
                drive = %drive_char,
                dispatch_reason = "replenish",
                drives_dispatched,
                drive_count = drives.len(),
                "Queued drive search task"
            );
            spawn_drive_search_task(
                &mut join_set,
                drive_char,
                Arc::clone(&owned_filters),
                progress_bars.clone(),
                needs_paths,
                no_bitmap,
                Arc::clone(&cancelled),
            );
        }
    }

    if filtered_results.is_empty() {
        bail!("No matching files found across {} drives", drives.len());
    }

    let mut merged = filtered_results.remove(0);
    for df in filtered_results {
        merged = merged.vstack(&df).context("Failed to merge results")?;
    }

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

    if filters.limit > 0 {
        lazy_result = lazy_result.limit(filters.limit);
    }

    let result = lazy_result.collect().context("Failed to reorder columns")?;

    info!(
        budget,
        total_matches = total_matches,
        rows = result.height(),
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
pub(super) async fn search_multi_drive_filtered(
    _drives: &[char],
    _filters: &QueryFilters<'_>,
    _needs_paths: bool,
    _no_bitmap: bool,
) -> Result<uffs_mft::DataFrame> {
    bail!("Multi-drive search is only supported on Windows")
}

/// Search multiple drives with bounded streaming output orchestration.
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
    use tokio::task::JoinSet;
    use uffs_mft::{IntoLazy, col};

    if drives.is_empty() {
        bail!("No drives specified for multi-drive search");
    }

    let budget = search_drive_task_budget(drives.len());

    info!(
        drives = ?drives,
        count = drives.len(),
        budget,
        format,
        limit = filters.limit,
        "Streaming search across drives with bounded orchestration"
    );

    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));
    let cancelled = Arc::new(AtomicBool::new(false));
    let streaming_writer = Arc::new(StreamingWriter::new(
        writer,
        format,
        filters.limit,
        output_config.clone(),
    ));

    let mut pending_drives = drives.iter().copied();
    let mut join_set: JoinSet<DriveResult> = JoinSet::new();
    let mut drives_dispatched = 0usize;

    for _ in 0..budget {
        if let Some(drive_char) = pending_drives.next() {
            drives_dispatched += 1;
            debug!(
                drive = %drive_char,
                dispatch_reason = "initial",
                drives_dispatched,
                drive_count = drives.len(),
                "Queued streaming drive search task"
            );
            spawn_drive_search_task(
                &mut join_set,
                drive_char,
                Arc::clone(&owned_filters),
                None,
                true,
                no_bitmap,
                Arc::clone(&cancelled),
            );
        }
    }

    let mut total_matches = 0usize;
    let mut drives_processed = 0usize;

    while !join_set.is_empty() {
        debug!(
            drives_processed,
            drives_dispatched,
            drive_count = drives.len(),
            in_flight = drives_dispatched.saturating_sub(drives_processed),
            wait_strategy = "join_next",
            "Waiting for next streaming drive result"
        );

        let Some(join_result) = join_set.join_next().await else {
            break;
        };

        drives_processed += 1;

        let result = match join_result {
            Ok(result) => result,
            Err(join_err) => {
                info!(
                    error = %join_err,
                    drives_processed,
                    drives_dispatched,
                    "Streaming drive task join failed"
                );
                DriveResult {
                    drive: '?',
                    df: None,
                    records_read: 0,
                    matches: 0,
                    error: Some(format!("Task failed: {join_err}")),
                    paths_resolved: false,
                }
            }
        };

        if let Some(error) = result.error {
            eprintln!("[{}:] Error: {}", result.drive, error);
        } else {
            total_matches += result.matches;

            if let Some(ref df) = result.df {
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
                    if let Err(error) = streaming_writer.write_batch(&reordered) {
                        eprintln!("[{}:] Write error: {}", result.drive, error);
                    }
                }
            }

            if streaming_writer.limit_reached() {
                cancelled.store(true, Ordering::Relaxed);
                join_set.abort_all();
                info!(
                    limit = filters.limit,
                    rows_output = streaming_writer.total_rows(),
                    drives_processed,
                    drives_dispatched,
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

        if let Some(drive_char) = pending_drives.next() {
            drives_dispatched += 1;
            debug!(
                drive = %drive_char,
                dispatch_reason = "replenish",
                drives_dispatched,
                drive_count = drives.len(),
                "Queued streaming drive search task"
            );
            spawn_drive_search_task(
                &mut join_set,
                drive_char,
                Arc::clone(&owned_filters),
                None,
                true,
                no_bitmap,
                Arc::clone(&cancelled),
            );
        }
    }

    info!(
        budget,
        total_matches = total_matches,
        rows_output = streaming_writer.total_rows(),
        drives = drives.len(),
        "Streaming search complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT_SEARCH_DRIVE_TASKS, search_drive_task_budget};

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
}
