//! Search dispatch routing and configuration building.

extern crate alloc;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::info;
use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;

use super::super::output::{can_write_native_results, write_results};
use super::super::raw_io::{QueryFilters, load_and_filter_data, load_and_filter_from_mft_file};
use super::streaming_io::build_record_filter;
use super::util::{compute_output_targets, is_full_scan_query};
use super::{SearchConfig, SearchDispatchResult};

/// Dispatch search to the appropriate execution path.
///
/// Returns `StreamingComplete` if output was written directly (early return),
/// or `DataFrame` if results need standard output processing.
pub(super) async fn dispatch_search(config: &SearchConfig<'_>) -> Result<SearchDispatchResult> {
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
        if let Some(result) = super::live::dispatch_windows_live(config).await? {
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
            .map(|path| super::util::infer_drive_from_filename(path))
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

    let stream_config = super::mft_file::MultiFileStreamConfig {
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
    let total_rows = super::mft_file::run_multi_file_streaming(&stream_config)?;
    let output_ms = t_output.elapsed().as_millis();
    info!(output_ms, total_rows, "📊 multi-file streaming complete");
    Ok(())
}

/// Dispatch single-file streaming search.
fn run_single_file_dispatch(config: &SearchConfig<'_>, mft_path: &std::path::Path) -> Result<()> {
    let stream_config = super::single_file::SingleFileStreamConfig {
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
    super::single_file::run_single_file_streaming(&stream_config)
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
pub(super) fn build_search_config<'a>(
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
pub(super) fn finalize_dataframe_output(
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
