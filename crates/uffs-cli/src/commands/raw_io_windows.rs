//! Windows-specific multi-drive parallel MFT search helpers.

#![cfg(windows)]

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::task::JoinSet;
use tracing::info;
use uffs_core::extensions::ExtensionFilter;
use uffs_mft::{INDEX_TTL_SECONDS, MftReader};

use crate::commands::output::results_to_dataframe;
use crate::commands::raw_io::QueryFilters;

/// Owned version of `QueryFilters` for parallel tasks.
///
/// This struct owns all its data so it can be sent across thread boundaries.
#[derive(Clone)]
pub(crate) struct OwnedQueryFilters {
    /// Parsed search pattern (glob, regex, or literal).
    parsed: uffs_core::pattern::ParsedPattern,
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

impl OwnedQueryFilters {
    /// Create owned filters from borrowed filters.
    pub(crate) fn from_borrowed(filters: &QueryFilters<'_>) -> Self {
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
    pub(crate) fn execute(&self, df: uffs_mft::DataFrame) -> Result<uffs_mft::DataFrame> {
        use uffs_core::MftQuery;

        let mut query = MftQuery::new(df);

        query = query.pattern(&self.parsed)?;

        if let Some(ext_str) = &self.ext_filter {
            let parsed_ext_filter = ExtensionFilter::parse(ext_str)
                .map_err(|err| anyhow::anyhow!("Invalid extension filter: {err}"))?;
            query = query.extension_filter(&parsed_ext_filter);
        }

        if self.files_only {
            query = query.files_only();
        } else if self.dirs_only {
            query = query.directories_only();
        }

        if self.hide_system {
            query = query.hide_system();
        }

        if let Some(min) = self.min_size {
            query = query.min_size(min);
        }
        if let Some(max) = self.max_size {
            query = query.max_size(max);
        }

        Ok(query.collect()?)
    }
}

/// Execute query against an `MftIndex` and return results as a `DataFrame`.
fn execute_index_query(
    index: &uffs_mft::MftIndex,
    filters: &QueryFilters<'_>,
    resolve_paths: bool,
) -> Result<uffs_mft::DataFrame> {
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

    let results = query.collect();
    results_to_dataframe(index, results, resolve_paths)
}

/// Load and filter data using fast `MftIndex` path (no `DataFrame` conversion
/// during search).
#[expect(clippy::single_call_fn, reason = "extracted from search() for clarity")]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
pub(crate) async fn load_and_filter_data_index(
    single_drive: Option<char>,
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_cache: bool,
) -> Result<uffs_mft::DataFrame> {
    let effective_drive = single_drive.or_else(|| filters.parsed.drive());
    let drive_letter = effective_drive.ok_or_else(|| {
        anyhow::anyhow!(
            "Index query mode requires a specific drive. Use --drive or include drive in pattern."
        )
    })?;

    let t_load = std::time::Instant::now();

    let reader = MftReader::open(drive_letter)
        .with_context(|| format!("Failed to open drive {drive_letter}:"))?;

    let index = if no_cache {
        info!(drive = %drive_letter, "🔄 --no-cache: reading MFT fresh");
        reader.read_all_index().await?
    } else {
        reader.read_index_cached(INDEX_TTL_SECONDS).await?
    };
    let load_ms = t_load.elapsed().as_millis();

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
#[expect(
    clippy::single_call_fn,
    reason = "extracted for multi-drive parallel search"
)]
#[expect(
    clippy::print_stderr,
    reason = "intentional profiling output to stderr"
)]
pub(crate) async fn load_and_filter_data_index_multi(
    drives: &[char],
    filters: &QueryFilters<'_>,
    needs_paths: bool,
    profile: bool,
    no_cache: bool,
) -> Result<uffs_mft::DataFrame> {
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
    let owned_filters = Arc::new(OwnedQueryFilters::from_borrowed(filters));

    let mut join_set: JoinSet<Result<(char, uffs_mft::DataFrame, u128, u128, usize)>> =
        JoinSet::new();

    for &drive in drives {
        let filters = Arc::clone(&owned_filters);

        join_set.spawn(async move {
            let t_load = std::time::Instant::now();

            let reader =
                MftReader::open(drive).with_context(|| format!("Failed to open drive {drive}:"))?;

            let index = if no_cache {
                info!(drive = %drive, "🔄 --no-cache: reading MFT fresh");
                reader.read_all_index().await?
            } else {
                reader.read_index_cached(INDEX_TTL_SECONDS).await?
            };
            let load_ms = t_load.elapsed().as_millis();
            let record_count = index.len();

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

    let mut all_results: Vec<uffs_mft::DataFrame> = Vec::with_capacity(drives.len());
    let mut total_records = 0usize;
    let mut total_matches = 0usize;

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok((drive, df, load_ms, query_ms, record_count))) => {
                let matches = df.height();
                if profile {
                    eprintln!(
                        "  Drive {drive}: {record_count} records, {matches} matches \
                         (load: {load_ms}ms, query: {query_ms}ms)"
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

    if all_results.is_empty() {
        return Ok(uffs_mft::DataFrame::empty());
    }

    if all_results.len() == 1 {
        return Ok(all_results.remove(0));
    }

    use uffs_polars::IntoLazy;
    let lazy_frames: Vec<uffs_polars::LazyFrame> =
        all_results.into_iter().map(|df| df.lazy()).collect();
    let combined = uffs_polars::concat(&lazy_frames, uffs_polars::UnionArgs::default())
        .context("Failed to combine results from multiple drives")?
        .collect()
        .context("Failed to collect combined results")?;

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

