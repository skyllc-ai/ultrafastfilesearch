// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Regression tests for [`super::ParseSearchParamsError`].
//!
//! Phase 5d migration of `parse_and_validate_search_params` from
//! `Result<SearchParams, String>` to a typed error.  These tests lock
//! the wire-byte equivalence of the error JSON-RPC body so external
//! callers see no change, and pin the Display contract of the typed
//! variants for future maintainers.
//!
//! Sibling-file layout (`#[path]` attached) matches the established
//! `handler_blob.rs` / `handler_paths_blob_tests.rs` /
//! `handler_csv_blob_tests.rs` pattern — keeps `handler.rs` under
//! the 800-line policy ceiling without sacrificing test coverage.

use core::error::Error as _;

use uffs_client::protocol::{ERR_INVALID_PARAMS, RpcRequest};

use super::{MAX_PATTERN_LENGTH, ParseSearchParamsError};

/// Build the wire bytes the **pre-Phase-5d** code emitted for the
/// `MissingOrInvalidParams` path — used as the byte-equivalence
/// reference for the new typed-error route.
fn legacy_missing_or_invalid_json(id: u64) -> String {
    serde_json::to_string(&uffs_client::protocol::RpcErrorResponse::error(
        Some(id),
        ERR_INVALID_PARAMS,
        "Missing or invalid search params",
    ))
    .expect("fixed-shape RpcErrorResponse must always serialise")
}

/// Build the wire bytes the **pre-Phase-5d** code emitted for the
/// `PatternTooLong` path — used as the byte-equivalence reference.
fn legacy_pattern_too_long_json(id: u64, len: usize) -> String {
    serde_json::to_string(&uffs_client::protocol::RpcErrorResponse::error(
        Some(id),
        ERR_INVALID_PARAMS,
        &format!("Pattern too long ({len} chars, max {MAX_PATTERN_LENGTH})"),
    ))
    .expect("fixed-shape RpcErrorResponse must always serialise")
}

/// The `MissingOrInvalidParams` variant Display matches the legacy
/// message text exactly — operator logs are unchanged.
#[test]
fn missing_or_invalid_params_display_locked() {
    let err = ParseSearchParamsError::MissingOrInvalidParams;
    assert_eq!(err.to_string(), "Missing or invalid search params");
    assert!(
        err.source().is_none(),
        "MissingOrInvalidParams has no underlying source",
    );
}

/// The `PatternTooLong` variant Display interpolates `len` and the
/// `MAX_PATTERN_LENGTH` constant exactly the way the previous code did.
#[test]
fn pattern_too_long_display_locked() {
    let err = ParseSearchParamsError::PatternTooLong { len: 9_999 };
    assert_eq!(
        err.to_string(),
        format!("Pattern too long (9999 chars, max {MAX_PATTERN_LENGTH})"),
    );
}

/// `to_rpc_error_json` for `MissingOrInvalidParams` must produce the
/// **byte-identical** JSON-RPC error body the pre-Phase-5d path
/// produced — same id, same code, same message, same encoder.
#[test]
fn missing_or_invalid_params_wire_bytes_locked() {
    let err = ParseSearchParamsError::MissingOrInvalidParams;
    for id in [0_u64, 1, 42, u64::MAX] {
        assert_eq!(
            err.to_rpc_error_json(id),
            legacy_missing_or_invalid_json(id),
            "wire bytes drifted for MissingOrInvalidParams at id={id}",
        );
    }
}

/// `to_rpc_error_json` for `PatternTooLong` must produce the
/// **byte-identical** JSON-RPC error body the pre-Phase-5d path
/// produced for the same `id` and `len`.
#[test]
fn pattern_too_long_wire_bytes_locked() {
    for len in [MAX_PATTERN_LENGTH + 1, MAX_PATTERN_LENGTH * 2, 1_000_000] {
        let err = ParseSearchParamsError::PatternTooLong { len };
        for id in [0_u64, 1, u64::MAX] {
            assert_eq!(
                err.to_rpc_error_json(id),
                legacy_pattern_too_long_json(id, len),
                "wire bytes drifted for PatternTooLong at id={id}, len={len}",
            );
        }
    }
}

/// End-to-end: an `RpcRequest` with `params = null` exercises the
/// `MissingOrInvalidParams` path through
/// [`super::RequestHandler::parse_and_validate_search_params`].
#[test]
fn parse_and_validate_rejects_missing_params() {
    let req = RpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Some(7),
        method: "search".to_owned(),
        params: None,
    };
    let err = super::RequestHandler::parse_and_validate_search_params(&req)
        .expect_err("missing params must error");
    assert_eq!(err, ParseSearchParamsError::MissingOrInvalidParams);
}

/// End-to-end: an `RpcRequest` carrying a `SearchParams` with a
/// `pattern` over the [`MAX_PATTERN_LENGTH`] guard exercises the
/// `PatternTooLong` path through
/// [`super::RequestHandler::parse_and_validate_search_params`].
#[test]
fn parse_and_validate_rejects_overlong_pattern() {
    let oversized = "a".repeat(MAX_PATTERN_LENGTH + 1);
    let req = RpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Some(11),
        method: "search".to_owned(),
        params: Some(serde_json::json!({ "pattern": oversized })),
    };
    let err = super::RequestHandler::parse_and_validate_search_params(&req)
        .expect_err("overlong pattern must error");
    assert_eq!(err, ParseSearchParamsError::PatternTooLong {
        len: MAX_PATTERN_LENGTH + 1
    },);
}

/// End-to-end happy path: a well-formed `SearchParams` round-trips
/// through `parse_and_validate_search_params` unchanged.  Locks the
/// contract that the migration did not introduce a spurious failure
/// mode for valid input.
#[test]
fn parse_and_validate_accepts_valid_params() {
    let req = RpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Some(13),
        method: "search".to_owned(),
        params: Some(serde_json::json!({ "pattern": "*.rs" })),
    };
    let params = super::RequestHandler::parse_and_validate_search_params(&req)
        .expect("valid params must parse");
    assert_eq!(params.pattern, "*.rs");
}
