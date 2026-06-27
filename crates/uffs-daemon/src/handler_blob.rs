// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Pre-format fast-path helpers for [`super::RequestHandler`].
//!
//! Both `try_pack_paths_blob` (path-only projections) and
//! `try_pack_csv_blob` (multi-column CSV / parity / `--format custom`)
//! pack a `SearchResponse`'s `InlineRows` payload into a UTF-8
//! [`SearchPayload::InlineBlob`] (≤ 512 KB) or
//! [`SearchPayload::ShmemBlob`] (above it), so the CLI can emit the
//! result with a single `write_all` instead of per-row JSON
//! deserialisation + `write_columnar` dispatch.
//!
//! Lifted out of `handler.rs` to keep that file under the 800-line
//! policy ceiling.  Re-attached via `#[path = "handler_blob.rs"] mod blob;`
//! in `handler.rs`, so these `impl RequestHandler` methods stay
//! addressable as `Self::try_pack_paths_blob(...)` /
//! `RequestHandler::try_pack_paths_blob(...)` from every call site
//! (including `handler::tests` in `handler_tests.rs`).
//!
//! None of these functions take `&self` — they are deliberately
//! static-style methods so they can be called directly from the
//! request dispatcher before an `Arc<IndexManager>` is materialised,
//! and so the tests can cover each branch without spinning up a
//! daemon.

use uffs_client::protocol::SearchParams;
use uffs_client::protocol::response::{SearchPayload, SearchResponse};

use super::RequestHandler;

impl RequestHandler {
    /// Pack a path-only search response into a single UTF-8 blob
    /// payload.
    ///
    /// When the client asked for a path-only projection, consume
    /// the inline `SearchRow` list and replace
    /// [`SearchResponse::payload`] with [`SearchPayload::InlineBlob`]
    /// (small payloads) or [`SearchPayload::ShmemBlob`] (large).
    /// The CLI then writes the entire buffer with a single
    /// `write_all`, skipping per-row JSON deserialization and
    /// format dispatch (both of which scale linearly with row count).
    ///
    /// This is invisible to the `--out=file` bench (which never
    /// transfers rows), but is a large win for interactive
    /// `uffs *.ext` to stdout or for pipe composition.
    ///
    /// ## Transport selection
    ///
    /// Two variants carry the same logical payload; the dispatch is
    /// keyed on blob **bytes**, not row count:
    ///
    /// - [`SearchPayload::InlineBlob`] — blob travels as a JSON string inside
    ///   the RPC envelope.  Used when `blob.len() <=
    ///   PATHS_BLOB_SHMEM_THRESHOLD` (512 KB). Simple, no extra file, works
    ///   over any transport that can carry the RPC envelope.
    /// - [`SearchPayload::ShmemBlob`] — blob is written verbatim to a mmap'd
    ///   file via [`uffs_client::shmem::write_paths_blob`]; the client streams
    ///   it back with a single `write_all` via
    ///   [`uffs_client::shmem::stream_paths_blob_into`].  Used above the
    ///   threshold to skip ~80 ms of JSON escape/unescape on backslash-heavy
    ///   multi-megabyte Windows-path payloads (the `C: ext:dll` benchmark).
    ///
    /// ## History
    ///
    /// Previously capped at [`uffs_client::shmem::SHMEM_THRESHOLD`]
    /// (100 K rows) on the theory that "a multi-megabyte JSON string
    /// would be the worse choice" above that count, with large
    /// responses routed through the old `shmem_path` (full
    /// `SearchRow` records) instead.  Measured wrong: on a 168 K-row
    /// path-only query the `SearchRow`-shmem path added ~744 ms vs.
    /// NUL (client-side `write_columnar` re-formatted every row
    /// through `extract_field` → `String::to_owned` + 4× `write!()`
    /// per field).  v0.5.60 removed the cap so all path-only
    /// projections use [`SearchPayload::InlineBlob`]; v0.5.61 added
    /// the binary shmem channel so multi-megabyte blobs bypass the
    /// JSON encode/decode too.  v0.5.62 unified both under the
    /// tagged [`SearchPayload`] enum, making the dispatch
    /// type-enforced.
    ///
    /// [`SearchPayload::ShmemRows`] is still used for multi-column
    /// responses above [`uffs_client::shmem::SHMEM_THRESHOLD`] —
    /// those genuinely need binary `SearchRow` transport because
    /// the client still has to format columns locally.
    pub(crate) fn try_pack_paths_blob(params: &SearchParams, response: &mut SearchResponse) {
        // Early-return unless the incoming payload is `InlineRows`
        // with at least one row AND the projection is path-only.
        // Any other payload variant means the search core already
        // chose a different shape (aggregations, file-sink, JSON
        // mode) and this fast path does not apply.  We borrow the
        // slice here (`row_slice`) solely for the cheap guard —
        // the owned `Vec` is extracted below via `mem::take` to
        // avoid cloning ~168 K entries on the fast path.
        let SearchPayload::InlineRows(row_slice) = &response.payload else {
            return;
        };
        if row_slice.is_empty() || !Self::is_path_only_projection(params) {
            return;
        }

        // Swap the payload out by value so we can consume the
        // `Vec<SearchRow>` without cloning its ~168 K entries.
        // `mem::take` leaves `SearchPayload::Empty` behind;
        // we overwrite `response.payload` again on either branch
        // below, so the `Empty` state is never observable.
        let taken = core::mem::take(&mut response.payload);
        let SearchPayload::InlineRows(rows) = taken else {
            // Unreachable: we matched `InlineRows` immediately above
            // and hold a `&mut SearchResponse` exclusively, so no
            // other thread can have swapped the variant between the
            // guard above and this `take`.  Any future edit that
            // breaks this invariant will hit this branch and fail
            // fast rather than silently discard the payload.
            tracing::error!(
                "try_pack_paths_blob: payload variant changed between guard and take \
                 \u{2014} this is a bug, see paths_blob dispatcher comment"
            );
            return;
        };
        let capacity: usize = rows
            .iter()
            .map(|row| row.path.len().saturating_add(1))
            .sum();
        let mut blob = String::with_capacity(capacity);
        for row in &rows {
            blob.push_str(&row.path);
            blob.push('\n');
        }
        // Drop the `Vec<SearchRow>` now that its contents are in
        // `blob` — keeps peak memory flat during the shmem write.
        drop(rows);

        // Prefer the raw-bytes shmem channel once the blob is big
        // enough that the JSON escape+decode round-trip dominates.
        // Small blobs stay inline to avoid paying the shmem file-
        // creation syscall for sub-millisecond payloads.
        if blob.len() > uffs_client::shmem::PATHS_BLOB_SHMEM_THRESHOLD {
            match uffs_client::shmem::write_paths_blob(&blob) {
                Ok(path) => {
                    let path_str = path.to_string_lossy().into_owned();
                    tracing::debug!(
                        blob_bytes = blob.len(),
                        path = %path_str,
                        "paths_blob offloaded to shmem (binary transport)"
                    );
                    response.payload = SearchPayload::ShmemBlob(path_str);
                    return;
                }
                Err(err) => {
                    // Fall through to inline JSON — slower, but still
                    // correct.  Logging at warn so the regression is
                    // visible in production if shmem breaks on a
                    // specific host (e.g. data dir not writable).
                    tracing::warn!(
                        error = %err,
                        blob_bytes = blob.len(),
                        "paths_blob shmem write failed; falling back to inline JSON"
                    );
                }
            }
        }
        response.payload = SearchPayload::InlineBlob(blob);
    }

