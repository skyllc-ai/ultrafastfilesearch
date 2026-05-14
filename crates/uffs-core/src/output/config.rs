// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Output configuration and `DataFrame` formatting helpers.
//!
//! The native `DisplayRow` formatter (sequential + rayon-parallel write
//! branches, the 30-arm column→text dispatch, attribute bit constants,
//! and the raw-FILETIME → text helper) lives in the sibling
//! [`super::display_rows`] module so `config.rs` stays under the
//! 800-LOC file-size policy.  The public entry point
//! [`OutputConfig::write_display_rows`] delegates to it; callers see
//! no API change.

use core::fmt::Write as _;
use std::io::Write;

use uffs_polars::{Column, DataFrame, DataType};

use super::{BASELINE_COLUMN_ORDER, OutputColumn};
use crate::error::Result;

/// Output configuration for customizable formatting.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Columns to output (None = all available).
    pub columns: Option<Vec<OutputColumn>>,
    /// Column separator (default: ",").
    pub separator: String,
    /// Quote character for strings (default: "\"").
    pub quote: String,
    /// Include header row (default: true).
    pub header: bool,
    /// Representation for true/active boolean (default: "1").
    pub pos: String,
    /// Representation for false/inactive boolean (default: "0").
    pub neg: String,
    /// Fixed timezone offset in seconds from UTC (computed once at startup).
    /// This matches established behavior where Windows'
    /// `FileTimeToLocalFileTime()` uses the CURRENT timezone offset for ALL
    /// timestamps, ignoring historical DST.
    pub timezone_offset_secs: i32,
    /// Parity-compat mode: directories get trailing `\` in `Path`,
    /// empty `Name`, self-path in `PathOnly`, and treesize for `Size`.
    pub parity_compat: bool,
    // NOTE: Tripwire was removed from OutputConfig (Fix #1).
    // Tripwire is now logged to stderr/tracing and embedded in binary string table.
    // See TRIPWIRE constant in uffs-cli/src/commands.rs.
}

impl Default for OutputConfig {
    fn default() -> Self {
        // Get current timezone offset once. On Windows,
        // Windows' FileTimeToLocalFileTime() uses the CURRENT offset for all timestamps
        let timezone_offset_secs = chrono::Local::now().offset().local_minus_utc();

        Self {
            columns: None,
            separator: ",".to_owned(),
            quote: "\"".to_owned(),
            header: true,
            pos: "1".to_owned(),
            neg: "0".to_owned(),
            timezone_offset_secs,
            parity_compat: false,
        }
    }
}

