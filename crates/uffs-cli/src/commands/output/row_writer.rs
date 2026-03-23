//! Streaming row writer: single-pass output from `MftIndex` to `Write`.

use std::io::Write;

use anyhow::Result;
use uffs_core::output::{OutputColumn, OutputConfig};

use super::CppFooterContext;
use super::filter::compare_records;
use super::types::StreamingRecordFilter;

/// Core streaming writer with optional pattern filter and optional record
/// indices.
///
/// - `pattern = None`: write ALL records (full scan `*`)
/// - `pattern = Some(pat)`: write only records whose name matches `pat`
/// - `record_indices = Some(indices)`: only visit these records (extension
///   index)
/// - `record_indices = None`: scan all records sequentially
#[expect(
    clippy::too_many_lines,
    reason = "single-pass streaming writer needs inline path + row logic"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "unified streaming writer accepting all filter options"
)]
#[expect(
    clippy::cognitive_complexity,
    reason = "flat column-match dispatch + filter checks — structurally simple, just many branches"
)]
pub(in crate::commands) fn write_index_streaming_with_filter<W: Write + ?Sized>(
    index: &uffs_mft::MftIndex,
    pattern: Option<&uffs_core::IndexPattern>,
    record_indices: Option<&[u32]>,
    case_sensitive: bool,
    is_path_pattern: bool,
    record_filter: &StreamingRecordFilter,
    writer: &mut W,
    format: &str,
    output_config: &OutputConfig,
    footer_ctx: &CppFooterContext<'_>,
) -> Result<usize> {
    use uffs_mft::index::{PathCache, PathResolver};

    // For filtered queries with an extension-index shortlist, skip the
    // expensive pre_cache_directory_paths() pass.  That pass materialises a
    // String for every valid directory (O(n), ~500K dirs, ~20 MB) which is
    // wasteful when only a handful of records will actually be output.
    // The PathResolver alone (O(n) state vec, cheap) is always needed for
    // validity checks; materialize_path_into gracefully falls back to a
    // parent-chain walk when the dir_cache is empty.
    const LAZY_THRESHOLD: usize = 100_000;

    let output_cols = super::selected_output_columns(output_config);
    let tz_offset_secs = output_config.timezone_offset_secs;

    let t_cache = std::time::Instant::now();
    let use_lazy = record_indices.is_some_and(|ri| ri.len() < LAZY_THRESHOLD);

    let path_cache_storage; // owns the full PathCache when needed
    let resolver_storage; // owns the lightweight resolver when lazy
    let empty_dir_cache = Vec::new();

    let (resolver, dir_cache): (&PathResolver, &[String]) = if use_lazy {
        resolver_storage = PathResolver::build(index, false);
        (&resolver_storage, &empty_dir_cache)
    } else {
        path_cache_storage = PathCache::build(index, false);
        (
            path_cache_storage.resolver(),
            path_cache_storage.dir_cache(),
        )
    };

    let cache_ms = t_cache.elapsed().as_millis();
    tracing::info!(cache_ms, use_lazy, "📊 streaming: path resolver built");

    write_native_header(writer, output_config, output_cols)?;

    let mut row_buffer = String::with_capacity(512);
    let mut path_buffer = String::with_capacity(256);
    let mut hardlink_buf = String::new();
    let mut itoa_buf = itoa::Buffer::new();
    let mut row_count: usize = 0;
    let t_rows = std::time::Instant::now();

    // If sorting is requested, collect matching record indices using Top-K
    // heap (when limit is set) or full collect+sort (when unlimited).
    // Default sort limit: 200 rows to avoid collecting millions of records.
    let sorted_indices: Option<Vec<u32>> = (!record_filter.sort_spec.is_empty()).then(|| {
        let effective_limit = if record_filter.limit > 0 {
            record_filter.limit
        } else {
            200 // default sort limit to avoid collecting millions of records
        };

        let sort_spec = &record_filter.sort_spec;
        let desc = record_filter.sort_desc;

        let base_iter: Box<dyn Iterator<Item = (usize, &uffs_mft::index::FileRecord)>> =
            if let Some(indices) = record_indices {
                Box::new(indices.iter().filter_map(|&idx_u32| {
                    let idx = idx_u32 as usize;
                    index.records.get(idx).map(|rec| (idx, rec))
                }))
            } else {
                Box::new(index.records.iter().enumerate())
            };

        // Collect matching record indices, then use select_nth_unstable_by
        // (introselect) for O(n) average Top-K selection instead of O(n log n) full
        // sort.
        let mut matching: Vec<u32> = Vec::new();
        for (record_idx, record) in base_iter {
            if !resolver.is_valid_idx(record_idx) {
                continue;
            }
            if !record_filter.matches(record) {
                continue;
            }
            if let Some(pat) = pattern {
                let matches = if is_path_pattern {
                    path_buffer.clear();
                    resolver.materialize_path_into(index, record_idx, dir_cache, &mut path_buffer);
                    pat.matches(&path_buffer, case_sensitive)
                } else {
                    pat.matches(index.record_name(record), case_sensitive)
                };
                if !matches {
                    continue;
                }
            }
            if let Some(excl) = &record_filter.exclude_pattern {
                if excl.matches(index.record_name(record), case_sensitive) {
                    continue;
                }
            }
            matching.push(u32::try_from(record_idx).unwrap_or(u32::MAX));
        }

        // Partial sort: if we have more matches than the limit, use
        // select_nth_unstable_by to find the top-K in O(n) average,
        // then sort only those K entries in O(k log k).
        if matching.len() > effective_limit {
            matching.select_nth_unstable_by(effective_limit, |&idx_a, &idx_b| {
                compare_records(idx_a as usize, idx_b as usize, index, sort_spec, desc)
            });
            matching.truncate(effective_limit);
        }

        // Final sort of the top-K entries.
        matching.sort_unstable_by(|&idx_a, &idx_b| {
            compare_records(idx_a as usize, idx_b as usize, index, sort_spec, desc)
        });

        matching
    });

    // Build the final iterator: sorted indices or original scan order.
    let record_iter: Box<dyn Iterator<Item = (usize, &uffs_mft::index::FileRecord)>> =
        if let Some(sorted) = &sorted_indices {
            // Sorted path: iterate pre-filtered, pre-sorted indices.
            Box::new(sorted.iter().filter_map(|&idx_u32| {
                let idx = idx_u32 as usize;
                index.records.get(idx).map(|rec| (idx, rec))
            }))
        } else if let Some(indices) = record_indices {
            Box::new(indices.iter().filter_map(|&idx_u32| {
                let idx = idx_u32 as usize;
                index.records.get(idx).map(|rec| (idx, rec))
            }))
        } else {
            Box::new(index.records.iter().enumerate())
        };

    for (record_idx, record) in record_iter {
        if !resolver.is_valid_idx(record_idx) {
            continue;
        }

        // Apply attribute filters (files_only, dirs_only, hide_system, size).
        if !record_filter.matches(record) {
            continue;
        }

        let is_directory = record.is_directory();

        // Resolve primary path into reusable buffer (zero per-record allocation).
        path_buffer.clear();
        resolver.materialize_path_into(index, record_idx, dir_cache, &mut path_buffer);

        // Apply pattern filter: match against full path or filename.
        if let Some(pat) = pattern {
            if is_path_pattern {
                if !pat.matches(&path_buffer, case_sensitive) {
                    continue;
                }
            } else {
                let name = index.record_name(record);
                if !pat.matches(name, case_sensitive) {
                    continue;
                }
            }
        }

        // Apply exclude pattern (reject matches).
        if let Some(excl) = &record_filter.exclude_pattern {
            let name = index.record_name(record);
            if excl.matches(name, case_sensitive) {
                continue;
            }
        }

        // Check limit — stop early if we've reached the max.
        if record_filter.limit > 0 && row_count >= record_filter.limit {
            break;
        }

        // Expand names × streams (same logic as RecordExpander).
        let name_count = record.name_count.max(1);
        let stream_count = record.stream_count.max(1);

        for name_idx in 0..name_count {
            for stream_idx in 0..stream_count {
                let Some(stream_info) = index.get_stream_at(record, stream_idx) else {
                    continue;
                };
                if !stream_info.is_output_stream() {
                    continue;
                }

                // Build the display name.
                let name_info = index
                    .get_name_at(record, name_idx)
                    .unwrap_or(&record.first_name);
                let stream_name = index.stream_name(stream_info);
                let has_ads = !stream_name.is_empty();
                let base_name = index.get_name(&name_info.name);

                // Path base: use path_buffer for primary name, resolve
                // alternate for hardlinks. NEVER mutate path_buffer in this
                // inner loop — it's shared across stream iterations.
                let base_path: &str = if name_idx == 0 {
                    &path_buffer
                } else {
                    // Hard link — resolve via alternate parent (rare, <1%).
                    hardlink_buf.clear();
                    let alt = resolver.materialize_path_for_name(index, record_idx, name_idx);
                    hardlink_buf.push_str(&alt);
                    &hardlink_buf
                };
                // Whether this directory path needs a trailing backslash.
                let dir_needs_sep = is_directory && !has_ads && !base_path.ends_with('\\');

                // Determine tree metrics and displayed sizes.
                let (descendants, treesize, tree_allocated) = if stream_idx == 0 {
                    record.tree_metrics()
                } else {
                    (0, 0, 0)
                };
                let displayed_size = if is_directory && !has_ads {
                    treesize
                } else {
                    stream_info.size.length
                };
                let displayed_alloc = if is_directory && !has_ads {
                    tree_allocated
                } else {
                    stream_info.size.allocated
                };

                // Display name: dirs get empty name for default stream.
                let display_name: &str = if is_directory && !has_ads {
                    ""
                } else if has_ads {
                    // Inline "base:stream" — avoid allocation by writing
                    // directly during column output below.
                    ""
                } else {
                    base_name
                };

                // Path-only (parent directory portion including trailing \).
                let path_only: &str = if is_directory && !has_ads {
                    base_path
                } else {
                    base_path
                        .rfind('\\')
                        .and_then(|pos| base_path.get(..=pos))
                        .unwrap_or_default()
                };

                // Build row.
                row_buffer.clear();
                write_row_columns(
                    &mut row_buffer,
                    output_cols,
                    output_config,
                    &mut itoa_buf,
                    base_path,
                    base_name,
                    display_name,
                    path_only,
                    stream_name,
                    has_ads,
                    dir_needs_sep,
                    is_directory,
                    displayed_size,
                    displayed_alloc,
                    descendants,
                    treesize,
                    tree_allocated,
                    record,
                    index,
                    tz_offset_secs,
                );

                row_buffer.push('\n');
                writer.write_all(row_buffer.as_bytes())?;
                row_count += 1;
            }
        }
    }

    let rows_ms = t_rows.elapsed().as_millis();
    tracing::debug!(cache_ms, rows_ms, row_count, "[TIMING] streaming output");
    tracing::info!(
        cache_ms,
        rows_ms,
        row_count,
        "📊 streaming: output phase breakdown"
    );

    if format == "custom" {
        let final_footer = CppFooterContext {
            output_targets: footer_ctx.output_targets,
            pattern: footer_ctx.pattern,
            row_count,
        };
        super::write_cpp_drive_footer(writer, &final_footer)?;
    }

    Ok(row_count)
}