    /// Return true when the client asked for a single path column.
    ///
    /// Matches the user-facing column aliases `"path"` and `"full path"`
    /// case-insensitively.  Multi-column projections, aggregation
    /// requests, projected-JSON mode, and custom sort clauses all
    /// disqualify the fast path — the response must still carry the
    /// full [`uffs_client::protocol::response::SearchRow`] data for the CLI's
    /// row-based formatters.
    ///
    /// Also requires the caller to have explicitly opted into a
    /// text-shaped payload via [`Self::caller_opted_into_blob_payload`]
    /// (i.e. `output_format` is `Some("csv" | "custom")`).  Non-CLI
    /// callers (`uffs-mcp`, programmatic API consumers) leave
    /// `output_format` as `None` to signal they want structured
    /// [`uffs_client::protocol::response::SearchRow`]s back, not a
    /// newline-separated UTF-8 blob they would have to re-parse.
    pub(crate) fn is_path_only_projection(params: &SearchParams) -> bool {
        if params.projection.len() != 1 {
            return false;
        }
        if !params.aggregations.is_empty() {
            return false;
        }
        if !Self::caller_opted_into_blob_payload(params) {
            return false;
        }
        let Some(col) = params.projection.first() else {
            return false;
        };
        let trimmed = col.trim();
        trimmed.eq_ignore_ascii_case("path") || trimmed.eq_ignore_ascii_case("full path")
    }

