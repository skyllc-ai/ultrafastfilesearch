//! Single-file MFT streaming search execution.

use anyhow::Result;
use tracing::info;
use uffs_core::output::OutputConfig;

use super::super::raw_io::QueryFilters;
use super::streaming_io::{
    build_record_filter, try_get_extension_indices, write_streaming_output,
    write_streaming_output_with_filter,
};
use super::util::infer_drive_from_filename;

/// Configuration for single-file streaming operations.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors CLI parameter structure"
)]
pub(super) struct SingleFileStreamConfig<'a> {
    /// Path to the MFT file.
    pub(super) mft_path: &'a std::path::Path,
    /// Pattern string for filtering.
    pub(super) pattern: &'a str,
    /// Explicit drive letter override.
    pub(super) single_drive: Option<char>,
    /// Whether to use case-sensitive matching.
    pub(super) effective_case_sensitive: bool,
    /// Query filters (`files_only`, size, etc).
    pub(super) filters: &'a QueryFilters<'a>,
    /// Attribute filter string.
    pub(super) attr_filter: Option<&'a str>,
    /// Date filters.
    pub(super) newer: Option<&'a str>,
    /// Date filters.
    pub(super) older: Option<&'a str>,
    /// Date filters.
    pub(super) newer_created: Option<&'a str>,
    /// Date filters.
    pub(super) older_created: Option<&'a str>,
    /// Date filters.
    pub(super) newer_accessed: Option<&'a str>,
    /// Date filters.
    pub(super) older_accessed: Option<&'a str>,
    /// Exclude patterns.
    pub(super) exclude: Option<&'a str>,
    /// Sort column.
    pub(super) sort: Option<&'a str>,
    /// Sort descending.
    pub(super) sort_desc: bool,
    /// Whether this is a full-scan (no filtering).
    pub(super) is_full_scan: bool,
    /// Output format.
    pub(super) format: &'a str,
    /// Output path.
    pub(super) out: &'a str,
    /// Output configuration.
    pub(super) output_config: &'a OutputConfig,
    /// Output targets (drive letters).
    pub(super) output_targets: &'a [char],
    /// Show profiling info.
    pub(super) profile: bool,
    /// Debug tree flag.
    pub(super) debug_tree: bool,
    /// Chaos seed for testing.
    pub(super) chaos_seed: Option<u64>,
    /// Reserved allocation for size queries.
    pub(super) reserved_allocated: Option<u64>,
    /// Start time for profiling.
    pub(super) start_time: std::time::Instant,
}

/// Execute single-file MFT streaming search.
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing profile output"
)]
pub(super) fn run_single_file_streaming(config: &SingleFileStreamConfig<'_>) -> Result<()> {
    let effective_drive = config
        .single_drive
        .or_else(|| Some(infer_drive_from_filename(config.mft_path)));

    let native_index = super::super::raw_io::load_index_from_mft_file(
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
