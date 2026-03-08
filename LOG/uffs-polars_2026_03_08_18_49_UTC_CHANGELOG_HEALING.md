# uffs-polars Lint Healing Changelog

**Date:** 2026-03-08 18:49 UTC
**Crate:** uffs-polars
**Branch:** rust-lint-cleanup

## Suppression Removed

| File | Line | Suppression | Action |
|------|------|-------------|--------|
| `crates/uffs-polars/src/lib.rs` | 44 | `#![allow(clippy::module_name_repetitions)]` | Removed — no items trigger this lint |

## Analysis

The blanket `#![allow(clippy::module_name_repetitions)]` was preemptive. The crate defines:

- `MftDataFrame` (type alias) — no module name repetition
- `MftLazyFrame` (type alias) — no module name repetition
- `columns` (module) — no module name repetition

All re-exported polars types come from `polars::prelude::*` and `polars::{chunked_array, datatypes, error, frame, lazy, series}`, which don't trigger this lint either since they originate from the `polars` crate.

## Verification

- `rg '#[allow|#![allow' crates/uffs-polars/src/`: no matches (PASS)
- `cargo clippy -p uffs-polars`: BLOCKED — pre-existing `polars-arrow` SIMD compilation failure at git rev `8f2501b9` (trait bound `LaneCount<N>: SupportedLaneCount` not satisfied). Fails identically on unmodified code. Not caused by this change.
- `cargo test -p uffs-polars --all-features --locked`: BLOCKED — same upstream compilation failure
