//! Output helpers for CLI search commands.

extern crate alloc;

use alloc::borrow::Cow;
use core::fmt::Write as _;
use core::time::Duration;
use std::fs::File;
use std::io::{BufWriter, Write};

use anyhow::{Context, Result};
use tracing::info;
use uffs_core::output::{CPP_COLUMN_ORDER, OutputColumn, OutputConfig};
use uffs_core::{export_json, export_table};

#[cfg(windows)]
#[path = "streaming.rs"]
mod streaming;
#[cfg(windows)]
pub(crate) use streaming::StreamingWriter;
#[cfg(windows)]
pub(crate) use streaming::{format_json_string, format_json_value};

// For tests on non-Windows, we need the JSON helpers too
#[cfg(all(test, not(windows)))]
#[path = "json_helpers.rs"]
mod json_helpers;
#[cfg(all(test, not(windows)))]
pub(super) use json_helpers::format_json_value;

/// Context for C++ baseline-compatible footer formatting.
pub(super) struct CppFooterContext<'a> {
    /// Drive letters to include in the footer (e.g., `['C', 'D']`).
    pub(super) output_targets: &'a [char],
    /// Original search pattern string.
    pub(super) pattern: &'a str,
    /// Total result row count for fast-scan heuristic.
    pub(super) row_count: usize,
}

/// Return whether the offline native results can be written directly without a
/// compatibility `DataFrame`.
#[must_use]
pub(super) fn can_write_native_results(format: &str, output_config: &OutputConfig) -> bool {
    matches!(format.to_ascii_lowercase().as_str(), "csv" | "custom")
        && !selected_output_columns(output_config).contains(&OutputColumn::Bulkiness)
}

/// Write native `IndexQuery` results directly for the offline `--mft-file`
/// output path.
pub(super) fn write_native_results(
    index: &uffs_mft::MftIndex,
    results: &[uffs_core::SearchResult],
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<()> {
    let normalized_format = format.to_ascii_lowercase();
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout_handle = std::io::stdout();
        let mut stdout = stdout_handle.lock();
        write_native_results_to(
            index,
            results,
            &normalized_format,
            &mut stdout,
            output_config,
            footer_ctx,
        )?;
        stdout.flush()?;
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = BufWriter::new(file);
        write_native_results_to(
            index,
            results,
            &normalized_format,
            &mut writer,
            output_config,
            footer_ctx,
        )?;
        writer.flush()?;

        info!(file = out, "Results written to file");
    }

    Ok(())
}

