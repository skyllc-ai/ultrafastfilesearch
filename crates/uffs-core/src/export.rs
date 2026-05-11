// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Export functions for query results.
//!
//! Provides multiple output formats: table, JSON, CSV.

use std::io::Write;

use uffs_polars::{DataFrame, SerWriter as _};

use crate::error::{CoreError, Result};

/// Export `DataFrame` as a formatted table.
///
/// # Errors
///
/// Returns an error if writing fails.
pub fn export_table<W: Write>(df: &DataFrame, mut writer: W) -> Result<()> {
    writeln!(writer, "{df}")?;
    Ok(())
}

/// Export `DataFrame` as JSON.
///
/// # Errors
///
/// Returns an error if serialization or writing fails.
pub fn export_json<W: Write>(df: &DataFrame, writer: W) -> Result<()> {
    // Convert DataFrame to JSON
    let mut json_writer = uffs_polars::JsonWriter::new(writer);
    json_writer
        .finish(&mut df.clone())
        .map_err(|err| CoreError::Export(err.to_string()))?;
    Ok(())
}

/// Export `DataFrame` as CSV.
///
/// # Errors
///
/// Returns an error if writing fails.
pub fn export_csv<W: Write>(df: &DataFrame, writer: W) -> Result<()> {
    let mut csv_writer = uffs_polars::CsvWriter::new(writer);
    csv_writer
        .finish(&mut df.clone())
        .map_err(|err| CoreError::Export(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use uffs_polars::Column;

    use super::*;

    type TestResult = core::result::Result<(), Box<dyn core::error::Error>>;

    fn create_test_df() -> core::result::Result<DataFrame, uffs_polars::PolarsError> {
        DataFrame::new_infer_height(vec![
            Column::new("name".into(), &["file1.txt", "file2.rs"]),
            Column::new("size".into(), &[1024_u64, 2048]),
        ])
    }

    #[test]
    fn test_export_table() -> TestResult {
        let df = create_test_df()?;
        let mut output = Vec::new();
        export_table(&df, &mut output)?;
        let output_str = String::from_utf8(output)?;
        assert!(output_str.contains("file1.txt"));
        assert!(output_str.contains("1024"));
        Ok(())
    }

    #[test]
    fn test_export_json() -> TestResult {
        let df = create_test_df()?;
        let mut output = Vec::new();
        export_json(&df, &mut output)?;
        let output_str = String::from_utf8(output)?;
        assert!(output_str.contains("file1.txt"));
        Ok(())
    }

    #[test]
    fn test_export_csv() -> TestResult {
        let df = create_test_df()?;
        let mut output = Vec::new();
        export_csv(&df, &mut output)?;
        let output_str = String::from_utf8(output)?;
        assert!(output_str.contains("name"));
        assert!(output_str.contains("file1.txt"));
        Ok(())
    }
}
