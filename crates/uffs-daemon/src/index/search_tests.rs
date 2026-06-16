// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`super::build_output_config`].
//!
//! Lifted out of `search.rs` to keep that file under the 800-line
//! policy ceiling.  Re-attached to the original `search::tests` path
//! via `#[path = "search_tests.rs"] mod tests;` in `search.rs`, so the
//! `super::*` glob below continues to resolve against the `search`
//! module's scope.

use uffs_client::protocol::SearchParams;

use super::*;

/// Regression: `build_output_config` must use `OutputConfig` defaults
/// (separator = `,`, quote = `"`) when `SearchParams` output fields
/// are `None`.
///
/// Previously, `from_cli_args` set `output_separator: Some("")` which
/// caused `build_output_config` to call `with_separator("")`, wiping
/// the comma delimiter and producing concatenated output with no field
/// separation.
#[test]
fn build_output_config_preserves_defaults_when_none() {
    let params = SearchParams::default();
    assert!(params.output_separator.is_none());
    assert!(params.output_quote.is_none());
    assert!(params.output_pos.is_none());
    assert!(params.output_neg.is_none());

    let cfg = build_output_config(&params);
    assert_eq!(cfg.separator, ",", "default separator must be comma");
    assert_eq!(cfg.quote, "\"", "default quote must be double-quote");
    assert_eq!(cfg.pos, "1", "default pos must be '1'");
    assert_eq!(cfg.neg, "0", "default neg must be '0'");
    assert!(cfg.header, "default header must be true");
}

/// Guard against the exact bug: passing `Some("")` to
/// `build_output_config` must NOT wipe the separator/quote.
/// The daemon function should skip empty-string overrides.
#[test]
fn build_output_config_some_empty_string_overrides_defaults() {
    // This test documents the current behavior: if Some("") is passed,
    // it DOES override the default.  The fix is in from_cli_args which
    // must never produce Some("") for unset flags.
    let params = SearchParams {
        output_separator: Some(String::new()),
        output_quote: Some(String::new()),
        ..Default::default()
    };
    let cfg = build_output_config(&params);
    // Some("") overrides defaults — this is why from_cli_args must
    // use None, not Some(""), for unset flags.
    assert_eq!(
        cfg.separator, "",
        "Some(\"\") overrides default — from_cli_args must use None"
    );
    assert_eq!(
        cfg.quote, "",
        "Some(\"\") overrides default — from_cli_args must use None"
    );
}

/// Explicit separator and quote values must be forwarded.
#[test]
fn build_output_config_explicit_values_applied() {
    let params = SearchParams {
        output_separator: Some(";".to_owned()),
        output_quote: Some("'".to_owned()),
        output_pos: Some("+".to_owned()),
        output_neg: Some("-".to_owned()),
        output_header: Some(false),
        output_columns: Some("parity".to_owned()),
        output_parity_compat: Some(true),
        output_tz_offset_hours: Some(-7_i32),
        ..Default::default()
    };
    let cfg = build_output_config(&params);
    assert_eq!(cfg.separator, ";");
    assert_eq!(cfg.quote, "'");
    assert_eq!(cfg.pos, "+");
    assert_eq!(cfg.neg, "-");
    assert!(!cfg.header);
    assert!(cfg.columns.is_some(), "parity columns must be set");
    assert!(cfg.parity_compat, "parity_compat must be true");
    assert_eq!(cfg.timezone_offset_secs, -7_i32 * 3_600_i32);
}

/// `--parity-compat` without explicit sep/quote must produce a valid
/// parity `OutputConfig` with default comma + double-quote delimiters.
#[test]
fn build_output_config_parity_compat_uses_defaults() {
    let params = SearchParams {
        output_columns: Some("parity".to_owned()),
        output_parity_compat: Some(true),
        ..Default::default()
    };
    let cfg = build_output_config(&params);
    assert_eq!(
        cfg.separator, ",",
        "parity mode must use comma separator by default"
    );
    assert_eq!(
        cfg.quote, "\"",
        "parity mode must use double-quote by default"
    );
    assert!(cfg.parity_compat);
    assert!(cfg.columns.is_some());
}

/// WI-1.2: the `--out` export writes the target with the expected content
/// AND does not follow a symlink pre-planted at the *old, predictable* temp
/// name (`<target>.uffs.tmp`).  The randomised temp name makes that guess
/// fail, so the sentinel the symlink points at is left untouched.
#[test]
fn write_rows_to_file_ignores_pre_planted_predictable_tmp() {
    use uffs_core::search::backend::DisplayRow;
    use uffs_mft::platform::DriveLetter;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("results.csv");

    // Pre-plant a symlink at the OLD predictable temp path, pointing at a
    // sentinel we must not clobber. (Unix only — needs symlink semantics.)
    #[cfg(unix)]
    let sentinel = {
        let sentinel = dir.path().join("sentinel.bin");
        std::fs::write(&sentinel, b"DO NOT TOUCH").unwrap();
        let guessed_tmp = target.with_extension("uffs.tmp");
        std::os::unix::fs::symlink(&sentinel, &guessed_tmp).unwrap();
        sentinel
    };

    let rows = vec![DisplayRow::new(
        0,
        DriveLetter::C,
        "C:\\Users\\file.txt".to_owned(),
        1234,
        false,
        0,
        0,
        0,
        0,
        1234,
        0,
        0,
        0,
    )];
    let cfg = build_output_config(&SearchParams::default());

    let written = write_rows_to_file(&rows, target.to_str().unwrap(), &cfg).unwrap();
    assert_eq!(written, 1);

    // Target exists with the row's filename in it.
    let body = std::fs::read_to_string(&target).unwrap();
    assert!(
        body.contains("file.txt"),
        "exported file must contain the row"
    );

    // The pre-planted symlink's sentinel target is untouched (randomised
    // temp name was used, not the guessed predictable one).
    #[cfg(unix)]
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"DO NOT TOUCH");
}

// ── resolve_search_limit ───────────────────────────────────────────

/// With no post-scan filter, the user's limit flows straight through so
/// the backend can stop early (the common, cheap path).
#[test]
fn resolve_search_limit_passes_user_limit_when_no_post_filter() {
    assert_eq!(
        resolve_search_limit(false, false, false, Some(10)),
        Some(10)
    );
    assert_eq!(resolve_search_limit(false, false, false, None), None);
}

/// A wire post-filter or a display-row filter lifts the cap so the
/// post-scan pass can still satisfy the user's limit.
#[test]
fn resolve_search_limit_lifts_cap_for_post_and_display_filters() {
    assert_eq!(resolve_search_limit(true, false, false, Some(10)), None);
    assert_eq!(resolve_search_limit(false, true, false, Some(10)), None);
}

/// `--malformed` (near-zero hit rate) lifts the cap so name/regex/tree
/// scans don't under-return.  Safe for match-all because
/// `collect_global_top_n` filters during the heap scan.
#[test]
fn resolve_search_limit_lifts_cap_for_malformed_positive() {
    assert_eq!(resolve_search_limit(false, false, true, Some(10)), None);
}

/// `--well-formed` (~100% hit rate) must NOT lift the cap: an unbounded
/// match-all scan would admit the whole index into the heap.  The caller
/// passes `malformed_positive = false` for `Some(false)`, so the user
/// limit is preserved.
#[test]
fn resolve_search_limit_keeps_cap_for_well_formed() {
    assert_eq!(
        resolve_search_limit(false, false, false, Some(10)),
        Some(10)
    );
}