/// Write native offline results to the provided writer.
fn write_native_results_to<W: Write>(
    index: &uffs_mft::MftIndex,
    results: &[uffs_core::SearchResult],
    format: &str,
    writer: &mut W,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<()> {
    let output_cols = selected_output_columns(output_config);
    let fixed_tz = chrono::FixedOffset::east_opt(output_config.timezone_offset_secs);

    write_native_header(writer, output_config, output_cols)?;

    let mut row_buffer = String::with_capacity(output_cols.len() * 32);
    for result in results {
        row_buffer.clear();
        let record = index.find(result.frs);
        let tree_metrics = native_tree_metrics(result, record);

        for (idx, col) in output_cols.iter().enumerate() {
            if idx > 0 {
                row_buffer.push_str(&output_config.separator);
            }
            write_native_value(
                &mut row_buffer,
                output_config,
                fixed_tz.as_ref(),
                index,
                result,
                record,
                tree_metrics,
                *col,
            );
        }

        row_buffer.push('\n');
        writer.write_all(row_buffer.as_bytes())?;
    }

    if format == "custom" {
        write_cpp_drive_footer(writer, footer_ctx)?;
    }

    Ok(())
}

/// Write the configured header for direct native output.
fn write_native_header<W: Write>(
    writer: &mut W,
    output_config: &OutputConfig,
    output_cols: &[OutputColumn],
) -> Result<()> {
    if !output_config.header {
        return Ok(());
    }

    let mut header = String::with_capacity(output_cols.len() * 24);
    for (idx, col) in output_cols.iter().enumerate() {
        if idx > 0 {
            header.push_str(&output_config.separator);
        }
        header.push_str(&output_config.quote);
        header.push_str(col.display_name());
        header.push_str(&output_config.quote);
    }
    header.push('\n');
    header.push('\n');
    writer.write_all(header.as_bytes())?;
    Ok(())
}

/// Return the effective output columns for the current configuration.
#[must_use]
fn selected_output_columns(output_config: &OutputConfig) -> &[OutputColumn] {
    output_config.columns.as_deref().unwrap_or(CPP_COLUMN_ORDER)
}

/// Write a single native value using the same formatting semantics as the
/// `DataFrame` output path.
#[expect(
    clippy::too_many_arguments,
    reason = "direct native writer carries the same row context as the legacy path"
)]
#[expect(
    clippy::too_many_lines,
    reason = "matches the existing full output schema column-by-column"
)]
fn write_native_value(
    row_buffer: &mut String,
    output_config: &OutputConfig,
    fixed_tz: Option<&chrono::FixedOffset>,
    index: &uffs_mft::MftIndex,
    result: &uffs_core::SearchResult,
    record: Option<&uffs_mft::index::FileRecord>,
    tree_metrics: (u32, u64, u64),
    column: OutputColumn,
) {
    match column {
        OutputColumn::Path => append_quoted(row_buffer, &output_config.quote, result_path(result)),
        OutputColumn::Name => append_quoted(row_buffer, &output_config.quote, &result.name),
        OutputColumn::PathOnly => append_quoted(
            row_buffer,
            &output_config.quote,
            path_only_from_path(result_path(result)),
        ),
        OutputColumn::Size => append_display(row_buffer, displayed_size(result, tree_metrics)),
        OutputColumn::SizeOnDisk => {
            append_display(row_buffer, displayed_allocated_size(result, tree_metrics));
        }
        OutputColumn::Created => append_datetime(
            row_buffer,
            record.map_or(0, |rec| rec.stdinfo.created),
            fixed_tz,
        ),
        OutputColumn::Modified => append_datetime(
            row_buffer,
            record.map_or(0, |rec| rec.stdinfo.modified),
            fixed_tz,
        ),
        OutputColumn::Accessed => append_datetime(
            row_buffer,
            record.map_or(0, |rec| rec.stdinfo.accessed),
            fixed_tz,
        ),
        OutputColumn::Type => append_quoted(
            row_buffer,
            &output_config.quote,
            native_file_type(index, result, record).as_ref(),
        ),
        OutputColumn::Attributes | OutputColumn::AttributeValue => append_display(
            row_buffer,
            record.map_or(0_u32, |rec| rec.stdinfo.to_attributes()),
        ),
        OutputColumn::Hidden => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_hidden()),
        ),
        OutputColumn::System => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_system()),
        ),
        OutputColumn::Archive => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_archive()),
        ),
        OutputColumn::ReadOnly => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_readonly()),
        ),
        OutputColumn::Compressed => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_compressed()),
        ),
        OutputColumn::Encrypted => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_encrypted()),
        ),
        OutputColumn::Sparse => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_sparse()),
        ),
        OutputColumn::Reparse => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_reparse()),
        ),
        OutputColumn::Offline => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_offline()),
        ),
        OutputColumn::NotIndexed => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_not_indexed()),
        ),
        OutputColumn::Temporary => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_temporary()),
        ),
        OutputColumn::Virtual => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_virtual()),
        ),
        OutputColumn::Pinned => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_pinned()),
        ),
        OutputColumn::Unpinned => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_unpinned()),
        ),
        OutputColumn::Descendants => append_display(row_buffer, tree_metrics.0),
        OutputColumn::TreeSize => append_display(row_buffer, tree_metrics.1),
        OutputColumn::TreeAllocated => append_display(row_buffer, tree_metrics.2),
        OutputColumn::Bulkiness => row_buffer.push_str(OutputColumn::Bulkiness.default_value()),
        OutputColumn::Integrity => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_integrity_stream()),
        ),
        OutputColumn::NoScrub => append_bool(
            row_buffer,
            output_config,
            record.is_some_and(|rec| rec.stdinfo.is_no_scrub_data()),
        ),
        OutputColumn::DirectoryFlag => append_bool(
            row_buffer,
            output_config,
            record.map_or(result.is_directory, uffs_mft::FileRecord::is_directory),
        ),
    }
}