    /// Pack a multi-column CSV search response into a single UTF-8 blob
    /// payload.
    ///
    /// Extends the path-only blob fast path ([`Self::try_pack_paths_blob`])
    /// to every projection the daemon's CSV formatter can reproduce
    /// byte-for-byte.  When the guard accepts the request, the method
    /// consumes the inline `SearchRow` list, runs it through
    /// [`uffs_format::write_rows`] against the same
    /// [`uffs_core::output::OutputConfig`] the `--out=file` path uses
    /// (via [`crate::index::search::build_output_config`]), and replaces
    /// [`SearchResponse::payload`] with
    /// [`SearchPayload::InlineBlob`] (small) or
    /// [`SearchPayload::ShmemBlob`] (above 512 KB).  The CLI then
    /// writes the buffer verbatim with a single `write_all`, skipping
    /// per-row JSON deserialisation and the client-side `extract_field`
    /// dispatch entirely.
    ///
    /// ## Gate
    ///
    /// Pre-formatting is only safe when the final stdout bytes are
    /// guaranteed to match what the CLI would have produced from the
    /// same rows.  The guard enforces:
    ///
    /// 1. Payload is still `InlineRows` with at least one row. Earlier fast
    ///    paths (path-only blob, empty response, file sink) have already
    ///    short-circuited.
    /// 2. `response_mode != Json` — JSON callers use NDJSON via `serde_json`,
    ///    not the CSV writer.
    /// 3. `aggregations.is_empty()` — aggregation responses surface additional
    ///    payload shapes that the blob channel does not carry.
    /// 4. `output_file` is `None` — the file sink already streamed the rows
    ///    directly to disk, so the payload is `Empty` and the blob path would
    ///    be a no-op.
    /// 5. `output_format` is absent (CLI default `csv`), exactly `"csv"`, or
    ///    exactly `"custom"`.  `"json"` (NDJSON) and `"table"` (fixed-width)
    ///    retain CLI-side formatters because they are structural, not columnar.
    ///
    /// Phase 3 lifts the earlier parity / custom exclusions: the
    /// daemon now appends the legacy drive footer via
    /// [`uffs_format::write_legacy_drive_footer`] when
    /// `output_format == "custom"`, and the parity byte-parity
    /// contract is pinned by
    /// `uffs_cli::commands::output::tests::parity_byte_parity_*`.
    ///
    /// The [`SearchPayload::ShmemBlob`] dispatch threshold and shmem
    /// write fallback logic mirror [`Self::try_pack_paths_blob`]
    /// exactly — any future change to that pattern should update
    /// both sites (or extract a shared helper).
    pub(crate) fn try_pack_csv_blob(params: &SearchParams, response: &mut SearchResponse) {
        // Fast guard: anything other than non-empty inline rows means
        // an earlier pass has already claimed the payload or the
        // search produced no rows worth packing.
        let SearchPayload::InlineRows(row_slice) = &response.payload else {
            return;
        };
        if row_slice.is_empty() || !Self::is_csv_blob_eligible(params) {
            return;
        }

        let output_config = crate::index::search::build_output_config(params);
        let mut fmt_cfg = Self::core_config_to_format(&output_config);

        // Parity output must always emit a header — the CLI's
        // hand-rolled `write_parity` ignores `--header false` and
        // always emits the 25-column header, so the daemon must
        // match or the fast/slow paths would drift when users
        // explicitly opt out of headers on parity queries.
        if fmt_cfg.parity_compat {
            fmt_cfg.header = true;
        }

        // Consume the owned rows without cloning — `mem::take` leaves
        // `SearchPayload::Empty` behind, which the overwrite below
        // always replaces.  See `try_pack_paths_blob` for the full
        // rationale on this pattern.
        let taken = core::mem::take(&mut response.payload);
        let SearchPayload::InlineRows(rows) = taken else {
            tracing::error!(
                "try_pack_csv_blob: payload variant changed between guard and take \
                 \u{2014} this is a bug, see csv_blob dispatcher comment"
            );
            return;
        };

        // `uffs-format` is char-typed at its public API; convert
        // the typed slice at the boundary so the format crate keeps
        // its narrow no-`uffs-mft` dep (issue #216).
        let drive_chars: Vec<char> = params
            .output_drive_targets
            .iter()
            .map(|dl| dl.as_char())
            .collect();
        let footer_ctx =
            Self::wants_custom_footer(params).then(|| uffs_format::DriveFooterContext {
                output_targets: &drive_chars,
                pattern: &params.pattern,
                row_count: rows.len(),
            });

        response.payload = Self::render_csv_blob_payload(&fmt_cfg, rows, footer_ctx.as_ref());
    }

