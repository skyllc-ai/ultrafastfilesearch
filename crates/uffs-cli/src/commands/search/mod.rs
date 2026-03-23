//! Search command implementation.
//!
//! Delegates to submodules for dispatch, streaming I/O, and platform-specific
//! paths. This file contains only the public entry point, shared types, and
//! module wiring.

extern crate alloc;

use std::path::PathBuf;

use anyhow::Result;
use tracing::debug;
use uffs_core::output::OutputConfig;

use super::raw_io::QueryFilters;

/// Search dispatch routing and configuration building.
mod dispatch;
/// Windows LIVE multi-drive and single-drive streaming search.
mod live;
/// Multi-file MFT streaming search execution.
mod mft_file;
/// Single-file MFT streaming search execution.
mod single_file;
/// Streaming I/O helpers for writing search results.
mod streaming_io;
/// Pure utility helpers for the search command.
mod util;

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
    /// Force filename-only matching (--name-only flag).
    name_only: bool,
    /// Query mode.
    query_mode: &'a str,
    /// Chaos seed for testing.
    chaos_seed: Option<u64>,
    /// Reserved allocation for size queries.
    reserved_allocated: Option<u64>,
    /// Start time for profiling.
    start_time: std::time::Instant,
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
    name_only: bool,
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
    let config = dispatch::build_search_config(
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
        name_only,
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
    let result = dispatch::dispatch_search(&config).await?;

    // Handle result.
    match result {
        SearchDispatchResult::StreamingComplete => Ok(()),
        SearchDispatchResult::DataFrame(df) => dispatch::finalize_dataframe_output(df, &config),
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
    anyhow::bail!("Multi-drive search is only supported on Windows")
}