/// Return the output path string for a native search result.
#[must_use]
fn result_path(result: &uffs_core::SearchResult) -> &str {
    result.path.as_deref().unwrap_or_default()
}

/// Return the parent-directory portion of a path, including the trailing
/// backslash when present.
#[must_use]
fn path_only_from_path(path: &str) -> &str {
    path.rfind('\\')
        .and_then(|last_sep| path.get(..=last_sep))
        .unwrap_or_default()
}

/// Compute the file-type string using the same metadata source as the
/// compatibility `DataFrame` path.
#[must_use]
fn native_file_type<'a>(
    index: &'a uffs_mft::MftIndex,
    result: &'a uffs_core::SearchResult,
    record: Option<&'a uffs_mft::index::FileRecord>,
) -> Cow<'a, str> {
    if let Some(rec) = record {
        let ext_id = rec.first_name.name.extension_id();
        return Cow::Borrowed(index.extensions.get_extension(ext_id).unwrap_or(""));
    }

    Cow::Owned(
        result
            .name
            .rfind('.')
            .and_then(|pos| {
                if pos > 0 && pos < result.name.len() - 1 {
                    result.name.get(pos + 1..)
                } else {
                    None
                }
            })
            .map(str::to_lowercase)
            .unwrap_or_default(),
    )
}

/// Compute descendants/tree metrics for the output row.
#[must_use]
#[expect(
    clippy::missing_const_for_fn,
    reason = "kept non-const for readability alongside the other row helpers"
)]
fn native_tree_metrics(
    result: &uffs_core::SearchResult,
    record: Option<&uffs_mft::index::FileRecord>,
) -> (u32, u64, u64) {
    if result.stream_index > 0 {
        (0, 0, 0)
    } else if let Some(rec) = record {
        rec.tree_metrics()
    } else {
        (result.descendants, result.treesize, result.tree_allocated)
    }
}

/// Return the displayed size after applying directory treesize semantics.
#[must_use]
fn displayed_size(result: &uffs_core::SearchResult, tree_metrics: (u32, u64, u64)) -> u64 {
    if result.is_directory && result.stream_name.is_empty() {
        tree_metrics.1
    } else {
        result.size
    }
}

/// Return the displayed allocated size after applying directory treesize
/// semantics.
#[must_use]
fn displayed_allocated_size(
    result: &uffs_core::SearchResult,
    tree_metrics: (u32, u64, u64),
) -> u64 {
    if result.is_directory && result.stream_name.is_empty() {
        tree_metrics.2
    } else {
        result.allocated_size
    }
}

/// Append a quoted string field.
fn append_quoted(row_buffer: &mut String, quote: &str, value: &str) {
    row_buffer.push_str(quote);
    row_buffer.push_str(value);
    row_buffer.push_str(quote);
}

/// Append a boolean field using the configured positive/negative strings.
fn append_bool(row_buffer: &mut String, output_config: &OutputConfig, value: bool) {
    if value {
        row_buffer.push_str(&output_config.pos);
    } else {
        row_buffer.push_str(&output_config.neg);
    }
}

/// Append a datetime field using the same fixed-offset formatting as the
/// `DataFrame` writer.
fn append_datetime(
    row_buffer: &mut String,
    timestamp_micros: i64,
    fixed_tz: Option<&chrono::FixedOffset>,
) {
    let secs = timestamp_micros.div_euclid(1_000_000);
    let micros = u32::try_from(timestamp_micros.rem_euclid(1_000_000)).unwrap_or(0);
    if let Some(utc_dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
        if let Some(timezone_offset) = fixed_tz {
            append_display(
                row_buffer,
                utc_dt
                    .with_timezone(timezone_offset)
                    .format("%Y-%m-%d %H:%M:%S"),
            );
        } else {
            append_display(row_buffer, utc_dt.format("%Y-%m-%d %H:%M:%S"));
        }
    }
}