    /// Render `rows` into a CSV blob and wrap it in the appropriate
    /// [`SearchPayload`] variant.
    ///
    /// Extracted from [`Self::try_pack_csv_blob`] so the dispatcher's
    /// cognitive complexity stays under the workspace lint ceiling —
    /// the guard + ownership-transfer dance in the caller is
    /// orthogonal to the format + transport selection this helper
    /// owns.
    ///
    /// When `footer_ctx` is `Some`, the legacy drive footer produced
    /// by [`uffs_format::write_legacy_drive_footer`] is appended
    /// after the CSV body.  This is used for `--format custom`
    /// callers; for plain `--format csv` the footer is omitted.
    ///
    /// Error fallbacks preserve the invariant that the payload
    /// always carries the same data the caller supplied: if the
    /// formatter or the shmem write fails, the rows are returned to
    /// the caller as an [`SearchPayload::InlineRows`] variant rather
    /// than silently dropped.  Every failure path logs at `warn` or
    /// `error` so the regression is visible in production.
    pub(crate) fn render_csv_blob_payload(
        fmt_cfg: &uffs_format::OutputConfig,
        rows: Vec<uffs_client::protocol::response::SearchRow>,
        footer_ctx: Option<&uffs_format::DriveFooterContext<'_>>,
    ) -> SearchPayload {
        // 128 B per row is the empirical median the
        // `format_parity_parallel_branch_matches` regression test
        // settles on; undersized allocation costs one `String::reserve`
        // on the sequential path, and the parallel path pre-sizes its
        // per-chunk buffers independently so this pre-allocation only
        // matters on small sequential runs.
        let mut blob_bytes: Vec<u8> = Vec::with_capacity(rows.len().saturating_mul(128));
        if let Err(err) = uffs_format::write_rows(fmt_cfg, &rows, &mut blob_bytes) {
            // Infallible in practice — `Vec<u8>` as `io::Write` never
            // fails.  Log and fall back to the structured-row path so
            // the response still reaches the client with correct
            // bytes, just less efficiently.
            tracing::warn!(
                error = %err,
                rows = rows.len(),
                "csv_blob pre-format failed; falling back to InlineRows"
            );
            return SearchPayload::InlineRows(rows);
        }
        // Drop `Vec<SearchRow>` now so peak memory stays flat during
        // the shmem write below.
        drop(rows);

        if let Some(ctx) = footer_ctx
            && let Err(err) = uffs_format::write_legacy_drive_footer(&mut blob_bytes, ctx)
        {
            // Same reasoning as the `write_rows` error branch above:
            // `Vec<u8>` as `io::Write` is infallible in practice.
            // Log + keep the CSV body; the client will render
            // without the footer rather than losing the blob entirely.
            tracing::warn!(
                error = %err,
                blob_bytes = blob_bytes.len(),
                "csv_blob footer append failed; emitting CSV body without footer"
            );
        }

        let blob = match String::from_utf8(blob_bytes) {
            Ok(utf8) => utf8,
            Err(err) => {
                // Unreachable in practice — `uffs_format::write_rows`
                // and `write_legacy_drive_footer` both emit only
                // UTF-8 (all field values come from `SearchRow`
                // strings which are themselves UTF-8; the footer
                // uses ASCII text + drive letters).  An error here
                // would indicate a genuine invariant break and
                // should be investigated.
                tracing::error!(
                    error = %err,
                    "csv_blob: formatter produced non-UTF-8 bytes; this is a bug"
                );
                return SearchPayload::Empty;
            }
        };

        Self::package_csv_blob(blob)
    }

