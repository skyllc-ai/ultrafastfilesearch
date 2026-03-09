//! Output helpers for CLI search commands.

use std::fs::File;
use std::io::{BufWriter, Write};
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use tracing::info;
use uffs_core::output::OutputConfig;
use uffs_core::{export_csv, export_json, export_table};

/// Streaming output writer for multi-drive search.
///
/// Supports CSV (header + rows) and NDJSON (one JSON object per line) formats.
/// Writes results as each drive completes for immediate user feedback.
#[cfg(windows)]
pub(super) struct StreamingWriter<W: Write> {
    writer: Mutex<W>,
    format: StreamingFormat,
    output_config: OutputConfig,
    header_written: AtomicBool,
    rows_written: AtomicUsize,
    limit: u32,
}

/// Output format for streaming writer.
#[cfg(windows)]
#[derive(Clone, Copy)]
enum StreamingFormat {
    Csv,
    Json,
}

#[cfg(windows)]
impl<W: Write> StreamingWriter<W> {
    pub(super) fn new(writer: W, format: &str, limit: u32, output_config: OutputConfig) -> Self {
        let fmt = match format.to_lowercase().as_str() {
            "json" => StreamingFormat::Json,
            _ => StreamingFormat::Csv,
        };
        Self {
            writer: Mutex::new(writer),
            format: fmt,
            output_config,
            header_written: AtomicBool::new(false),
            rows_written: AtomicUsize::new(0),
            limit,
        }
    }

