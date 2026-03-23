//! Windows LIVE multi-drive and single-drive streaming search.

#[cfg(windows)]
use anyhow::{Context, Result, bail};
#[cfg(windows)]
use tracing::{debug, info};
#[cfg(windows)]
use uffs_core::output::OutputConfig;

#[cfg(windows)]
use super::super::output;
#[cfg(windows)]
use super::super::raw_io::QueryFilters;
#[cfg(windows)]
use super::streaming_io::{
    build_record_filter, try_get_extension_indices, write_streaming_output,
    write_streaming_output_with_filter,
};
#[cfg(windows)]
use super::util::extract_trailing_extension;
#[cfg(windows)]
use super::{SearchConfig, SearchDispatchResult};

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
    let cpp_pattern = if config.pattern.starts_with('>') {
        config.pattern.to_owned()
    } else {
        format!(
            ">{}:{}",
            config.index.volume,
            config.pattern.replace('*', ".*")
        )
    };

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

/// Dispatch Windows LIVE search paths.
#[cfg(windows)]
pub(super) async fn dispatch_windows_live(
    config: &SearchConfig<'_>,
) -> Result<Option<SearchDispatchResult>> {
    use super::super::output::can_write_native_results;

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
    let (index, load_ms) =
        super::super::raw_io::load_live_index(drive_letter, config.no_cache).await?;
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
            super::super::raw_io::load_live_index(drive_letter, nc)
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
    rec_filter: &output::StreamingRecordFilter,
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
            output::write_index_streaming_with_filter(
                index,
                compiled_pattern.as_ref(),
                ext_indices.as_deref(),
                case_sensitive,
                is_path_pattern,
                rec_filter,
                writer,
                "",
                output_config,
                &output::CppFooterContext::empty(),
            )
        };

    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );
    let cols = output::selected_output_columns(output_config);

    let mut total_rows = 0_usize;
    if is_console {
        let stdout_handle = std::io::stdout();
        let stdout_lock = stdout_handle.lock();
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
        output::write_native_header_pub(&mut buf_writer, output_config, cols)?;
        while let Some((drive, index, load_ms)) = rx.blocking_recv() {
            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
            total_rows += stream_drive(&index, &mut buf_writer)?;
        }
        if format == "custom" {
            let footer = output::CppFooterContext {
                output_targets,
                pattern: cpp_pattern,
                row_count: total_rows,
            };
            output::write_cpp_footer_pub(&mut buf_writer, &footer)?;
        }
        buf_writer.flush()?;
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("Failed to create output file: {out}"))?;
        let mut buf_writer = std::io::BufWriter::with_capacity(1024 * 1024, file);
        output::write_native_header_pub(&mut buf_writer, output_config, cols)?;
        while let Some((drive, index, load_ms)) = rx.blocking_recv() {
            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
            total_rows += stream_drive(&index, &mut buf_writer)?;
        }
        if format == "custom" {
            let footer = output::CppFooterContext {
                output_targets,
                pattern: cpp_pattern,
                row_count: total_rows,
            };
            output::write_cpp_footer_pub(&mut buf_writer, &footer)?;
        }
        buf_writer.flush()?;
        info!(file = out, "Results written to file");
    }
    Ok(total_rows)
}
