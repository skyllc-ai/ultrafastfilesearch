//! Output configuration and row formatting helpers.

use core::fmt::Write as _;
use std::io::Write;

use uffs_polars::{Column, DataFrame, DataType};

use super::{CPP_COLUMN_ORDER, OutputColumn};
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
    // NOTE: Tripwire was removed from OutputConfig (Fix #1).
    // Tripwire is now logged to stderr/tracing and embedded in binary string table.
    // See TRIPWIRE constant in uffs-cli/src/commands.rs.
}

impl Default for OutputConfig {
    fn default() -> Self {
        // Get current timezone offset once, matching C++ behavior where
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
            .is_some_and(|cols| cols.iter().any(OutputColumn::is_tree_column))
    }

    /// Get the list of requested tree columns.
    #[must_use]
    pub fn get_tree_columns(&self) -> Vec<crate::tree::TreeColumn> {
        self.columns
            .as_ref()
            .map(|cols| {
                cols.iter()
                    .filter_map(OutputColumn::to_tree_column)
                    .collect()
            })
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
        // Determine columns to output - use CPP_COLUMN_ORDER when "all" is specified
        let output_cols: &[OutputColumn] = if let Some(cols) = &self.columns {
            cols.as_slice()
        } else {
            CPP_COLUMN_ORDER
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
            // C++ outputs header followed by empty line
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
                        // Numeric columns (like Descendants) should show "0" to match C++.
                        row_buffer.push_str(default_value);
                    }
                }
            }

            row_buffer.push('\n');
            writer.write_all(row_buffer.as_bytes())?;
        }

        Ok(())
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
                if let Ok(val) = series.str() {
                    if let Some(str_val) = val.get(row_idx) {
                        row_buffer.push_str(&self.quote);
                        row_buffer.push_str(str_val);
                        row_buffer.push_str(&self.quote);
                    }
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
                // Convert UTC timestamp to local time using FIXED offset (matching C++ output).
                // C++ uses Windows' FileTimeToLocalFileTime() which applies the CURRENT
                // timezone offset to ALL timestamps, ignoring historical DST transitions.
                // We match this by using a fixed offset computed once at startup.
                if let Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) = series.get(row_idx) {
                    // Use div_euclid/rem_euclid for correct handling of negative timestamps.
                    // rem_euclid(1_000_000) always returns [0, 999_999] for any i64 input.
                    let secs = ts.div_euclid(1_000_000);
                    let micros_i64 = ts.rem_euclid(1_000_000);
                    // Safe: rem_euclid(1_000_000) is always in [0, 999_999], fits in u32
                    let micros = u32::try_from(micros_i64).unwrap_or(0);
                    if let Some(utc_dt) = chrono::DateTime::from_timestamp(secs, micros * 1000) {
                        // Apply fixed timezone offset (computed once at startup)
                        // This matches established behavior: same offset for all timestamps
                        if let Some(timezone_offset) = fixed_tz {
                            let local_dt = utc_dt.with_timezone(timezone_offset);
                            // Format WITHOUT subseconds to match C++ output exactly
                            Self::append_display(row_buffer, local_dt.format("%Y-%m-%d %H:%M:%S"));
                        } else {
                            // Fallback: format as UTC if offset is invalid
                            Self::append_display(row_buffer, utc_dt.format("%Y-%m-%d %H:%M:%S"));
                        }
                    }
                }
            }
            _ => {
                if let Ok(val) = series.get(row_idx) {
                    if !matches!(val, AnyValue::Null) {
                        Self::append_display(row_buffer, val);
                    }
                }
            }
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
