//! Raw MFT and data-loading helpers for CLI search commands.

#![expect(
    clippy::single_call_fn,
    reason = "raw MFT/data-loading helpers are orchestrated from the search pipeline"
)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tracing::{debug, info};
use uffs_core::MftQuery;
use uffs_core::extensions::ExtensionFilter;
use uffs_core::pattern::ParsedPattern;
use uffs_mft::MftReader;

use super::output::results_to_dataframe;

#[cfg(windows)]
#[path = "raw_io_windows.rs"]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{
    OwnedQueryFilters, load_and_filter_data_index, load_and_filter_data_index_multi,
};

/// Native offline query results for direct `--mft-file` output.
pub(super) struct NativeOfflineQueryResults {
    /// Loaded offline index used for record metadata lookups during output.
    pub(super) index: uffs_mft::MftIndex,
    /// Native search results collected from `IndexQuery`.
    pub(super) results: Vec<uffs_core::SearchResult>,
    /// Raw MFT load duration in milliseconds.
    pub(super) load_ms: u128,
    /// Query/filter duration in milliseconds.
    pub(super) query_ms: u128,
}

/// Query filter options for the search command.
pub struct QueryFilters<'a> {
    /// Parsed search pattern (glob, regex, or literal).
    pub parsed: &'a ParsedPattern,
    /// Extension filter string (e.g., "pictures,mp4,pdf").
    pub ext_filter: Option<&'a str>,
    /// Only return files (not directories).
    pub files_only: bool,
    /// Only return directories (not files).
    pub dirs_only: bool,
    /// Hide system files (files starting with $).
    pub hide_system: bool,
    /// Minimum file size filter.
    pub min_size: Option<u64>,
    /// Maximum file size filter.
    pub max_size: Option<u64>,
    /// Maximum number of results to return.
    pub limit: u32,
}

/// Load and filter search data from a raw MFT file (cross-platform debugging).
///
/// This function loads a previously saved raw MFT file and processes it
/// exactly like a live MFT read, enabling debugging on any platform.
/// Same pipeline as Windows live read - only the load source differs.
#[expect(clippy::single_call_fn, reason = "extracted from search() for clarity")]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
#[tracing::instrument(
    level = "info",
    skip(mft_path, filters),
    fields(
        drive_letter = ?drive_letter,
        needs_paths,
        profile,
        debug_tree,
        files_only = filters.files_only,
        dirs_only = filters.dirs_only,
        hide_system = filters.hide_system,
        min_size = ?filters.min_size,
        max_size = ?filters.max_size,
        limit = filters.limit,
        has_ext_filter = filters.ext_filter.is_some()
    )
)]
pub(super) fn load_and_filter_from_mft_file(
    mft_path: &Path,
    drive_letter: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    debug_tree: bool,
    chaos_seed: Option<u64>,
) -> Result<uffs_mft::DataFrame> {
    let native = load_and_filter_native_from_mft_file(
        mft_path,
        drive_letter,
        filters,
        needs_paths,
        debug_tree,
        chaos_seed,
    )?;
    let matches = native.results.len();
    let records = native.index.len();
    let df = results_to_dataframe(&native.index, native.results, needs_paths)?;

    if profile {
        let total_ms = native.load_ms + native.query_ms;
        eprintln!("=== RAW MFT FILE TIMING ===");
        eprintln!(
            "  Load from file:  {:>6} ms  ({} records)",
            native.load_ms, records
        );
        eprintln!(
            "  Query/filter:    {:>6} ms  ({} matches)",
            native.query_ms, matches
        );
        eprintln!("  TOTAL:           {total_ms:>6} ms");
    }

    Ok(df)
}

