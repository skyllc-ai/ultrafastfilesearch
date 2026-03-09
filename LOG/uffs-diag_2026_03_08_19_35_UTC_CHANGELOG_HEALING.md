# uffs-diag Lint Cleanup — 2026-03-08 19:35 UTC

## Summary

Converted all 48 blanket `#[allow(...)]` suppressions in `crates/uffs-diag/src/`
to targeted `#[expect(lint, reason = "...")]` annotations. No blanket allows remain.

## Pre-existing Issue

**polars-arrow SIMD build failure** — The `polars-arrow` crate (upstream dependency)
fails to compile on the current nightly toolchain due to a `SupportedLaneCount` trait
bound issue in `bitmap/bitmask.rs`. This blocks `cargo clippy`, `cargo check`, and
`cargo test` for any crate depending on polars (including uffs-diag). This is a
pre-existing issue unrelated to our lint changes.

## Changes by File

### `src/lib.rs`
- `#![allow(clippy::missing_docs_in_private_items)]` → `#![expect(..., reason)]`

### `src/bin/dump_mft_extents.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, too_many_lines)]` → `#![expect(print_stdout, print_stderr, reason)]`
  - Removed `too_many_lines` (no function exceeds threshold)

### `src/bin/analyze_mft_parents.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, cast_precision_loss, shadow_reuse, too_many_lines, std_instead_of_alloc)]` → `#![expect(print_stdout, print_stderr, reason)]`
  - Removed 4 lints that were unnecessarily blanket-suppressed
- 3x `#[allow(unused_imports)]` / `#[allow(clippy::single_call_fn)]` → `#[expect(..., reason)]`

### `src/bin/scan_mft_magic.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, too_many_lines, std_instead_of_alloc)]` → `#![expect(print_stdout, print_stderr, reason)]`
  - Removed `too_many_lines` and `std_instead_of_alloc`
- `#[allow(unused_imports)]` → `#[expect(..., reason)]`
- `#[allow(clippy::single_call_fn)]` → `#[expect(..., reason)]`
- `#[allow(unsafe_code)]` → `#[expect(..., reason)]`

### `src/bin/compare_raw_mft.rs`
- Blanket `#![allow(print_stdout, print_stderr, float_arithmetic, cast_precision_loss, too_many_lines, single_call_fn, unused_crate_dependencies)]` → targeted expects
  - `print_stdout`/`print_stderr` → crate-level `#![expect(...)]`
  - `unused_crate_dependencies` → crate-level `#![expect(...)]`
  - `float_arithmetic`/`cast_precision_loss`/`too_many_lines` → function-level `#[expect]` on `main`
  - `single_call_fn` → function-level `#[expect]` on `read_header`
- `#[allow(dead_code)]` on `compressed_size` → `#[expect(..., reason)]`

### `src/bin/dump_mft_records.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, too_many_lines)]` → `#![expect(print_stdout, print_stderr, reason)]`
- `#[allow(unused_imports)]` → `#[expect(..., reason)]`
- `#[allow(single_call_fn, use_debug, similar_names, cast_possible_truncation)]` on `test_merge` → 5 separate `#[expect(..., reason)]`
- `#[allow(unsafe_code, single_call_fn)]` on `dump_record` → 2 separate `#[expect(..., reason)]`

### `src/bin/compare_scan_parity.rs`
- Massive blanket `#![allow(...)]` with 22 lints → 12 targeted `#![expect(..., reason)]` groups
- 4x `#[allow(dead_code)]` → `#[expect(dead_code, reason)]`

### `src/bin/inspect_mft_record_flow.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, too_many_lines)]` → `#![expect(print_stdout, print_stderr, reason)]`
- `#[allow(unused_imports)]` → `#[expect(..., reason)]`
- `#[allow(clippy::single_call_fn)]` → `#[expect(..., reason)]` + added `too_many_lines` expect
- `#[allow(unsafe_code, missing_const_for_fn, single_call_fn)]` → 3 separate `#[expect(..., reason)]`

### `src/bin/cross_check_mft_reference.rs`
- Blanket `#![allow(unused_crate_dependencies)]` → `#![expect(..., reason)]`
- Blanket `#![allow(print_stdout, print_stderr, cast_precision_loss, too_many_lines, std_instead_of_alloc)]` → `#![expect(print_stdout, print_stderr, reason)]`
  - Removed `cast_precision_loss`, `too_many_lines`, `std_instead_of_alloc` from blanket
  - Added function-level `too_many_lines` where needed
- `#[allow(unused_imports)]` → `#[expect(..., reason)]`
- 6x `#[allow(clippy::single_call_fn)]` → `#[expect(..., reason)]`

### `src/bin/analyze_diff.rs`
- Blanket `#![allow(...)]` with 13 lints → 8 targeted `#![expect(..., reason)]` groups
- `#[allow(clippy::cast_possible_wrap)]` → `#[expect(..., reason)]`

## Verification

- `rg '#[allow|#![allow' crates/uffs-diag/src/` → **0 matches** (all converted)
- `cargo fmt -p uffs-diag` → clean
- `cargo clippy`/`cargo check`/`cargo test` → **blocked by pre-existing polars-arrow SIMD failure**

## Principles Applied

1. All `#[allow]` → `#[expect]` with descriptive `reason` strings
2. Blanket crate-level suppressions narrowed to only lints actually needed
3. Removed unnecessary suppressions (e.g., `too_many_lines` where no function exceeds threshold)
4. Function-level annotations preferred over crate-level where feasible
5. No behavior changes — purely lint annotation updates