impl OutputConfig {
    /// Create a new output configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse columns from a comma-separated string.
    ///
    /// Special value "all" returns None (meaning all columns).
    #[must_use]
    #[expect(
        clippy::shadow_reuse,
        reason = "rebinding input to trimmed+lowered version is clearer than a new name"
    )]
    pub fn parse_columns(input: &str) -> Option<Vec<OutputColumn>> {
        let input = input.trim().to_lowercase();
        if input == "all" {
            return None;
        }
        if input == "parity" {
            return Some(super::column::PARITY_COLUMN_ORDER.to_vec());
        }

        let cols: Vec<OutputColumn> = input
            .split(',')
            .filter_map(|col| OutputColumn::parse(col.trim()))
            .collect();

        if cols.is_empty() { None } else { Some(cols) }
    }

    /// Parse separator with special character handling.
    ///
    /// Supports (case-insensitive):
    /// - TAB → `\t`
    /// - NEWLINE, NEW LINE → `\n`
    /// - SPACE → ` `
    /// - RETURN → `\r`
    /// - DOUBLE → `"`
    /// - SINGLE → `'`
    /// - NULL → `\0`
    #[must_use]
    pub fn parse_separator(input: &str) -> String {
        match input.to_uppercase().as_str() {
            "TAB" => "\t".to_owned(),
            "NEWLINE" | "NEW LINE" => "\n".to_owned(),
            "SPACE" => " ".to_owned(),
            "RETURN" => "\r".to_owned(),
            "DOUBLE" => "\"".to_owned(),
            "SINGLE" => "'".to_owned(),
            "NULL" => "\0".to_owned(),
            _ => input.to_owned(),
        }
    }

    /// Set columns from string.
    #[must_use]
    pub fn with_columns(mut self, columns: &str) -> Self {
        self.columns = Self::parse_columns(columns);
        self
    }

    /// Set separator.
    #[must_use]
    pub fn with_separator(mut self, sep: &str) -> Self {
        self.separator = Self::parse_separator(sep);
        self
    }

    /// Set quote character.
    #[must_use]
    pub fn with_quote(mut self, quote: &str) -> Self {
        quote.clone_into(&mut self.quote);
        self
    }

    /// Set header inclusion.
    #[must_use]
    pub const fn with_header(mut self, header: bool) -> Self {
        self.header = header;
        self
    }

    /// Set positive boolean representation.
    #[must_use]
    pub fn with_pos(mut self, pos: &str) -> Self {
        pos.clone_into(&mut self.pos);
        self
    }

    /// Set negative boolean representation.
    #[must_use]
    pub fn with_neg(mut self, neg: &str) -> Self {
        neg.clone_into(&mut self.neg);
        self
    }

    /// Override the timezone offset used for timestamp display.
    ///
    /// Accepts offset in hours from UTC (e.g., `-8` for PST, `-7` for PDT,
    /// `1` for CET). This overrides the auto-detected local timezone offset.
    ///
    /// Useful for reproducible parity testing when the reference output was
    /// generated in a different DST period than the current one.
    #[must_use]
    pub const fn with_tz_offset_hours(mut self, hours: i32) -> Self {
        self.timezone_offset_secs = hours * 3_600_i32;
        self
    }

    /// Enable parity-compat directory formatting.
    #[must_use]
    pub const fn with_parity_compat(mut self, enabled: bool) -> Self {
        self.parity_compat = enabled;
        self
    }

    // NOTE: with_tripwire() was removed (Fix #1).
    // Tripwire is now logged to stderr/tracing and embedded in binary string table.
    // See TRIPWIRE constant in uffs-cli/src/commands.rs.

    /// Check if the descendants column is requested.
    #[must_use]
    pub fn needs_descendants(&self) -> bool {
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&OutputColumn::Descendants))
    }

    /// Check if the path column is requested.
    ///
    /// The path column requires resolution from FRS + `parent_frs`.
    /// Returns true when columns is None (meaning "all") since "all" includes
    /// Path.
    #[must_use]
    pub fn needs_path_column(&self) -> bool {
        self.columns.as_ref().is_none_or(|cols| {
            cols.contains(&OutputColumn::Path) || cols.contains(&OutputColumn::PathOnly)
        })
    }

    /// Check if any tree-derived columns are requested.
    /// Note: "all" columns does NOT include tree columns by default (they're
    /// expensive to compute).
    #[must_use]
    pub fn needs_tree_columns(&self) -> bool {
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.iter().any(|col| col.is_tree_field()))
    }

    /// Get the list of requested tree columns.
    #[must_use]
    pub fn get_tree_columns(&self) -> Vec<crate::tree::TreeColumn> {
        self.columns
            .as_ref()
            .map(|cols| cols.iter().filter_map(|col| col.to_tree_column()).collect())
            .unwrap_or_default()
    }

    /// Write `DataFrame` to output with this configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    #[expect(
        clippy::option_if_let_else,
        reason = "if-let-else is clearer for control flow with early return"
    )]
    pub fn write<W: Write>(&self, df: &DataFrame, mut writer: W) -> Result<()> {
        // Determine columns to output - use BASELINE_COLUMN_ORDER when "all" is
        // specified
        let output_cols: &[OutputColumn] = if let Some(cols) = &self.columns {
            cols.as_slice()
        } else {
            BASELINE_COLUMN_ORDER
        };

        let fixed_tz = chrono::FixedOffset::east_opt(self.timezone_offset_secs);

        let resolved_columns: Vec<_> = output_cols
            .iter()
            .map(|col| {
                df.column(col.df_column())
                    .map_or_else(|_| Err(col.default_value()), Ok)
            })
            .collect();

        // NOTE: Tripwire is now logged to stderr/tracing instead of CSV output.
        // This keeps CSV output strict (header + data rows only) for parity analysis.
        // The tripwire is also embedded in the binary string table (see TRIPWIRE
        // constant).

        // Write header if enabled
        if self.header {
            let mut header = String::with_capacity(output_cols.len() * 24);
            for (idx, col) in output_cols.iter().enumerate() {
                if idx > 0 {
                    header.push_str(&self.separator);
                }
                header.push_str(&self.quote);
                header.push_str(col.display_name());
                header.push_str(&self.quote);
            }
            // Header followed by empty line
            header.push('\n');
            header.push('\n');
            writer.write_all(header.as_bytes())?;
        }

        // Write data rows
        let mut row_buffer = String::with_capacity(output_cols.len() * 32);
        for row_idx in 0..df.height() {
            row_buffer.clear();

            for (idx, resolved_column) in resolved_columns.iter().enumerate() {
                if idx > 0 {
                    row_buffer.push_str(&self.separator);
                }

                match resolved_column {
                    Ok(series) => {
                        self.write_value(&mut row_buffer, series, row_idx, fixed_tz.as_ref());
                    }
                    Err(default_value) => {
                        // Column not in DataFrame - use appropriate default.
                        // Numeric columns (like Descendants) should show "0".
                        row_buffer.push_str(default_value);
                    }
                }
            }

            row_buffer.push('\n');
            writer.write_all(row_buffer.as_bytes())?;
        }

        Ok(())
    }

    /// Write `DisplayRow` results directly — **no `DataFrame` involved**.
    ///
    /// Uses the same separator / quote / header / boolean formatting as
    /// [`write`](Self::write) so output is identical.  The
    /// implementation (sequential + rayon-parallel write branches, the
    /// column→text dispatch, attribute bit constants, and the
    /// FILETIME → text helper) lives in the sibling
    /// `crate::output::display_rows` module — this method is a thin
    /// delegation kept here so the public entry point stays a method
    /// on [`OutputConfig`] while the file stays under the 800-LOC
    /// policy.  Callers that want the formatter directly (e.g. to
    /// reuse `attr` bit constants or `append_datetime_native`) should
    /// reference `crate::output::display_rows` at its canonical
    /// root, not via a re-export.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails.
    pub fn write_display_rows<W: Write>(
        &self,
        rows: &[crate::search::backend::DisplayRow],
        writer: W,
    ) -> Result<()> {
        crate::output::display_rows::write_display_rows(self, rows, writer)
    }

    /// Append a single formatted series value to the provided row buffer.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "intentional catch-all for remaining dtypes"
    )]
    fn write_value(
        &self,
        row_buffer: &mut String,
        series: &Column,
        row_idx: usize,
        fixed_tz: Option<&chrono::FixedOffset>,
    ) {
        use uffs_polars::{AnyValue, TimeUnit};

        let dtype = series.dtype();

        match dtype {
            DataType::Boolean => {
                if let Ok(val) = series.bool() {
                    match val.get(row_idx) {
                        Some(true) => row_buffer.push_str(&self.pos),
                        Some(false) => row_buffer.push_str(&self.neg),
                        None => {}
                    }
                }
            }
            DataType::String => {
                if let Ok(val) = series.str()
                    && let Some(str_val) = val.get(row_idx)
                {
                    row_buffer.push_str(&self.quote);
                    row_buffer.push_str(str_val);
                    row_buffer.push_str(&self.quote);
                }
            }
            DataType::UInt64 => {
                if let Ok(val) = series.u64() {
                    match val.get(row_idx) {
                        Some(number) => {
                            Self::append_display(row_buffer, number);
                        }
                        None => row_buffer.push('0'),
                    }
                } else {
                    row_buffer.push('0');
                }
            }
            DataType::Int64 => {
                if let Ok(val) = series.i64() {
                    match val.get(row_idx) {
                        Some(number) => {
                            Self::append_display(row_buffer, number);
                        }
                        None => row_buffer.push('0'),
                    }
                } else {
                    row_buffer.push('0');
                }
            }
            DataType::UInt32 => {
                if let Ok(val) = series.u32() {
                    match val.get(row_idx) {
                        Some(number) => {
                            Self::append_display(row_buffer, number);
                        }
                        None => row_buffer.push('0'),
                    }
                } else {
                    row_buffer.push('0');
                }
            }
            DataType::Int32 => {
                if let Ok(val) = series.i32() {
                    match val.get(row_idx) {
                        Some(number) => {
                            Self::append_display(row_buffer, number);
                        }
                        None => row_buffer.push('0'),
                    }
                } else {
                    row_buffer.push('0');
                }
            }
            DataType::Datetime(TimeUnit::Microseconds, _) => {
                Self::append_filetime_value(row_buffer, series, row_idx, fixed_tz);
            }
            _ => {
                if let Ok(val) = series.get(row_idx)
                    && !matches!(val, AnyValue::Null)
                {
                    Self::append_display(row_buffer, val);
                }
            }
        }
    }

    /// Format the value at `row_idx` of a `Datetime(Microseconds)` column
    /// as a calendar string, treating the underlying i64 as **raw
    /// FILETIME** (100-ns ticks since 1601-01-01).
    ///
    /// ── v13+ FILETIME semantics ─────────────────────────────────────────
    ///
    /// The `DataFrame` schema declares timestamp columns as
    /// `Datetime(TimeUnit::Microseconds)` for backward compatibility with
    /// Polars analytics (date ops, SQL coercion, Parquet round-trips),
    /// but the underlying i64 values are **raw FILETIME** — see
    /// `uffs_mft::index::dataframe.rs` and
    /// `uffs_mft::reader::dataframe_build.rs`, which push
    /// `rec.stdinfo.created` (FILETIME per the `StandardInfo` doc)
    /// directly into the column with a type cast rather than a value
    /// conversion.
    ///
    /// Formatting therefore has to go through the FILETIME decomposition,
    /// not `chrono::DateTime::from_timestamp` with Unix-micros semantics —
    /// using the latter produces year-6220 output for 2026-era timestamps
    /// (combined ~369-year + 10× unit offset between the two encodings,
    /// same bug class that caused the `append_datetime_native` regression
    /// in this file).
    ///
    /// NOTE: Polars' own `CsvWriter` (used by `uffs load --output *.csv`)
    /// still formats this column as Unix-micros via its built-in
    /// `Datetime` serializer.  Fixing that requires a `DataFrame` schema
    /// change (switch to `Int64` or pre-convert values to Unix micros)
    /// and is tracked as a separate latent bug.
    ///
    /// Polars exposes two variants — `Datetime` (borrowed tz) and
    /// `DatetimeOwned` (owned tz `Arc`) — depending on how the column was
    /// constructed.  Both are matched here.
    fn append_filetime_value(
        row_buffer: &mut String,
        series: &Column,
        row_idx: usize,
        fixed_tz: Option<&chrono::FixedOffset>,
    ) {
        use uffs_polars::{AnyValue, TimeUnit};

        let filetime_opt: Option<i64> = match series.get(row_idx) {
            Ok(
                AnyValue::Datetime(ticks, TimeUnit::Microseconds, _)
                | AnyValue::DatetimeOwned(ticks, TimeUnit::Microseconds, _),
            ) => Some(ticks),
            _ => None,
        };
        let Some(filetime) = filetime_opt else { return };
        let tz_offset_secs: i32 = fixed_tz.map_or(0_i32, chrono::FixedOffset::local_minus_utc);
        let local_ft = uffs_time::filetime_with_tz_bias(filetime, tz_offset_secs);
        if let Some((year, month, day, hour, minute, second)) =
            uffs_time::filetime_to_calendar(local_ft)
        {
            Self::append_display(
                row_buffer,
                format_args!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"),
            );
        }
    }

    /// Append a displayable value to the row buffer without intermediate
    /// allocations.
    fn append_display<T>(row_buffer: &mut String, value: T)
    where
        T: core::fmt::Display,
    {
        if row_buffer.write_fmt(format_args!("{value}")).is_err() {
            row_buffer.push_str(&value.to_string());
        }
    }
}

// Native `DisplayRow` output — all of the attribute-bit constants, the
// 30-arm column→text dispatch, the `push_flag` / `append_datetime_native`
// helpers, and the parallel / sequential `write_display_rows` pair —
// live in the sibling `display_rows` module.  See
// [`OutputConfig::write_display_rows`] above for the public entry
// point that delegates into it; the unit-test anchor for
// `append_datetime_native` moved alongside to `display_rows_tests.rs`.