    /// Wrap a rendered CSV blob string in the right transport
    /// variant — inline when small, shmem when above the 512 KB
    /// threshold.
    ///
    /// Mirrors [`Self::try_pack_paths_blob`]'s threshold dispatch
    /// exactly; any future change to that pattern should update
    /// both sites (or the two should share a single helper).
    pub(crate) fn package_csv_blob(blob: String) -> SearchPayload {
        if blob.len() > uffs_client::shmem::PATHS_BLOB_SHMEM_THRESHOLD {
            match uffs_client::shmem::write_paths_blob(&blob) {
                Ok(path) => {
                    let path_str = path.to_string_lossy().into_owned();
                    tracing::debug!(
                        blob_bytes = blob.len(),
                        path = %path_str,
                        "csv_blob offloaded to shmem (binary transport)"
                    );
                    return SearchPayload::ShmemBlob(path_str);
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        blob_bytes = blob.len(),
                        "csv_blob shmem write failed; falling back to inline JSON"
                    );
                }
            }
        }
        SearchPayload::InlineBlob(blob)
    }

    /// Gate for [`Self::try_pack_csv_blob`] — factored out so the
    /// per-condition comments stay readable without cluttering the
    /// method body.  Every early-return matches a numbered bullet in
    /// the caller's rustdoc.
    pub(crate) fn is_csv_blob_eligible(params: &SearchParams) -> bool {
        // ②
        if matches!(
            params.response_mode,
            Some(uffs_client::protocol::SearchResponseMode::Json)
        ) {
            return false;
        }
        // ③
        if !params.aggregations.is_empty() {
            return false;
        }
        // ④
        if params.output_file.is_some() {
            return false;
        }
        // ⑤ — only `csv` and `custom` are byte-reproducible from the
        // daemon.  `json` (NDJSON) and `table` (fixed-width) are
        // structural formats owned by the CLI; pre-format would drift.
        // `custom` is accepted from Phase 3 onward — the legacy drive
        // footer is emitted by [`Self::render_csv_blob_payload`] via
        // `uffs_format::write_legacy_drive_footer`.  An absent
        // `output_format` is treated as "caller did not opt in" and
        // rejected here — see [`Self::caller_opted_into_blob_payload`]
        // for the full rationale.
        Self::caller_opted_into_blob_payload(params)
    }

    /// Return `true` when the caller explicitly opted into receiving a
    /// pre-rendered text blob payload instead of structured
    /// [`uffs_client::protocol::response::SearchRow`]s.
    ///
    /// Both fast paths ([`Self::try_pack_paths_blob`] and
    /// [`Self::try_pack_csv_blob`]) irreversibly consume the inline
    /// row list and replace it with a UTF-8 blob variant
    /// ([`SearchPayload::InlineBlob`] or
    /// [`SearchPayload::ShmemBlob`]).  Callers that can't re-parse
    /// that blob (e.g. `uffs-mcp`, which feeds
    /// [`uffs_client::protocol::response::SearchRow`]s into its
    /// tool-result JSON envelope) would see an "unexpected non-rows
    /// payload" error as a result.
    ///
    /// The opt-in signal is
    /// [`SearchParams::output_format`](uffs_client::protocol::SearchParams::output_format):
    /// the CLI always populates it (defaulting to `"csv"` when the
    /// user omits `--format`; see `SearchParams::from_cli_args` in
    /// `uffs-client`), while non-CLI callers leave it `None`.  The
    /// blob fast paths are therefore eligible only when
    /// `output_format == Some("csv" | "custom")` — the two formats
    /// the shared [`uffs_format::write_rows`] writer can reproduce
    /// byte-for-byte.  `"json"` / `"table"` stay on the CLI's local
    /// formatter (structural formats the daemon does not emit) and
    /// `None` stays on structured rows (non-CLI callers).
    pub(crate) fn caller_opted_into_blob_payload(params: &SearchParams) -> bool {
        params.output_format.as_deref().is_some_and(|fmt| {
            fmt.eq_ignore_ascii_case("csv") || fmt.eq_ignore_ascii_case("custom")
        })
    }

    /// Return `true` when the caller's `output_format` requests the
    /// legacy `custom` format.  Absent / empty / any non-"custom"
    /// value returns `false` — callers use `"csv"` or the absent
    /// default for footer-less CSV output.
    pub(crate) fn wants_custom_footer(params: &SearchParams) -> bool {
        params
            .output_format
            .as_deref()
            .is_some_and(|fmt| fmt.eq_ignore_ascii_case("custom"))
    }

    /// Translate a core [`uffs_core::output::OutputConfig`] into the
    /// corresponding [`uffs_format::OutputConfig`] the shared writer
    /// consumes.
    ///
    /// The two types have the same field layout; only the column
    /// enum differs (`FieldId` alias vs. `uffs_format::OutputColumn`).
    /// This wrapper mirrors the identically-named converter in
    /// `uffs_core::output::display_rows_format_bridge::field_id_to_format_column`
    /// — kept local to the handler so `uffs-daemon` does not need to
    /// depend on a test-only helper path in `uffs-core`.
    pub(crate) fn core_config_to_format(
        cfg: &uffs_core::output::OutputConfig,
    ) -> uffs_format::OutputConfig {
        let columns = cfg.columns.as_ref().map(|cols| {
            cols.iter()
                .copied()
                .map(uffs_core::output::display_rows_format_bridge::field_id_to_format_column)
                .collect::<Vec<_>>()
        });
        uffs_format::OutputConfig {
            columns,
            separator: cfg.separator.clone(),
            quote: cfg.quote.clone(),
            header: cfg.header,
            pos: cfg.pos.clone(),
            neg: cfg.neg.clone(),
            timezone_offset_secs: cfg.timezone_offset_secs,
            parity_compat: cfg.parity_compat,
        }
    }
}
