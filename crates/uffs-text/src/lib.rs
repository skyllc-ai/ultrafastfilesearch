// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! # uffs-text: NTFS-Compatible Text Processing
//!
//! Layer 0 foundation crate providing NTFS-compatible case folding.
//! No internal crate dependencies.
//!
//! ## Current Capabilities
//!
//! - **[`case_fold::CaseFold`]**: NTFS `$UpCase` case folding engine (128 KB
//!   table, `Copy`, zero-alloc comparisons, buffer-reuse folding) — matches the
//!   on-disk `$UpCase` semantics of NTFS bit-for-bit, which differs in subtle
//!   ways from generic Unicode case folding and is the correct primitive for
//!   any filename comparison that must agree with the filesystem's own
//!   ordering.
//!
//! ## Scope
//!
//! This crate is intentionally minimal.  UFFS-index-specific helpers (e.g.
//! the trigram packers used by the search engine's CSR index) live in
//! `uffs-core::trigram_key` as crate-private utilities — they have no value
//! outside the index implementation and so do not appear in this crate's
//! publish surface.  Trigram packers were relocated on 2026-05-14 as part
//! of the crates.io publishability scrub (see
//! `docs/refactor/crates-io-publishability-deep-dive.md`).
//!
//! ## Future (i18n)
//!
//! - Unicode normalisation (NFC/NFD)
//! - Script detection
//! - Locale-aware collation
//! - Search tokenisation

// On docs.rs only: enable the `doc_cfg` rustdoc feature so cfg-gated items
// render with their cfg badge.  Gated behind `cfg(docsrs)` so local
// `cargo doc` never exercises the nightly-only feature.  Post-Rust-1.92
// the `doc_auto_cfg` feature was merged into `doc_cfg`
// (rust-lang/rust#138907) — `doc_cfg` is now the unified name.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod case_fold;
