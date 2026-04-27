// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests are inherently outside cfg(test)"
)]
#![expect(
    clippy::expect_used,
    clippy::default_numeric_fallback,
    reason = "integration test — `expect` makes failures self-describing; default-typed range bounds keep the synthetic fixture readable"
)]

//! CI-visible regression guard for the v13 FILETIME output pipeline.
//!
//! # Background
//!
//! The v12 → v13 index migration changed `DisplayRow::{modified, created,
//! accessed}` from Unix microseconds to **raw Windows FILETIME** (100-ns
//! ticks since 1601-01-01 UTC) to match the C++ NTFS baseline and to
//! allow representing pre-1970 dates.  A handful of formatter functions
//! (`append_datetime_native`, the Polars `write_value` Datetime arm)
//! silently kept their Unix-micros interpretation and produced
//! `year-6220` output for 2026-era timestamps — an order-of-magnitude
//! date corruption that slipped past every unit test because the
//! formatters had no assertions on actual date values.
//!
//! # What this file covers
//!
//! A minimal end-to-end walk through the production output path using
//! synthetic `DisplayRow`s that the daemon itself would emit:
//!
//! 1. Build rows with known-FILETIME timestamps spanning past, present, and
//!    future dates.
//! 2. Feed them to [`OutputConfig::write_display_rows`] — the exact function
//!    `write_rows_to_file` calls in `uffs-daemon::search`.
//! 3. Assert the emitted CSV contains the expected calendar-date strings.
//!
//! If the FILETIME interpretation regresses anywhere in the column
//! writer, formatter helpers, or `uffs_time::filetime_to_calendar`,
//! these assertions fire at `cargo test` time — no drive-specific
//! baselines required, so they run in CI on any platform.
//!
//! # Related
//!
//! - Per-formatter unit tests live in
//!   `uffs-core::output::config::tests::append_datetime_native_*` and
//!   `uffs-core::output::tests::test_write_datetime_column_*`.
//! - Full real-drive parity (C++ baseline byte-for-byte) is checked via
//!   `scripts/verify_parity.rs`, which requires local `cpp_*.txt` baselines and
//!   therefore cannot run in CI.

// Acknowledge crates declared as workspace deps of `uffs-core` but not
// reachable from this integration test's tiny surface (`OutputConfig` +
// `DisplayRow` + `uffs_time` constants).  The pattern mirrors
// `uffs-mcp/tests/mcp_protocol.rs`.
use aho_corasick as _;
use anyhow as _;
use bytemuck as _;
use chrono as _;
use criterion as _;
use devicons as _;
use globset as _;
use itoa as _;
use memchr as _;
use memmap2 as _;
use rayon as _;
use regex as _;
use rustc_hash as _;
use serde_json as _;
use sha2 as _;
use tempfile as _;
use thiserror as _;
use tokio as _;
use tracing as _;
use uffs_core::output::OutputConfig;
use uffs_core::search::backend::DisplayRow;
use uffs_format as _;
use uffs_mft as _;
use uffs_polars as _;
use uffs_security as _;
use uffs_text as _;
use zstd as _;

/// FILETIME constants for reference calendar dates, computed once so
/// the test assertions stay readable.
mod ft {
    use uffs_time::{FILETIME_TICKS_PER_MICROSECOND, FILETIME_UNIX_DIFF};

    /// Build a FILETIME from a Unix-micros input at build time.
    const fn filetime_from_unix_micros(unix_us: i64) -> i64 {
        unix_us * FILETIME_TICKS_PER_MICROSECOND + FILETIME_UNIX_DIFF
    }

    /// 2024-01-01 00:00:00 UTC — Unix sec `1_704_067_200`.
    pub(super) const T_2024_01_01: i64 = filetime_from_unix_micros(1_704_067_200_000_000);

    /// 2026-01-20 00:00:00 UTC — Unix sec `1_768_867_200`.
    pub(super) const T_2026_01_20: i64 = filetime_from_unix_micros(1_768_867_200_000_000);