/// Append a displayable value without introducing extra string allocations in
/// the common case.
fn append_display<T>(row_buffer: &mut String, value: T)
where
    T: core::fmt::Display,
{
    if row_buffer.write_fmt(format_args!("{value}")).is_err() {
        row_buffer.push_str(&value.to_string());
    }
}

/// Convert `IndexQuery` results to a `DataFrame` for output compatibility.
///
/// **TEMPORARY**: This function exists only for compatibility with the current
/// output pipeline which expects a `DataFrame`. The proper solution is to
/// output directly from `SearchResults` without `DataFrame` conversion.
///
/// TODO: Remove this function and output directly from `SearchResults` +
/// `MftIndex`.
#[expect(
    clippy::too_many_lines,
    reason = "builds the full output schema with 30+ columns"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short names (e.g. df) conventional in DataFrame-heavy code"
)]
pub(super) fn results_to_dataframe(
    index: &uffs_mft::MftIndex,
    results: Vec<uffs_core::SearchResult>,
    _resolve_paths: bool,
) -> Result<uffs_mft::DataFrame> {
    use uffs_polars::{DataType, IntoColumn, NamedFrom, Series, TimeUnit};

    let height = results.len();

    let mut frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut parent_frs_values: Vec<u64> = Vec::with_capacity(height);
    let mut names: Vec<String> = Vec::with_capacity(height);
    let mut file_types: Vec<String> = Vec::with_capacity(height);
    let mut paths: Vec<String> = Vec::with_capacity(height);
    let mut sizes: Vec<u64> = Vec::with_capacity(height);
    let mut allocated_sizes: Vec<u64> = Vec::with_capacity(height);
    let mut created_times: Vec<i64> = Vec::with_capacity(height);
    let mut modified_times: Vec<i64> = Vec::with_capacity(height);
    let mut accessed_times: Vec<i64> = Vec::with_capacity(height);
    let mut mft_changed_times: Vec<i64> = Vec::with_capacity(height);
    let mut is_dirs: Vec<bool> = Vec::with_capacity(height);
    let mut is_readonly: Vec<bool> = Vec::with_capacity(height);
    let mut is_hidden: Vec<bool> = Vec::with_capacity(height);
    let mut is_system: Vec<bool> = Vec::with_capacity(height);
    let mut is_archive: Vec<bool> = Vec::with_capacity(height);
    let mut is_compressed: Vec<bool> = Vec::with_capacity(height);
    let mut is_encrypted: Vec<bool> = Vec::with_capacity(height);
    let mut is_sparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_reparse: Vec<bool> = Vec::with_capacity(height);
    let mut is_offline: Vec<bool> = Vec::with_capacity(height);
    let mut is_not_indexed: Vec<bool> = Vec::with_capacity(height);
    let mut is_temporary: Vec<bool> = Vec::with_capacity(height);
    let mut is_integrity: Vec<bool> = Vec::with_capacity(height);
    let mut is_no_scrub: Vec<bool> = Vec::with_capacity(height);
    let mut is_pinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_unpinned: Vec<bool> = Vec::with_capacity(height);
    let mut is_virtual: Vec<bool> = Vec::with_capacity(height);
    let mut flags_values: Vec<u32> = Vec::with_capacity(height);

    let mut descendants_values: Vec<u32> = Vec::with_capacity(height);
    let mut treesize_values: Vec<u64> = Vec::with_capacity(height);
    let mut tree_allocated_values: Vec<u64> = Vec::with_capacity(height);
    let mut stream_names: Vec<String> = Vec::with_capacity(height);

    for result in results {
        let record = index.find(result.frs);
        let file_type = if let Some(rec) = record {
            let ext_id = rec.first_name.name.extension_id();
            index
                .extensions
                .get_extension(ext_id)
                .unwrap_or("")
                .to_owned()
        } else {
            result
                .name
                .rfind('.')
                .and_then(|pos| {
                    if pos > 0 && pos < result.name.len() - 1 {
                        result.name.get(pos + 1..)
                    } else {
                        None
                    }
                })
                .map(str::to_lowercase)
                .unwrap_or_default()
        };

        frs_values.push(result.frs);
        parent_frs_values.push(result.parent_frs);
        paths.push(result.path.unwrap_or_default());
        sizes.push(result.size);
        stream_names.push(result.stream_name);
        names.push(result.name);

        file_types.push(file_type);

        if let Some(rec) = record {
            allocated_sizes.push(result.allocated_size);
            created_times.push(rec.stdinfo.created);
            modified_times.push(rec.stdinfo.modified);
            accessed_times.push(rec.stdinfo.accessed);
            mft_changed_times.push(rec.stdinfo.mft_changed);
            is_dirs.push(rec.is_directory());
            is_readonly.push(rec.stdinfo.is_readonly());
            is_hidden.push(rec.stdinfo.is_hidden());
            is_system.push(rec.stdinfo.is_system());
            is_archive.push(rec.stdinfo.is_archive());
            is_compressed.push(rec.stdinfo.is_compressed());
            is_encrypted.push(rec.stdinfo.is_encrypted());
            is_sparse.push(rec.stdinfo.is_sparse());
            is_reparse.push(rec.stdinfo.is_reparse());
            is_offline.push(rec.stdinfo.is_offline());
            is_not_indexed.push(rec.stdinfo.is_not_indexed());
            is_temporary.push(rec.stdinfo.is_temporary());
            is_integrity.push(rec.stdinfo.is_integrity_stream());
            is_no_scrub.push(rec.stdinfo.is_no_scrub_data());
            is_pinned.push(rec.stdinfo.is_pinned());
            is_unpinned.push(rec.stdinfo.is_unpinned());
            is_virtual.push(rec.stdinfo.is_virtual());
            flags_values.push(rec.stdinfo.to_attributes());
        } else {
            allocated_sizes.push(0);
            created_times.push(0);
            modified_times.push(0);
            accessed_times.push(0);
            mft_changed_times.push(0);
            is_dirs.push(result.is_directory);
            is_readonly.push(false);
            is_hidden.push(false);
            is_system.push(false);
            is_archive.push(false);
            is_compressed.push(false);
            is_encrypted.push(false);
            is_sparse.push(false);
            is_reparse.push(false);
            is_offline.push(false);
            is_not_indexed.push(false);
            is_temporary.push(false);
            is_integrity.push(false);
            is_no_scrub.push(false);
            is_pinned.push(false);
            is_unpinned.push(false);
            is_virtual.push(false);
            flags_values.push(0);
        }

        let (desc, tsize, talloc) = if result.stream_index > 0 {
            (0_u32, 0_u64, 0_u64)
        } else if let Some(rec) = record {
            rec.tree_metrics()
        } else {
            (result.descendants, result.treesize, result.tree_allocated)
        };
        descendants_values.push(desc);
        treesize_values.push(tsize);
        tree_allocated_values.push(talloc);
    }

    let columns = vec![
        Series::new("frs".into(), frs_values).into_column(),
        Series::new("parent_frs".into(), parent_frs_values).into_column(),
        Series::new("name".into(), names).into_column(),
        Series::new("type".into(), file_types).into_column(),
        Series::new("path".into(), paths).into_column(),
        Series::new("size".into(), sizes).into_column(),
        Series::new("allocated_size".into(), allocated_sizes).into_column(),
        Series::new("created".into(), created_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast created column: {e}"))?
            .into_column(),
        Series::new("modified".into(), modified_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast modified column: {e}"))?
            .into_column(),
        Series::new("accessed".into(), accessed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast accessed column: {e}"))?
            .into_column(),
        Series::new("mft_changed".into(), mft_changed_times)
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .map_err(|e| anyhow::anyhow!("Failed to cast mft_changed column: {e}"))?
            .into_column(),
        Series::new("is_directory".into(), is_dirs).into_column(),
        Series::new("is_readonly".into(), is_readonly).into_column(),
        Series::new("is_hidden".into(), is_hidden).into_column(),
        Series::new("is_system".into(), is_system).into_column(),
        Series::new("is_archive".into(), is_archive).into_column(),
        Series::new("is_compressed".into(), is_compressed).into_column(),
        Series::new("is_encrypted".into(), is_encrypted).into_column(),
        Series::new("is_sparse".into(), is_sparse).into_column(),
        Series::new("is_reparse".into(), is_reparse).into_column(),
        Series::new("is_offline".into(), is_offline).into_column(),
        Series::new("is_not_indexed".into(), is_not_indexed).into_column(),
        Series::new("is_temporary".into(), is_temporary).into_column(),
        Series::new("is_integrity_stream".into(), is_integrity).into_column(),
        Series::new("is_no_scrub_data".into(), is_no_scrub).into_column(),
        Series::new("is_pinned".into(), is_pinned).into_column(),
        Series::new("is_unpinned".into(), is_unpinned).into_column(),
        Series::new("is_virtual".into(), is_virtual).into_column(),
        Series::new("flags".into(), flags_values).into_column(),
        Series::new("descendants".into(), descendants_values).into_column(),
        Series::new("treesize".into(), treesize_values).into_column(),
        Series::new("tree_allocated".into(), tree_allocated_values).into_column(),
        Series::new("stream_name".into(), stream_names).into_column(),
    ];

    let mut df = uffs_mft::DataFrame::new_infer_height(columns)
        .map_err(|err| anyhow::anyhow!("Failed to create DataFrame: {err}"))?;

    df = tokio::task::block_in_place(|| uffs_core::apply_directory_treesize(&df))
        .map_err(|err| anyhow::anyhow!("Failed to apply directory treesize: {err}"))?;

    df = uffs_core::add_path_only_column(&df)
        .map_err(|err| anyhow::anyhow!("Failed to add path_only column: {err}"))?;

    Ok(df)
}

/// Write search results to console or file.
pub(super) fn write_results(
    results: &uffs_mft::DataFrame,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
    _elapsed: Duration,
    pattern: &str,
) -> Result<()> {
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    let row_count = results.height();

    let footer_ctx = CppFooterContext {
        output_targets,
        pattern,
        row_count,
    };

    if is_console {
        let stdout_handle = std::io::stdout();
        let mut stdout = stdout_handle.lock();
        match format {
            "json" => export_json(results, &mut stdout)?,
            "csv" => output_config.write(results, &mut stdout)?,
            "custom" => {
                output_config.write(results, &mut stdout)?;
                write_cpp_drive_footer(&mut stdout, &footer_ctx)?;
            }
            _ => export_table(results, &mut stdout)?,
        }
        stdout.flush()?;
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = BufWriter::new(file);

        match format {
            "json" => export_json(results, &mut writer)?,
            "custom" => {
                output_config.write(results, &mut writer)?;
                write_cpp_drive_footer(&mut writer, &footer_ctx)?;
            }
            _ => output_config.write(results, &mut writer)?,
        }
        writer.flush()?;

        info!(file = out, "Results written to file");
    }

    Ok(())
}

/// Append the legacy C++ drive footer for baseline-compatible custom output.
///
/// Uses CRLF line endings (`\r\n`) to match C++ baseline behavior.
/// When `row_count` is < 20,000, appends the fast-scan message.
fn write_cpp_drive_footer<W: Write>(writer: &mut W, ctx: &CppFooterContext<'_>) -> Result<()> {
    if ctx.output_targets.is_empty() {
        return Ok(());
    }

    write!(writer, "\r\n")?;
    write!(writer, "\r\n")?;
    write!(
        writer,
        "Drives? \t{}\t{}\r\n",
        ctx.output_targets.len(),
        format_cpp_drive_letters(ctx.output_targets)
    )?;
    write!(writer, "\r\n")?;

    if ctx.row_count < 20_000 {
        write!(
            writer,
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t{pattern}\r\n",
            pattern = ctx.pattern
        )?;
        write!(writer, "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n")?;
    }

    Ok(())
}

/// Format drive letters using the legacy C++ footer style (for example `D:` or
/// `C:|D:`).
#[must_use]
fn format_cpp_drive_letters(output_targets: &[char]) -> String {
    output_targets
        .iter()
        .map(|drive| format!("{}:", drive.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
