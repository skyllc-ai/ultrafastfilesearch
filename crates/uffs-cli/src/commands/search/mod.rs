//! Search command implementation.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::info;
use uffs_core::QueryMode;
use uffs_core::output::OutputConfig;
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::add_tree_columns;

use super::output::{can_write_native_results, write_native_results, write_results};
use super::raw_io::{
    QueryFilters, load_and_filter_data, load_and_filter_from_mft_file,
    load_and_filter_native_from_mft_file,
};
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
pub(super) use self::multi_drive::search_multi_drive_filtered;
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
                info!("⚠️ --query-mode=index ignored: using parquet index file");
                return false;
            }
            if multi_drives.is_some() {
                info!("⚠️ --query-mode=index: multi-drive not yet supported, using single drive");
            }
            true
        }
        QueryMode::ForceDataFrame => false,
        QueryMode::Auto => {
            if parquet_index.is_some() {
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
    reason = "top-level search orchestrator remains the command surface entry point"
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

    if let Some(mft_path) = mft_file.as_ref()
        && !benchmark
        && can_write_native_results(format, &output_config)
    {
        info!(
            path = %mft_path.display(),
            format,
            "📂 Loading raw MFT file via native direct-output path"
        );

        let native = load_and_filter_native_from_mft_file(
            mft_path,
            single_drive,
            &filters,
            needs_paths,
            debug_tree,
        )?;

        let t_output = std::time::Instant::now();
        write_native_results(
            &native.index,
            &native.results,
            format,
            out,
            &output_config,
            &output_targets,
        )?;
        let output_ms = t_output.elapsed().as_millis();
        let elapsed = start_time.elapsed();

        if profile {
            let raw_total_ms = native.load_ms + native.query_ms;
            eprintln!("=== RAW MFT FILE TIMING ===");
            eprintln!(
                "  Load from file:  {:>6} ms  ({} records)",
                native.load_ms,
                native.index.len()
            );
            eprintln!(
                "  Query/filter:    {:>6} ms  ({} matches)",
                native.query_ms,
                native.results.len()
            );
            eprintln!("  TOTAL:           {raw_total_ms:>6} ms");
            eprintln!("=== PROFILE: Output ===");
            eprintln!("  Tree columns:    {:>6} ms", 0_u128);
            eprintln!(
                "  Output/write:    {:>6} ms  ({} rows)",
                output_ms,
                native.results.len()
            );
            eprintln!("=== TOTAL: {} ms ===", elapsed.as_millis());
        }

        info!(count = native.results.len(), "Search complete");
        return Ok(());
    }

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
            if drives_to_search.is_empty() {
                bail!("No NTFS drives found on this system");
            }

            if drives_to_search.len() == 1 {
                load_and_filter_data_index(
                    Some(drives_to_search[0]),
                    &filters,
                    needs_paths,
                    profile,
                    no_cache,
                )
                .await?
            } else {
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
        #[cfg(windows)]
        if !benchmark {
            let needs_streaming = index.is_none()
                && (multi_drives.is_some()
                    || (single_drive.is_none() && filters.parsed.drive().is_none()));

            if needs_streaming {
                return search_streaming(
                    multi_drives.clone(),
                    single_drive,
                    &filters,
                    format,
                    out,
                    &output_config,
                    no_bitmap,
                )
                .await;
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
