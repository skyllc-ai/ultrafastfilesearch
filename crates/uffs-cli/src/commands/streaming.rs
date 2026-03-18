//! Streaming output writer for multi-drive search (Windows-only).
//!
//! Supports CSV (header + rows) and NDJSON (one JSON object per line) formats.
//! Writes results as each drive completes for immediate user feedback.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use uffs_core::output::OutputConfig;

/// Streaming output writer for multi-drive search.
pub(crate) struct StreamingWriter<W: Write> {
    writer: Mutex<W>,
    format: StreamingFormat,
    output_config: OutputConfig,
    header_written: AtomicBool,
    rows_written: AtomicUsize,
    limit: u32,
}

/// Output format for streaming writer.
#[derive(Clone, Copy)]
enum StreamingFormat {
    Csv,
    Json,
}

impl<W: Write> StreamingWriter<W> {
    /// Create a new streaming writer.
    pub(crate) fn new(writer: W, format: &str, limit: u32, output_config: OutputConfig) -> Self {
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
    pub(crate) fn write_batch(&self, df: &uffs_mft::DataFrame) -> Result<usize> {
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
        use std::io::Write as _;

        let col_names: Vec<_> = df.get_column_names();
        let columns: Vec<_> = col_names
            .iter()
            .filter_map(|name| {
                df.column(name)
                    .ok()
                    .map(|col| (format_json_string(name.as_str()), col))
            })
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
                obj.push_str(col_name);
                obj.push_str(": ");
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
    pub(crate) fn limit_reached(&self) -> bool {
        if self.limit == 0 {
            return false;
        }
        self.rows_written.load(Ordering::Relaxed) >= self.limit as usize
    }

    /// Get total rows written.
    pub(crate) fn total_rows(&self) -> usize {
        self.rows_written.load(Ordering::Relaxed)
    }
}

/// Escape a string for JSON output.
pub(crate) fn format_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control <= '\u{1F}' => push_json_unicode_escape(&mut escaped, control),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn push_json_unicode_escape(buf: &mut String, ch: char) {
    const HEX: &[char; 16] = &[
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    let code = ch as u32;
    buf.push_str("\\u");
    for shift in [12_u32, 8_u32, 4_u32, 0_u32] {
        let nibble = usize::try_from((code >> shift) & 0xF).unwrap_or_default();
        buf.push(*HEX.get(nibble).unwrap_or(&'0'));
    }
}

/// Format a cell value for JSON output.
pub(crate) fn format_json_value(col: &uffs_polars::Column, row_idx: usize) -> String {
    use uffs_polars::{AnyValue, TimeUnit};

    match col.get(row_idx) {
        Ok(AnyValue::Null) | Err(_) => "null".to_owned(),
        Ok(AnyValue::String(value)) => format_json_string(value),
        Ok(AnyValue::Boolean(boolean)) => if boolean { "true" } else { "false" }.to_owned(),
        Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) => {
            let secs = ts.div_euclid(1_000_000);
            let micros = u32::try_from(ts.rem_euclid(1_000_000)).unwrap_or_default();
            chrono::DateTime::from_timestamp(secs, micros * 1000).map_or_else(
                || "null".to_owned(),
                |datetime| format_json_string(&datetime.format("%Y-%m-%d %H:%M:%S").to_string()),
            )
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
        Ok(value) => format_json_string(&value.to_string()),
    }
}

