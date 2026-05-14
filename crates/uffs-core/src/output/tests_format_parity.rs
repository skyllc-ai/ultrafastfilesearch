// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Byte-parity regression tests between `uffs-core`'s legacy
//! `write_display_rows` and the shared `uffs_format::write_rows`.
//!
//! Split out of `output/tests.rs` to keep that file under the
//! 800-line policy ceiling.  Re-attached via `mod tests_format_parity;`
//! in `output/mod.rs`, so the tests still live under
//! `output::tests_format_parity` and `super::*` resolves against
//! `output`'s scope.

use super::*;

// ============================================================================
// uffs-format byte-parity regression tests
// ============================================================================
//
// These tests pin the v0.5.62 formatter unification: `uffs-core`'s
// legacy `write_display_rows` and the shared `uffs_format::write_rows`
// must produce byte-identical output for every configuration the CLI
// exposes.  If either drifts, the cross-crate contract — "CLI stdout
// == daemon --out=file" — is silently broken.
//
// Each test builds the same `Vec<DisplayRow>` + `OutputConfig` twice,
// feeds one copy through the legacy writer and one through the shared
// writer, and asserts the resulting byte buffers are equal.

/// Helper: run both formatters and assert byte equality.  Returns the
/// shared bytes for additional assertions.
fn assert_format_parity(
    config: &OutputConfig,
    rows: &[crate::search::backend::DisplayRow],
) -> Vec<u8> {
    let mut legacy = Vec::new();
    config
        .write_display_rows(rows, &mut legacy)
        .expect("legacy write_display_rows should succeed");

    let fmt_cfg = uffs_format::OutputConfig {
        columns: config.columns.as_ref().map(|cols| {
            cols.iter()
                .copied()
                .map(display_rows_format_bridge::field_id_to_format_column)
                .collect()
        }),
        separator: config.separator.clone(),
        quote: config.quote.clone(),
        header: config.header,
        pos: config.pos.clone(),
        neg: config.neg.clone(),
        timezone_offset_secs: config.timezone_offset_secs,
        parity_compat: config.parity_compat,
    };
    let mut shared = Vec::new();
    uffs_format::write_rows(&fmt_cfg, rows, &mut shared)
        .expect("uffs_format::write_rows should succeed");

    assert_eq!(
        legacy,
        shared,
        "uffs_format::write_rows output must match legacy write_display_rows byte-for-byte\n\
         legacy:\n{legacy_text}\nshared:\n{shared_text}",
        legacy_text = String::from_utf8_lossy(&legacy),
        shared_text = String::from_utf8_lossy(&shared),
    );
    legacy
}

/// Small file row — exercises Path / Name / Size / bool-flag columns.
#[test]
fn format_parity_basic_file_row() {
    use crate::search::backend::DisplayRow;

    let rows = vec![DisplayRow::new(
        0_u32,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Temp\\sample.txt".to_owned(),
        256,
        false,
        0,
        0,
        0,
        0x0022, // HIDDEN | ARCHIVE
        4096,
        0,
        0,
        0,
    )];
    let config = OutputConfig::new()
        .with_columns("path,name,size,size_on_disk,hidden,archive")
        .with_header(true)
        .with_quote("\"")
        .with_pos("Y")
        .with_neg("N")
        .with_tz_offset_hours(0);

    assert_format_parity(&config, &rows);
}

/// Directory row in parity-compat mode — exercises the `parity_dir`
/// branch that rewrites `Path` / `Name` / `PathOnly` / `Size` / `SizeOnDisk`.
#[test]
fn format_parity_parity_compat_directory_row() {
    use crate::search::backend::DisplayRow;

    let rows = vec![DisplayRow::new(
        0_u32,
        uffs_mft::platform::DriveLetter::D,
        "D:\\Users\\alice\\Documents".to_owned(),
        0,
        true,
        0,
        0,
        0,
        0x0010, // DIRECTORY
        0,
        12,
        65_536,
        73_728,
    )];
    let config = OutputConfig::new()
        .with_columns("parity")
        .with_header(false)
        .with_quote("\"")
        .with_tz_offset_hours(0)
        .with_parity_compat(true);

    assert_format_parity(&config, &rows);
}

/// `--columns all` at the baseline — exercises every projectable
/// output column in one pass, including derived (Bulkiness, Type,
/// `Extension`, `NameLength`, `PathLength`).
#[test]
fn format_parity_all_columns_baseline() {
    use crate::search::backend::DisplayRow;

    let rows = vec![DisplayRow::new(
        0_u32,
        uffs_mft::platform::DriveLetter::C,
        "C:\\Projects\\uffs\\README.md".to_owned(),
        4321,
        false,
        0,
        0,
        0,
        0x0020, // ARCHIVE
        8192,
        0,
        0,
        0,
    )];
    let config = OutputConfig::new()
        .with_columns("all")
        .with_header(true)
        .with_quote("\"")
        .with_tz_offset_hours(-8);

    assert_format_parity(&config, &rows);
}

/// Parallel branch: a 20 000-row batch must hit the same bytes as
/// the shared writer's parallel branch.  Pins the chunk-merge order
/// agreement between the two implementations.
#[test]
fn format_parity_parallel_branch_matches() {
    use crate::search::backend::DisplayRow;

    let rows: Vec<DisplayRow> = (0..20_000_u32)
        .map(|idx| {
            DisplayRow::new(
                idx,
                uffs_mft::platform::DriveLetter::C,
                format!("C:\\batch\\row_{idx:05}.bin"),
                u64::from(idx),
                false,
                0,
                0,
                0,
                0,
                u64::from(idx) + 1024,
                0,
                0,
                0,
            )
        })
        .collect();
    let config = OutputConfig::new()
        .with_columns("path,name,size,size_on_disk")
        .with_header(false)
        .with_quote("\"")
        .with_tz_offset_hours(0);

    assert_format_parity(&config, &rows);
}