/// Write all column values for a single output row.
#[expect(clippy::too_many_arguments, reason = "passes all per-row state inline")]
#[expect(
    clippy::too_many_lines,
    reason = "flat match over 30+ OutputColumn variants — each branch is 2-5 lines"
)]
fn write_row_columns(
    row_buffer: &mut String,
    output_cols: &[OutputColumn],
    output_config: &OutputConfig,
    itoa_buf: &mut itoa::Buffer,
    base_path: &str,
    base_name: &str,
    display_name: &str,
    path_only: &str,
    stream_name: &str,
    has_ads: bool,
    dir_needs_sep: bool,
    is_directory: bool,
    displayed_size: u64,
    displayed_alloc: u64,
    descendants: u32,
    treesize: u64,
    tree_allocated: u64,
    record: &uffs_mft::index::FileRecord,
    index: &uffs_mft::MftIndex,
    tz_offset_secs: i32,
) {
    for (col_idx, col) in output_cols.iter().enumerate() {
        if col_idx > 0 {
            row_buffer.push_str(&output_config.separator);
        }
        match col {
            OutputColumn::Path => {
                row_buffer.push_str(&output_config.quote);
                row_buffer.push_str(base_path);
                if has_ads {
                    row_buffer.push(':');
                    row_buffer.push_str(stream_name);
                } else if dir_needs_sep {
                    row_buffer.push('\\');
                }
                row_buffer.push_str(&output_config.quote);
            }
            OutputColumn::Name => {
                if has_ads {
                    row_buffer.push_str(&output_config.quote);
                    row_buffer.push_str(base_name);
                    row_buffer.push(':');
                    row_buffer.push_str(stream_name);
                    row_buffer.push_str(&output_config.quote);
                } else {
                    append_quoted(row_buffer, &output_config.quote, display_name);
                }
            }
            OutputColumn::PathOnly => {
                row_buffer.push_str(&output_config.quote);
                row_buffer.push_str(path_only);
                if dir_needs_sep && is_directory && !has_ads {
                    row_buffer.push('\\');
                }
                row_buffer.push_str(&output_config.quote);
            }
            OutputColumn::Size => {
                row_buffer.push_str(itoa_buf.format(displayed_size));
            }
            OutputColumn::SizeOnDisk => {
                row_buffer.push_str(itoa_buf.format(displayed_alloc));
            }
            OutputColumn::Created => {
                append_datetime(row_buffer, record.stdinfo.created, tz_offset_secs);
            }
            OutputColumn::Modified => {
                append_datetime(row_buffer, record.stdinfo.modified, tz_offset_secs);
            }
            OutputColumn::Accessed => {
                append_datetime(row_buffer, record.stdinfo.accessed, tz_offset_secs);
            }
            OutputColumn::Descendants => {
                row_buffer.push_str(itoa_buf.format(descendants));
            }
            OutputColumn::TreeSize => {
                row_buffer.push_str(itoa_buf.format(treesize));
            }
            OutputColumn::TreeAllocated => {
                row_buffer.push_str(itoa_buf.format(tree_allocated));
            }
            OutputColumn::Type => {
                let ext_id = record.first_name.name.extension_id();
                let ext = index.extensions.get_extension(ext_id).unwrap_or("");
                append_quoted(row_buffer, &output_config.quote, ext);
            }
            OutputColumn::Attributes | OutputColumn::AttributeValue => {
                row_buffer.push_str(itoa_buf.format(record.stdinfo.to_attributes()));
            }
            OutputColumn::Hidden => {
                append_bool(row_buffer, output_config, record.stdinfo.is_hidden());
            }
            OutputColumn::System => {
                append_bool(row_buffer, output_config, record.stdinfo.is_system());
            }
            OutputColumn::Archive => {
                append_bool(row_buffer, output_config, record.stdinfo.is_archive());
            }
            OutputColumn::ReadOnly => {
                append_bool(row_buffer, output_config, record.stdinfo.is_readonly());
            }
            OutputColumn::Compressed => {
                append_bool(row_buffer, output_config, record.stdinfo.is_compressed());
            }
            OutputColumn::Encrypted => {
                append_bool(row_buffer, output_config, record.stdinfo.is_encrypted());
            }
            OutputColumn::Sparse => {
                append_bool(row_buffer, output_config, record.stdinfo.is_sparse());
            }
            OutputColumn::Reparse => {
                append_bool(row_buffer, output_config, record.stdinfo.is_reparse());
            }
            OutputColumn::Offline => {
                append_bool(row_buffer, output_config, record.stdinfo.is_offline());
            }
            OutputColumn::NotIndexed => {
                append_bool(row_buffer, output_config, record.stdinfo.is_not_indexed());
            }
            OutputColumn::Temporary => {
                append_bool(row_buffer, output_config, record.stdinfo.is_temporary());
            }
            OutputColumn::Virtual => {
                append_bool(row_buffer, output_config, record.stdinfo.is_virtual());
            }
            OutputColumn::Pinned => {
                append_bool(row_buffer, output_config, record.stdinfo.is_pinned());
            }
            OutputColumn::Unpinned => {
                append_bool(row_buffer, output_config, record.stdinfo.is_unpinned());
            }
            OutputColumn::DirectoryFlag => {
                append_bool(row_buffer, output_config, is_directory);
            }
            OutputColumn::Integrity => {
                append_bool(
                    row_buffer,
                    output_config,
                    record.stdinfo.is_integrity_stream(),
                );
            }
            OutputColumn::NoScrub => {
                append_bool(row_buffer, output_config, record.stdinfo.is_no_scrub_data());
            }
            OutputColumn::Bulkiness => {
                row_buffer.push_str(OutputColumn::Bulkiness.default_value());
            }
        }
    }
}