    /// Write a DataFrame batch. Returns number of rows written.
    pub(super) fn write_batch(&self, df: &uffs_mft::DataFrame) -> Result<usize> {
        if df.height() == 0 {
            return Ok(0);
        }

        if self.limit > 0 {
            let current = self.rows_written.load(Ordering::Relaxed);
            if current >= self.limit as usize {
                return Ok(0);
            }
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock error: {e}"))?;

        match self.format {
            StreamingFormat::Csv => self.write_csv_batch(&mut *writer, df),
            StreamingFormat::Json => self.write_json_batch(&mut *writer, df),
        }
    }

    fn write_csv_batch(&self, writer: &mut W, df: &uffs_mft::DataFrame) -> Result<usize> {
        let height = df.height();
        if height == 0 {
            return Ok(0);
        }

        let write_header = !self.header_written.swap(true, Ordering::SeqCst);

        let rows_to_write = if self.limit > 0 {
            let current = self.rows_written.load(Ordering::Relaxed);
            let remaining = (self.limit as usize).saturating_sub(current);
            if remaining == 0 {
                return Ok(0);
            }
            remaining.min(height)
        } else {
            height
        };

        let df_slice = if rows_to_write < height {
            df.slice(0, rows_to_write)
        } else {
            df.clone()
        };

        let mut config = self.output_config.clone();
        config.header = write_header;

        config
            .write(&df_slice, &mut *writer)
            .map_err(|e| anyhow::anyhow!("Write error: {e}"))?;

        self.rows_written
            .fetch_add(rows_to_write, Ordering::Relaxed);

        writer.flush()?;
        Ok(rows_to_write)
    }

    fn write_json_batch(&self, writer: &mut W, df: &uffs_mft::DataFrame) -> Result<usize> {
        let col_names: Vec<_> = df.get_column_names();
        let columns: Vec<_> = col_names
            .iter()
            .filter_map(|name| df.column(name).ok().map(|col| (*name, col)))
            .collect();

        let mut rows_written = 0;
        let height = df.height();
        let mut obj = String::with_capacity(512);

        for row_idx in 0..height {
            if self.limit > 0 {
                let current = self.rows_written.fetch_add(1, Ordering::Relaxed);
                if current >= self.limit as usize {
                    break;
                }
            } else {
                self.rows_written.fetch_add(1, Ordering::Relaxed);
            }

            obj.clear();
            obj.push('{');
            for (i, (col_name, col)) in columns.iter().enumerate() {
                if i > 0 {
                    obj.push_str(", ");
                }
                obj.push('"');
                obj.push_str(col_name);
                obj.push_str("\": ");
                obj.push_str(&format_json_value(col, row_idx));
            }
            obj.push('}');
            writeln!(writer, "{obj}")?;
            rows_written += 1;
        }

        writer.flush()?;
        Ok(rows_written)
    }

    /// Check if we've hit the output limit.
    pub(super) fn limit_reached(&self) -> bool {
        if self.limit == 0 {
            return false;
        }
        self.rows_written.load(Ordering::Relaxed) >= self.limit as usize
    }

    /// Get total rows written.
    pub(super) fn total_rows(&self) -> usize {
        self.rows_written.load(Ordering::Relaxed)
    }
}

/// Format a cell value for JSON output.
#[cfg(windows)]
fn format_json_value(col: &uffs_polars::Column, row_idx: usize) -> String {
    use uffs_polars::{AnyValue, TimeUnit};

    let val = col.get(row_idx);
    match val {
        Ok(AnyValue::Null) => "null".to_string(),
        Ok(AnyValue::String(s)) => format!("\"{}\"", s.replace('"', "\\\"").replace('\n', "\\n")),
        Ok(AnyValue::Boolean(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) => {
            let secs = ts / 1_000_000;
            let micros = (ts % 1_000_000) as u32;
            if let Some(dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
                format!("\"{}\"", dt.format("%Y-%m-%d %H:%M:%S"))
            } else {
                "null".to_string()
            }
        }
        Ok(AnyValue::UInt8(n)) => n.to_string(),
        Ok(AnyValue::UInt16(n)) => n.to_string(),
        Ok(AnyValue::UInt32(n)) => n.to_string(),
        Ok(AnyValue::UInt64(n)) => n.to_string(),
        Ok(AnyValue::Int8(n)) => n.to_string(),
        Ok(AnyValue::Int16(n)) => n.to_string(),
        Ok(AnyValue::Int32(n)) => n.to_string(),
        Ok(AnyValue::Int64(n)) => n.to_string(),
        Ok(AnyValue::Float32(n)) => n.to_string(),
        Ok(AnyValue::Float64(n)) => n.to_string(),
        Ok(v) => format!("\"{}\"", v.to_string().replace('"', "\\\"")),
        Err(_) => "null".to_string(),
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
    clippy::single_call_fn,
    reason = "temporary conversion layer — will be removed when output pipeline supports SearchResults directly"
)]
#[expect(
    clippy::too_many_lines,
    reason = "builds the full output schema with 30+ columns"
)]
#[expect(
    clippy::min_ident_chars,
    reason = "short names (e.g. df) conventional in DataFrame-heavy code"
)]
#[expect(
    clippy::option_if_let_else,
    reason = "if-let chains are clearer for record lookup fallback"
)]
pub(super) fn results_to_dataframe(
    index: &uffs_mft::MftIndex,
    results: &[uffs_core::SearchResult],
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

        frs_values.push(result.frs);
        parent_frs_values.push(result.parent_frs);
        names.push(result.name.clone());
        paths.push(result.path.clone().unwrap_or_default());
        sizes.push(result.size);
        stream_names.push(result.stream_name.clone());

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
#[expect(
    clippy::single_call_fn,
    reason = "extracted to reduce search() line count below clippy::too_many_lines limit"
)]
pub(super) fn write_results(
    results: &uffs_mft::DataFrame,
    format: &str,
    out: &str,
    output_config: &OutputConfig,
    output_targets: &[char],
) -> Result<()> {
    let is_console = matches!(
        out.to_lowercase().as_str(),
        "console" | "con" | "term" | "terminal"
    );

    if is_console {
        let stdout = std::io::stdout();
        match format {
            "json" => export_json(results, stdout)?,
            "csv" => export_csv(results, stdout)?,
            "custom" => output_config.write(results, stdout)?,
            _ => export_table(results, stdout)?,
        }
    } else {
        let file =
            File::create(out).with_context(|| format!("Failed to create output file: {out}"))?;
        let mut writer = BufWriter::new(file);

        match format {
            "json" => export_json(results, &mut writer)?,
            "csv" => export_csv(results, &mut writer)?,
            _ => output_config.write(results, &mut writer)?,
        }

        if !output_targets.is_empty() {
            let drive_list = output_targets
                .iter()
                .map(|drive| format!("{drive}:"))
                .collect::<Vec<_>>()
                .join("|");
            let summary_label = ['D', 'r', 'i', 'v', 'e', 's', '?']
                .into_iter()
                .collect::<String>();
            write!(
                writer,
                "\r\n\r\n{} \t{}\t{drive_list}\r\n\r\n",
                summary_label,
                output_targets.len()
            )?;
        }
        writer.flush()?;

        info!(file = out, "Results written to file");
    }

    Ok(())
}
