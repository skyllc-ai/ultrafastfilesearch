// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Typed error for [`super::RequestHandler::parse_and_validate_search_params`].
//!
//! Phase 5d migration of the previous `Result<SearchParams, String>`
//! return type: the `String` used to be a pre-serialised JSON-RPC
//! error body, conflating two concerns (parsing vs. wire-output) in
//! one signature.  Splitting them out yields a domain-only error type
//! here and a single boundary call at
//! [`super::RequestHandler::handle_search`] — same wire bytes,
//! sharper types.
//!
//! Lifted out of `handler.rs` to keep that file under the 800-line
//! policy ceiling.  Re-attached via
//! `#[path = "handler_parse_search_params.rs"] mod parse_search_params;`
//! in `handler.rs`, which then re-exports the type with
//! `use parse_search_params::ParseSearchParamsError;` so every call
//! site (and the co-located test module at
//! `handler_parse_search_params_tests.rs`) keeps the unqualified name.

use alloc::borrow::Cow;

use uffs_client::protocol::{ERR_INVALID_PARAMS, RpcErrorResponse};

use super::MAX_PATTERN_LENGTH;

/// Typed error produced by
/// [`super::RequestHandler::parse_and_validate_search_params`].
///
/// `#[non_exhaustive]` per Phase 5c discipline so a future validation
/// branch (e.g. drive-letter-allowlist enforcement) can grow a variant
/// without a semver bump on the (workspace-internal) consumer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(super) enum ParseSearchParamsError {
    /// `req.params` was absent, malformed JSON, or failed to
    /// deserialise as `SearchParams`.
    #[error("Missing or invalid search params")]
    MissingOrInvalidParams,
    /// `search_params.pattern.len()` exceeded [`MAX_PATTERN_LENGTH`]
    /// — the S4.4.3 regex-DoS guard rail.
    #[error("Pattern too long ({len} chars, max {MAX_PATTERN_LENGTH})")]
    PatternTooLong {
        /// The offending pattern's length in bytes.
        len: usize,
    },
}

impl ParseSearchParamsError {
    /// Render this error as a fully-formed JSON-RPC error response
    /// body, using `id` as the request correlation id.
    ///
    /// The wire bytes produced here are byte-identical with the
    /// pre-Phase-5d output (same JSON-RPC version, same
    /// [`ERR_INVALID_PARAMS`] code, same message text, same encoder),
    /// so clients see no change.  When serialisation itself fails
    /// (which would be a `serde_json` defect on a fixed-shape error
    /// payload), we fall back to the empty-string default exactly as
    /// the previous code did — preserving the existing observable
    /// behaviour without silently swallowing a panic.
    pub(super) fn to_rpc_error_json(&self, id: u64) -> String {
        // Build the message as `&str` where possible to avoid a
        // throw-away heap allocation for the static-text variant.
        let message: Cow<'_, str> = match *self {
            Self::MissingOrInvalidParams => "Missing or invalid search params".into(),
            Self::PatternTooLong { len } => {
                format!("Pattern too long ({len} chars, max {MAX_PATTERN_LENGTH})").into()
            }
        };
        serde_json::to_string(&RpcErrorResponse::error(
            Some(id),
            ERR_INVALID_PARAMS,
            &message,
        ))
        .unwrap_or_default()
    }
}