/// Write the configured header for direct native output.
pub(super) fn write_native_header<W: Write + ?Sized>(
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

/// Append a datetime field using fast manual formatting.
///
/// Replaces `chrono::format("%Y-%m-%d %H:%M:%S")` which re-parses the format
/// string on every call (24.9M times for 8.3M records × 3 timestamp columns).
/// Manual formatting is ~10-20× faster for this fixed format.
#[expect(
    clippy::cast_sign_loss,
    reason = "rem_euclid always returns non-negative value"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "day_secs and doe are mathematically bounded within u32 range"
)]
fn append_datetime(row_buffer: &mut String, timestamp_micros: i64, tz_offset_secs: i32) {
    use core::fmt::Write;

    // Apply timezone offset directly to the Unix timestamp (avoids chrono
    // DateTime construction + with_timezone + format overhead entirely).
    let adjusted_secs = timestamp_micros.div_euclid(1_000_000) + i64::from(tz_offset_secs);

    // Civil time decomposition (no leap seconds — matches chrono behavior).
    // Algorithm: days since Unix epoch → year/month/day; remainder → H:M:S.
    let day_secs = adjusted_secs.rem_euclid(86_400) as u32;
    let days = adjusted_secs.div_euclid(86_400) + 719_468; // shift to 0000-03-01 epoch

    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year_offset = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_proxy = (5 * doy + 2) / 153;
    let day = doy - (153 * month_proxy + 2) / 5 + 1;
    let month = if month_proxy < 10 {
        month_proxy + 3
    } else {
        month_proxy - 9
    };
    let year = if month <= 2 {
        year_offset + 1
    } else {
        year_offset
    };

    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    // Write "YYYY-MM-DD HH:MM:SS" directly — no format string parsing.
    // String::write_fmt is infallible, so ignoring the result is safe.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "String::write_fmt never fails"
    )]
    let _ = write!(
        row_buffer,
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
    );
}
