# uffs-tui Lint Suppression Cleanup

**Date**: 2026-03-08 18:50 UTC
**Crate**: `uffs-tui`
**Branch**: `rust-lint-cleanup`

## What Failed

The crate had 15 blanket `#![allow(...)]` inner attributes at crate root plus 4 item-level
`#[allow(...)]` attributes, totaling 19 lint suppressions that masked potential code quality issues.

## Why It Failed

Blanket allows were added as a convenience during initial TUI development, suppressing entire
categories of lints across the whole crate rather than addressing specific items.

## How It Was Fixed

### Removed (15 crate-root blanket allows)
All `#![allow(...)]` attributes removed from `main.rs` lines 18-32:
- `clippy::single_call_fn`, `clippy::indexing_slicing`, `clippy::min_ident_chars`
- `clippy::missing_docs_in_private_items`, `clippy::str_to_string`, `clippy::print_stderr`
- `clippy::use_debug`, `clippy::wildcard_enum_match_arm`, `clippy::missing_asserts_for_indexing`
- `clippy::option_if_let_else`, `clippy::ref_patterns`, `clippy::shadow_unrelated`
- `clippy::doc_markdown`, `dead_code`, `unused_crate_dependencies`

### Root Cause Fixes
- **`ref_patterns`**: Changed `Some(ref err)` to `Some(err) = &app.error` (idiomatic borrow)
- **`shadow_unrelated`**: Renamed shadowed `app` to `fallback` in error branch
- **`str_to_string`**: Changed `.to_string()` to `.to_owned()` on string literals
- **`min_ident_chars`**: Renamed `f` → `frame`, `c` → `ch`, `r` → `result`, `i` → `idx`/`current`, `p` → `path`, `q` → `filtered`, `c` → `col`
- **`missing_docs_in_private_items`**: Added doc comments to `mod app`, `main()`, `run_app()`, `ui()`
- **`needless_pass_by_value`**: Changed `load_index(&PathBuf)` to `load_index(&Path)`

### Converted to Narrow `#[expect]` (with reasons)
- `unused_crate_dependencies` — crate-level, tokio is transitive dependency
- `clippy::single_call_fn` — on `init_logging`, `run_app`, `ui`, `format_size`, `dataframe_to_results`
- `clippy::expect_used` — on `set_global_default` (unrecoverable startup failure)
- `clippy::wildcard_enum_match_arm` — on `run_app` (idiomatic key dispatch)
- `clippy::indexing_slicing` — on `ui` (layout guarantees chunk count)
- `clippy::missing_asserts_for_indexing` — on `ui` (layout guarantees chunk count)
- `clippy::option_if_let_else` — on `ui` (readability for widget branches)
- `clippy::print_stderr` — on error reporting block (terminal already restored)
- `clippy::use_debug` — on error reporting block (debug for full error chain)
- `clippy::cast_precision_loss` — on `format_size` (f64 sufficient for file sizes)
- `clippy::float_arithmetic` — on `format_size` (float division for size formatting)
- `clippy::partial_pub_fields` — on `App` struct (dataframe intentionally private)
- `dead_code` — on `frs` field and `load_index` method (planned features)

### Removed Without Replacement
- `clippy::struct_field_names` — lint doesn't actually fire on these structs
- `clippy::doc_markdown` — doc comments already use proper backtick formatting

## Files Changed
- `crates/uffs-tui/src/main.rs` — removed 15 blanket allows, added narrow expects, fixed root causes
- `crates/uffs-tui/src/app.rs` — removed 4 allows, added narrow expects, fixed root causes

## Verification Status
- `rg '#[allow|#![allow' crates/uffs-tui/src/`: **PASS** (no matches)
- `cargo clippy -p uffs-tui`: **BLOCKED** (pre-existing polars-arrow SIMD build failure)
- `cargo test -p uffs-tui`: **BLOCKED** (same polars-arrow dependency issue)

The polars-arrow dependency from git has a known SIMD incompatibility with the pinned
nightly-2025-12-15 toolchain. This is a pre-existing issue not caused by these changes.