    /// 2008-05-20 18:40:39 UTC — one of the Burning Man 2008 JPGs in the
    /// `verify_parity` drive D baseline.  Unix sec `1_211_308_839`.
    pub(super) const T_2008_05_20_184039: i64 = filetime_from_unix_micros(1_211_308_839_000_000);

    /// FILETIME 0 — NTFS "unset" sentinel.  Must render as `0000-00-00
    /// 00:00:00` or empty, never as `1601-01-01`.
    pub(super) const T_UNSET: i64 = 0;
}

/// Build a synthetic `DisplayRow` with only the fields we care about in
/// this test.  All other fields are zeroed.
fn row(path: &str, modified: i64, created: i64, accessed: i64) -> DisplayRow {
    DisplayRow::new(
        0_u32,
        'C',
        path.to_owned(),
        1024_u64,
        false,
        modified,
        created,
        accessed,
        0_u32,
        1024_u64,
        0_u32,
        0_u64,
        0_u64,
    )
}

/// Run `rows` through `OutputConfig::write_display_rows` and return the
/// resulting CSV as a String.
fn render(rows: &[DisplayRow], config: &OutputConfig) -> String {
    let mut buf: Vec<u8> = Vec::new();
    config
        .write_display_rows(rows, &mut buf)
        .expect("write_display_rows should succeed on synthetic rows");
    String::from_utf8(buf).expect("output must be valid UTF-8")
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Default (baseline) column output — FILETIME → calendar date string
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn baseline_output_renders_2024_filetime_as_2024() {
    let rows = vec![row(
        r"C:\tmp\newyear.txt",
        ft::T_2024_01_01,
        ft::T_2024_01_01,
        ft::T_2024_01_01,
    )];
    let config = OutputConfig::new()
        .with_columns("name,created,modified,accessed")
        .with_header(false)
        .with_tz_offset_hours(0_i32);

    let csv = render(&rows, &config);
    assert!(
        csv.contains("2024-01-01 00:00:00"),
        "FILETIME for 2024-01-01 must render as 2024-01-01 (got: {csv})"
    );
    assert!(
        !csv.contains("6220-") && !csv.contains("2084-"),
        "must NOT render as year 6220 (FILETIME-as-micros) or 2084 (micros-as-FILETIME): {csv}"
    );
}

#[test]
fn baseline_output_renders_2026_filetime_as_2026() {
    let rows = vec![row(
        r"C:\work\report.xlsx",
        ft::T_2026_01_20,
        ft::T_2026_01_20,
        ft::T_2026_01_20,
    )];
    let config = OutputConfig::new()
        .with_columns("name,modified")
        .with_header(false)
        .with_tz_offset_hours(0_i32);

    let csv = render(&rows, &config);
    assert!(
        csv.contains("2026-01-20 00:00:00"),
        "FILETIME for 2026-01-20 must render as 2026-01-20 (got: {csv})"
    );
}

#[test]
fn baseline_output_renders_2008_filetime_as_2008() {
    // The exact timestamp from the drive D parity baseline (Burning Man
    // 2008 photos) that surfaced the original regression.
    let rows = vec![row(
        r"D:\Bilder\Burning Man 2008\100_0371.JPG",
        ft::T_2008_05_20_184039,
        ft::T_2008_05_20_184039,
        ft::T_2008_05_20_184039,
    )];
    let config = OutputConfig::new()
        .with_columns("name,modified")
        .with_header(false)
        .with_tz_offset_hours(0_i32);

    let csv = render(&rows, &config);
    assert!(
        csv.contains("2008-05-20 18:40:39"),
        "FILETIME for 2008-05-20 18:40:39 must round-trip exactly (got: {csv})"
    );
    assert!(
        !csv.contains("6760-") && !csv.contains("6043-"),
        "must NOT render as year 6760 or 6043 (the two observed regressions): {csv}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Parity-compat CSV — the 25-column format the C++ baseline emits
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parity_compat_output_renders_filetime_correctly() {
    let rows = vec![row(
        r"D:\data\recent.bin",
        ft::T_2026_01_20,
        ft::T_2024_01_01,
        ft::T_2008_05_20_184039,
    )];
    // Parity-compat mode: header off, "parity" column preset,
    // comma separator, double-quote, `-7` tz to match verify_parity
    // script defaults.  The output here mirrors what
    // `uffs --format custom --parity-compat --tz-offset -7` produces.
    let config = OutputConfig::new()
        .with_columns("parity")
        .with_header(false)
        .with_parity_compat(true)
        .with_quote("\"")
        .with_separator(",")
        .with_tz_offset_hours(-7_i32);

    let csv = render(&rows, &config);
    // -7h tz: 2026-01-20 00:00 UTC → 2026-01-19 17:00 local
    assert!(
        csv.contains("2026-01-19 17:00:00"),
        "parity-compat must apply -7h tz to 2026-01-20 UTC → 2026-01-19 17:00 (got: {csv})"
    );
    // -7h tz: 2024-01-01 00:00 UTC → 2023-12-31 17:00 local
    assert!(
        csv.contains("2023-12-31 17:00:00"),
        "parity-compat must apply -7h tz to 2024-01-01 UTC → 2023-12-31 17:00 (got: {csv})"
    );
    // -7h tz: 2008-05-20 18:40:39 UTC → 2008-05-20 11:40:39 local
    assert!(
        csv.contains("2008-05-20 11:40:39"),
        "parity-compat must apply -7h tz to 2008-05-20 18:40:39 UTC → 2008-05-20 11:40:39 (got: {csv})"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Sentinel: FILETIME 0 must render as the parity "0000-00-00" zero-date,
//    NEVER as 1601-01-01 (which is what a naive Hinnant decode gives).
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn unset_filetime_renders_as_zero_sentinel_not_1601() {
    let rows = vec![row(
        r"C:\never-touched.dat",
        ft::T_UNSET,
        ft::T_UNSET,
        ft::T_UNSET,
    )];
    let config = OutputConfig::new()
        .with_columns("name,created,modified,accessed")
        .with_header(false)
        .with_tz_offset_hours(0_i32);

    let csv = render(&rows, &config);
    assert!(
        !csv.contains("1601-"),
        "FILETIME 0 must NOT decode to 1601-01-01 (got: {csv})"
    );
    assert!(
        csv.contains("0000-00-00 00:00:00"),
        "FILETIME 0 must render as the parity zero-date sentinel (got: {csv})"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. Batch smoke — many rows in a single write call must stay consistent
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn batch_filetime_output_is_line_stable() {
    let rows: Vec<DisplayRow> = (0..100)
        .map(|i| {
            // Span 2024-01-01 .. 2024-01-01 + 100 days by adding one day
            // of FILETIME ticks (86400 seconds × 10_000_000 ticks/sec).
            let delta = i64::from(i) * 86_400 * 10_000_000;
            row(
                &format!(r"C:\batch\file_{i:03}.dat"),
                ft::T_2024_01_01 + delta,
                ft::T_2024_01_01,
                ft::T_2024_01_01,
            )
        })
        .collect();

    let config = OutputConfig::new()
        .with_columns("name,modified")
        .with_header(false)
        .with_tz_offset_hours(0_i32);

    let csv = render(&rows, &config);
    let line_count = csv.lines().count();
    assert_eq!(
        line_count, 100,
        "100 input rows must produce 100 output lines (got {line_count}): {csv}"
    );
    // First row: 2024-01-01, last row: day 99 of 2024 = 2024-04-09.
    assert!(
        csv.contains("2024-01-01 00:00:00"),
        "first day must appear: {csv}"
    );
    assert!(
        csv.contains("2024-04-09 00:00:00"),
        "day 99 of 2024 = 2024-04-09 must appear: {csv}"
    );
    assert!(
        !csv.contains("6220-"),
        "no row may render as year 6220: {csv}"
    );
}
