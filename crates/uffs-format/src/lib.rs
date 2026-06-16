// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unified CSV / columnar output formatter for UFFS.
//!
//! This crate owns the single source of truth for the CSV / columnar
//! output bytes that both the daemon (`--out=file`) and the CLI (stdout
//! after receiving
//! `uffs_client::protocol::response::SearchPayload::InlineRows`) emit.  The
//! daemon and the thin CLI both delegate to [`write_rows`] so the two output
//! sites are byte-identical by construction — they literally run the same code.
//!
//! # Architecture
//!
//! The formatter is generic over a [`FormatRow`] trait so it doesn't
//! care which concrete row type its caller holds.  The daemon's
//! `uffs_core::search::backend::DisplayRow` and the CLI's
//! `uffs_client::protocol::response::SearchRow` both implement
//! `FormatRow`, so both callers can feed their native row type
//! through this crate without paying for a conversion copy.
//!
//! # Non-goals
//!
//! - No polars.  The daemon still has an `OutputConfig::write(DataFrame, …)`
//!   convenience method in `uffs-core`, but that is a separate code path that
//!   the load/CSV export uses; it does not flow through this crate.
//! - No JSON.  JSON output stays on the CLI side because it is structural, not
//!   columnar.
//! - No fixed-width "table" output.  Same reason — that lives on the CLI side
//!   in `uffs_cli::commands::output::write_table`.
//!
//! # Wire compatibility
//!
//! Every byte this crate emits is regression-pinned by
//! `uffs_format::tests::*` and by cross-crate byte-parity tests under
//! `uffs-client::output::tests::*` / `uffs-core::output::tests::*`.
//! Changing the canonical output is a breaking change for any pipeline
//! that consumes `uffs --search` output — bump the version and call it
//! out in the CHANGELOG.

#![forbid(unsafe_code)]

// Phase 3 module-layout: submodules are crate-internal organization.
// External callers consume the curated `pub use` items below; no
// downstream crate uses the `uffs_format::<sub>::Item` module path
// (verified by workspace-wide grep on 2026-05-13 post-PR #220).
pub(crate) mod attr;
pub(crate) mod column;
pub(crate) mod config;
pub(crate) mod datetime;
pub(crate) mod derived;
pub(crate) mod footer;
pub(crate) mod row;
pub(crate) mod writer;

pub use column::{BASELINE_COLUMN_ORDER, OutputColumn, PARITY_COLUMN_ORDER};
pub use config::OutputConfig;
pub use footer::{DriveFooterContext, write_legacy_drive_footer};
pub use row::FormatRow;
pub use writer::write_rows;