/// Load, query, and return native results from a raw offline MFT file.
pub(super) fn load_and_filter_native_from_mft_file(
    mft_path: &Path,
    drive_letter: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    debug_tree: bool,
    chaos_seed: Option<u64>,
) -> Result<NativeOfflineQueryResults> {
    use uffs_mft::LoadRawOptions;

    let volume = drive_letter.unwrap_or('X');
    info!(volume = %volume, path = %mft_path.display(), "Loading raw MFT file");

    let t_load = std::time::Instant::now();

    // Check if this is an IOCP capture file (has UFFS-IOCP magic header)
    let is_iocp = uffs_mft::is_iocp_capture(mft_path).unwrap_or(false);

    let index = if let Some(seed) = chaos_seed {
        // Use ChaosMftReader for deterministic chaos-order testing
        // Works on all platforms when reading offline MFT files
        use uffs_mft::io::readers::parallel::{ChaosMftReader, ChaosStrategy};
        debug!(seed, "[PARITY_TRACE] CHAOS path");
        info!(
            seed = seed,
            "Loading MFT with chaos-order (randomized chunks)"
        );
        let chaos_reader = ChaosMftReader::new(
            ChaosStrategy::Random { seed },
            2 * 1024 * 1024, // 2MB chunks (same as test)
        );
        chaos_reader
            .read_with_chaos(mft_path, volume)
            .with_context(|| format!("Failed to load MFT in chaos mode: {}", mft_path.display()))?
    } else if is_iocp && !debug_tree {
        // IOCP capture: use load_iocp_to_index which replays the exact Windows LIVE
        // pipeline (parallel parse → MftRecordMerger → merge → from_parsed_records)
        // Skip this path if debug_tree is set (use sequential debug path instead)
        debug!("[PARITY_TRACE] IOCP path -> load_iocp_to_index()");
        info!("IOCP capture detected - using LIVE pipeline replay for exact parity");
        uffs_mft::load_iocp_to_index(mft_path)
            .with_context(|| format!("Failed to load IOCP capture: {}", mft_path.display()))?
    } else {
        let options = LoadRawOptions {
            volume_letter: Some(volume),
            ..Default::default()
        };

        if debug_tree {
            debug!("[PARITY_TRACE] DEBUG_TREE path");
            load_raw_mft_with_debug(mft_path, &options)?
        } else {
            debug!("[PARITY_TRACE] RAW MFT path -> load_raw_to_index_with_options()");
            MftReader::load_raw_to_index_with_options(mft_path, &options)
                .with_context(|| format!("Failed to load raw MFT: {}", mft_path.display()))?
        }
    };
    let load_ms = t_load.elapsed().as_millis();
    debug!(
        records = index.records.len(),
        load_ms, "[PARITY_TRACE] loaded"
    );

    let t_query = std::time::Instant::now();
    let results = execute_index_query_native(&index, filters, needs_paths)?;
    let query_ms = t_query.elapsed().as_millis();
    debug!(
        results = results.len(),
        query_ms, "[PARITY_TRACE] query returned"
    );

    Ok(NativeOfflineQueryResults {
        index,
        results,
        load_ms,
        query_ms,
    })
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

    println!();
    println!("Building MftIndex...");
    let mut index = MftIndex::from_parsed_records(raw.header.volume_letter, parsed_records);

    println!(
        "Index built: {} records, {} children entries",
        index.len(),
        index.children_count()
    );

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
#[tracing::instrument(
    level = "info",
    skip(index, multi_drives, filters),
    fields(
        has_index = index.is_some(),
        single_drive = ?single_drive,
        multi_drive_count = multi_drives.as_ref().map_or(0, Vec::len),
        needs_paths,
        profile,
        no_bitmap,
        files_only = filters.files_only,
        dirs_only = filters.dirs_only,
        hide_system = filters.hide_system,
        min_size = ?filters.min_size,
        max_size = ?filters.max_size,
        limit = filters.limit,
        has_ext_filter = filters.ext_filter.is_some()
    )
)]
pub(super) async fn load_and_filter_data(
    index: Option<PathBuf>,
    multi_drives: Option<Vec<char>>,
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_bitmap: bool,
) -> Result<uffs_mft::DataFrame> {
    if let Some(index_path) = index {
        let df = MftReader::load_parquet(&index_path)
            .with_context(|| format!("Failed to load index: {}", index_path.display()))?;
        return execute_query(df, filters);
    }

    if let Some(drives) = multi_drives {
        return super::search::search_multi_drive_filtered(
            &drives,
            filters,
            needs_paths,
            no_bitmap,
        )
        .await;
    }

    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    if let Some(drive_letter) = effective_drive {
        let t_read = std::time::Instant::now();
        tracing::trace!(drive = %drive_letter, "search_dataframe: before load_or_build_dataframe_cached");

        #[cfg(windows)]
        let full_df =
            uffs_mft::load_or_build_dataframe_cached(drive_letter, uffs_mft::INDEX_TTL_SECONDS)
                .await
                .with_context(|| format!("Failed to read MFT for drive {drive_letter}:"))?;

        tracing::trace!(drive = %drive_letter, "search_dataframe: after load_or_build_dataframe_cached");

        #[cfg(not(windows))]
        let full_df = {
            let reader = MftReader::open(drive_letter)
                .with_context(|| format!("Failed to open drive {drive_letter}:"))?
                .with_use_bitmap(!no_bitmap);
            reader.read_all()?
        };

        let read_ms = t_read.elapsed().as_millis();
        let total_records = full_df.height();

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

        let t_filter = std::time::Instant::now();
        let mut filtered = execute_query(full_df, filters)?;
        let filter_ms = t_filter.elapsed().as_millis();
        let filtered_count = filtered.height();

        let t_paths = std::time::Instant::now();
        if let Some(resolver) = &path_resolver {
            filtered = resolver
                .add_path_column_with_dir_suffix(&filtered)
                .context("Failed to add path column")?;
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
        super::search::search_multi_drive_filtered(&all_drives, filters, needs_paths, no_bitmap)
            .await
    }
    #[cfg(not(windows))]
    {
        bail!(
            "No drive specified. Use --drive, --drives, --index, or include drive in pattern (e.g., c:/pro*)"
        )
    }
}
/// Build and execute the MFT query with all filters applied.
#[tracing::instrument(
    level = "debug",
    skip(df, filters),
    fields(
        rows = df.height(),
        columns = df.width(),
        files_only = filters.files_only,
        dirs_only = filters.dirs_only,
        hide_system = filters.hide_system,
        min_size = ?filters.min_size,
        max_size = ?filters.max_size,
        limit = filters.limit,
        has_ext_filter = filters.ext_filter.is_some()
    )
)]
fn execute_query(
    df: uffs_mft::DataFrame,
    filters: &QueryFilters<'_>,
) -> Result<uffs_mft::DataFrame> {
    let mut query = MftQuery::new(df);

    query = query.pattern(filters.parsed)?;

    if let Some(ext_str) = filters.ext_filter {
        let parsed_ext_filter = ExtensionFilter::parse(ext_str)
            .map_err(|err| anyhow::anyhow!("Invalid extension filter: {err}"))?;
        query = query.extension_filter(&parsed_ext_filter);
    }

    if filters.files_only {
        query = query.files_only();
    } else if filters.dirs_only {
        query = query.directories_only();
    }

    if filters.hide_system {
        query = query.hide_system();
    }

    if let Some(min) = filters.min_size {
        query = query.min_size(min);
    }
    if let Some(max) = filters.max_size {
        query = query.max_size(max);
    }

    if filters.limit > 0 {
        query = query.limit(filters.limit);
    }
    Ok(query.collect()?)
}

/// Execute query using fast `IndexQuery` path (no `DataFrame` conversion).
///
/// This is the fast path for simple queries.
fn execute_index_query_native(
    index: &uffs_mft::MftIndex,
    filters: &QueryFilters<'_>,
    resolve_paths: bool,
) -> Result<Vec<uffs_core::SearchResult>> {
    use uffs_core::{IndexQuery, TypeFilter, compile_parsed_pattern};

    let mut query = IndexQuery::new(index);

    let pattern = compile_parsed_pattern(filters.parsed);
    query = query.with_pattern_result(pattern);

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

    if filters.files_only {
        query = query.with_type_filter(TypeFilter::FilesOnly);
    } else if filters.dirs_only {
        query = query.with_type_filter(TypeFilter::DirsOnly);
    }

    if let Some(min) = filters.min_size {
        query = query.min_size(min);
    }
    if let Some(max) = filters.max_size {
        query = query.max_size(max);
    }

    if filters.limit > 0 {
        query = query.limit(filters.limit as usize);
    }

    query = query.case_sensitive(filters.parsed.is_case_sensitive());
    query = query.with_resolve_paths(resolve_paths);

    Ok(query.collect())
}
