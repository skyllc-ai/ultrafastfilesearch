//! Search command implementation.

extern crate alloc;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};
use uffs_core::QueryMode;
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

    // Smart case: if enabled and pattern has any uppercase letter,
    // automatically enable case-sensitive matching (like fd/ripgrep).
    // Explicit --case always wins over smart case.
    let effective_case_sensitive =
        case_sensitive || (smart_case && pattern.chars().any(|ch| ch.is_ascii_uppercase()));

    // Whole word: wrap pattern in \b...\b regex for word boundary matching.
    let effective_pattern: alloc::borrow::Cow<'_, str> = if word {
        alloc::borrow::Cow::Owned(format!(">\\b{pattern}\\b"))
    } else {
        alloc::borrow::Cow::Borrowed(pattern)
    };

    let parsed = ParsedPattern::parse(&effective_pattern)
        .with_context(|| format!("Invalid pattern: {pattern}"))?
        .with_case_sensitive(effective_case_sensitive);

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

    // Detect full-scan (no filtering) — used by both --mft-file and
    // Windows LIVE streaming paths to bypass SearchResult allocation.
    let is_full_scan = !filters.files_only
        && !filters.dirs_only
        && !filters.hide_system
        && filters.ext_filter.is_none()
        && filters.min_size.is_none()
        && filters.max_size.is_none()
        && filters.limit == 0
        && (filters.parsed.pattern() == "*"
            || filters.parsed.pattern() == "**"
            || filters.parsed.pattern() == "**/*"
            || filters.parsed.pattern().is_empty());

    if let Some(mft_path) = mft_file.as_ref()
        && !benchmark
        && can_write_native_results(format, &output_config)
    {
        if is_full_scan {
            info!(
                path = %mft_path.display(),
                format,
                "📂 Loading raw MFT file via STREAMING direct-from-index path (full scan)"
            );

            // Load index only (skip IndexQuery::collect entirely)
            let native_index = super::raw_io::load_index_from_mft_file(
                mft_path,
                single_drive,
                debug_tree,
                chaos_seed,
                reserved_allocated,
            )?;

            let t_output = std::time::Instant::now();
            let cpp_pattern = format!(
                ">{}:{}",
                native_index.index.volume,
                pattern.replace('*', ".*")
            );
            let row_count = write_streaming_output(
                &native_index.index,
                format,
                out,
                &output_config,
                &output_targets,
                &cpp_pattern,
            )?;
            let output_ms = t_output.elapsed().as_millis();

            if profile {
                eprintln!("=== RAW MFT FILE TIMING (streaming) ===");
                eprintln!(
                    "  Load from file:  {:>6} ms  ({} records)",
                    native_index.load_ms,
                    native_index.index.len()
                );
                eprintln!("  Query/filter:    skipped (streaming)");
                eprintln!("  Output/write:    {output_ms:>6} ms  ({row_count} rows)");
                eprintln!("=== TOTAL: {} ms ===", start_time.elapsed().as_millis());
            }

            info!(count = row_count, "Search complete (streaming)");
            return Ok(());
        }

        // Filtered query: use unified streaming path (same as LIVE).
        // All filters (attr, date, sort, exclude) work cross-platform via --mft-file.
        info!(
            path = %mft_path.display(),
            format,
            "📂 Loading raw MFT file via STREAMING filtered path"
        );

        let native_index = super::raw_io::load_index_from_mft_file(
            mft_path,
            single_drive,
            debug_tree,
            chaos_seed,
            reserved_allocated,
        )?;

        let compiled = uffs_core::compile_parsed_pattern(filters.parsed)?;
        let ext_indices = try_get_extension_indices(&native_index.index, &filters);
        let rec_filter = build_record_filter(
            &filters,
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

        let t_output = std::time::Instant::now();
        let cpp_pattern = format!(
            ">{}:{}",
            native_index.index.volume,
            pattern.replace('*', ".*")
        );
        let row_count = write_streaming_output_with_filter(
            &native_index.index,
            &compiled,
            ext_indices.as_deref(),
            effective_case_sensitive,
            filters.parsed.is_path_pattern(),
            &rec_filter,
            format,
            out,
            &output_config,
            &output_targets,
            &cpp_pattern,
        )?;
        let output_ms = t_output.elapsed().as_millis();

        if profile {
            eprintln!("=== RAW MFT FILE TIMING (streaming) ===");
            eprintln!(
                "  Load from file:  {:>6} ms  ({} records)",
                native_index.load_ms,
                native_index.index.len()
            );
            eprintln!("  Output/write:    {output_ms:>6} ms  ({row_count} rows)");
            eprintln!("=== TOTAL: {} ms ===", start_time.elapsed().as_millis());
        }

        info!(count = row_count, "Search complete (streaming)");
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
            chaos_seed,
        )?
    } else if use_index_path {
        info!("🚀 Using fast cached MftIndex query path");
        #[cfg(windows)]
        {
            if drives_to_search.is_empty() {
                bail!("No NTFS drives found on this system");
            }

            // DRY: Single-drive with native output uses the SAME path as
            // --mft-file.  The only difference is how the MftIndex is built
            // (IOCP live vs file replay).  After that, query+output is
            // identical.
            if drives_to_search.len() == 1
                && !benchmark
                && can_write_native_results(format, &output_config)
            {
                let drive_letter = drives_to_search[0];
                debug!(
                    ms = start_time.elapsed().as_millis(),
                    "[TIMING] LIVE: loading index"
                );
                let (index, load_ms) =
                    super::raw_io::load_live_index(drive_letter, no_cache).await?;
                debug!(
                    ms = start_time.elapsed().as_millis(),
                    load_ms, "[TIMING] LIVE: index loaded"
                );

                // From here, IDENTICAL to the --mft-file native output path.
                // Full scan → streaming; filtered → IndexQuery + native write.
                if is_full_scan {
                    info!(
                        drive = %drive_letter,
                        "📂 LIVE STREAMING direct-from-index (full scan, same as --mft-file)"
                    );
                    let t_output = std::time::Instant::now();
                    let cpp_pattern = format!(">{}:{}", index.volume, pattern.replace('*', ".*"));
                    let row_count = write_streaming_output(
                        &index,
                        format,
                        out,
                        &output_config,
                        &output_targets,
                        &cpp_pattern,
                    )?;
                    let output_ms = t_output.elapsed().as_millis();
                    debug!(
                        ms = start_time.elapsed().as_millis(),
                        load_ms, output_ms, row_count, "[TIMING] LIVE: done"
                    );
                    info!(load_ms, output_ms, row_count, "📊 LIVE streaming complete");
                    return Ok(());
                }

                // ALL filtered patterns use unified streaming: compile the
                // pattern, optionally use extension index for O(matches)
                // scan, and write matches directly.  Zero SearchResult.
                let compiled = uffs_core::compile_parsed_pattern(filters.parsed)?;
                let ext_indices = try_get_extension_indices(&index, &filters);
                info!(
                    drive = %drive_letter,
                    has_ext_index = ext_indices.is_some(),
                    "📂 LIVE STREAMING with pattern filter (zero SearchResult)"
                );
                let t_output = std::time::Instant::now();
                let cpp_pattern = format!(">{}:{}", index.volume, pattern.replace('*', ".*"));
                let rec_filter = build_record_filter(
                    &filters,
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
                let row_count = write_streaming_output_with_filter(
                    &index,
                    &compiled,
                    ext_indices.as_deref(),
                    effective_case_sensitive,
                    filters.parsed.is_path_pattern(),
                    &rec_filter,
                    format,
                    out,
                    &output_config,
                    &output_targets,
                    &cpp_pattern,
                )?;
                let output_ms = t_output.elapsed().as_millis();
                info!(
                    load_ms,
                    output_ms, row_count, "📊 LIVE streaming+filter complete"
                );
                return Ok(());
            }

            // Multi-drive with native output: load indexes in parallel,
            // stream each drive's output through our fast path.
            // Works for ALL patterns (full scan + filtered).
            // No DataFrame, no SearchResult, no Polars merge.
            if drives_to_search.len() > 1
                && !benchmark
                && can_write_native_results(format, &output_config)
            {
                info!(
                    drives = ?drives_to_search,
                    "📂 LIVE MULTI-DRIVE STREAMING (parallel load, shared output)"
                );

                // Load all indexes in parallel (IOCP reads overlap).
                // Collect completed indexes, then stream output synchronously.
                // StdoutLock is !Send so we can't hold it across .await.
                let mut join_set = tokio::task::JoinSet::new();
                for &drive_letter in &drives_to_search {
                    let nc = no_cache;
                    join_set.spawn(async move {
                        super::raw_io::load_live_index(drive_letter, nc)
                            .await
                            .map(|(idx, ms)| (drive_letter, idx, ms))
                    });
                }

                // C++ architecture: output each drive AS SOON AS it finishes
                // loading, while other drives continue reading in the
                // background.  This overlaps output I/O with disk reads.
                let cpp_pattern = format!(
                    ">{}",
                    drives_to_search
                        .iter()
                        .map(|d| format!("{}:{}", d, pattern.replace('*', ".*")))
                        .collect::<Vec<_>>()
                        .join("|")
                );
                let t_output = std::time::Instant::now();

                // Use a channel: load tasks send indexes, main thread writes.
                let (tx, rx) = std::sync::mpsc::sync_channel::<(char, uffs_mft::MftIndex, u128)>(2);

                // Compile pattern once for the writer thread.
                let compiled_pattern = if is_full_scan {
                    None
                } else {
                    Some(uffs_core::compile_parsed_pattern(filters.parsed)?)
                };

                // Spawn a writer thread that streams output as indexes arrive.
                let output_config_clone = output_config.clone();
                let format_owned = format.to_owned();
                let output_targets_clone = output_targets.clone();
                let cpp_pattern_clone = cpp_pattern.clone();
                let out_owned = out.to_owned();
                let pattern_owned = pattern.to_owned();
                let cs = effective_case_sensitive;
                let is_pp = filters.parsed.is_path_pattern();
                let rf = build_record_filter(
                    &filters,
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
                    use std::io::Write as _;
                    let is_console = matches!(
                        out_owned.to_lowercase().as_str(),
                        "console" | "con" | "term" | "terminal"
                    );

                    // Per-drive streaming: for each index, optionally extract
                    // extension indices for O(matches) pre-filtering, then
                    // write with the unified streaming writer.
                    let stream_drive = |index: &uffs_mft::MftIndex,
                                        w: &mut dyn std::io::Write|
                     -> Result<usize> {
                        // Extract extension indices per-drive (each drive has its own ext index).
                        let ext_indices: Option<Vec<u32>> =
                            compiled_pattern.as_ref().and_then(|_pat| {
                                let ext_index = index.extension_index.as_ref()?;
                                let ext = extract_trailing_extension(&pattern_owned)?;
                                let ext_lower = ext.to_ascii_lowercase();
                                let ext_id = index.extensions.map.get(ext_lower.as_str())?;
                                Some(ext_index.get_records(*ext_id).to_vec())
                            });
                        crate::commands::output::write_index_streaming_with_filter(
                            index,
                            compiled_pattern.as_ref(),
                            ext_indices.as_deref(),
                            cs,
                            is_pp,
                            &rf,
                            w,
                            "",
                            &output_config_clone,
                            &crate::commands::output::CppFooterContext {
                                output_targets: &[],
                                pattern: "",
                                row_count: 0,
                            },
                        )
                    };

                    let mut total_rows = 0usize;
                    if is_console {
                        let stdout_handle = std::io::stdout();
                        let stdout_lock = stdout_handle.lock();
                        let mut w = std::io::BufWriter::with_capacity(1024 * 1024, stdout_lock);
                        let cols =
                            crate::commands::output::selected_output_columns(&output_config_clone);
                        crate::commands::output::write_native_header_pub(
                            &mut w,
                            &output_config_clone,
                            cols,
                        )?;
                        for (drive, index, load_ms) in rx {
                            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
                            total_rows += stream_drive(&index, &mut w)?;
                        }
                        if format_owned == "custom" {
                            let footer = crate::commands::output::CppFooterContext {
                                output_targets: &output_targets_clone,
                                pattern: &cpp_pattern_clone,
                                row_count: total_rows,
                            };
                            crate::commands::output::write_cpp_footer_pub(&mut w, &footer)?;
                        }
                        w.flush()?;
                    } else {
                        let file = std::fs::File::create(&out_owned).with_context(|| {
                            format!("Failed to create output file: {out_owned}")
                        })?;
                        let mut w = std::io::BufWriter::with_capacity(1024 * 1024, file);
                        let cols =
                            crate::commands::output::selected_output_columns(&output_config_clone);
                        crate::commands::output::write_native_header_pub(
                            &mut w,
                            &output_config_clone,
                            cols,
                        )?;
                        for (drive, index, load_ms) in rx {
                            info!(drive = %drive, load_ms, records = index.len(), "📊 streaming drive");
                            total_rows += stream_drive(&index, &mut w)?;
                        }
                        if format_owned == "custom" {
                            let footer = crate::commands::output::CppFooterContext {
                                output_targets: &output_targets_clone,
                                pattern: &cpp_pattern_clone,
                                row_count: total_rows,
                            };
                            crate::commands::output::write_cpp_footer_pub(&mut w, &footer)?;
                        }
                        w.flush()?;
                        info!(file = out_owned, "Results written to file");
                    }
                    Ok(total_rows)
                });

                // As each drive finishes loading, send its index to the
                // writer thread immediately.  Other drives continue loading
                // in parallel while the writer outputs the completed drive.
                while let Some(result) = join_set.join_next().await {
                    match result {
                        Ok(Ok(tuple)) => {
                            info!(drive = %tuple.0, load_ms = tuple.2, records = tuple.1.len(), "📊 drive ready, sending to writer");
                            let _ = tx.send(tuple);
                        }
                        Ok(Err(e)) => info!(error = %e, "Drive load failed (continuing)"),
                        Err(e) => info!(error = %e, "Task join error (continuing)"),
                    }
                }
                drop(tx); // Signal writer thread that all drives are done.

                let total_rows = writer_handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("Writer thread panicked"))??;

                let output_ms = t_output.elapsed().as_millis();
                info!(
                    output_ms,
                    total_rows, "📊 LIVE multi-drive streaming complete"
                );
                return Ok(());
            }

            // Multi-drive non-native or filtered: fall back to DataFrame path
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

    let elapsed = start_time.elapsed();
    let t_output = std::time::Instant::now();
    if !benchmark {
        write_results(
            &results,
            format,
            out,
            &output_config,
            &output_targets,
            elapsed,
            pattern,
        )?;
    }
    let output_ms = t_output.elapsed().as_millis();

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
