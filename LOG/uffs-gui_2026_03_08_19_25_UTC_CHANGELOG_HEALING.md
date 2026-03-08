# uffs-gui Lint Cleanup Changelog

**Date**: 2026-03-08 19:25 UTC
**Crate**: `uffs-gui`
**Branch**: `rust-lint-cleanup`

## Summary

Removed all 6 blanket `#![allow(...)]` crate-root suppressions from `crates/uffs-gui/src/main.rs`.
Fixed underlying lint issues and converted necessary suppressions to narrow `#[expect(lint, reason="...")]`.

## What Was Removed

| Suppression | Resolution |
|---|---|
| `#![allow(clippy::print_stdout)]` | Removed — no `println!` calls exist |
| `#![allow(clippy::print_stderr)]` | Replaced with `#[expect(clippy::print_stderr, ...)]` on `main()` |
| `#![allow(clippy::use_debug)]` | Removed — no `Debug` formatting used |
| `#![allow(clippy::single_call_fn)]` | Replaced with `#[expect(clippy::single_call_fn, ...)]` on `init_logging()` |
| `#![allow(clippy::missing_docs_in_private_items)]` | Removed — all items already have doc comments |
| `#![allow(unused_crate_dependencies)]` | Replaced with `#![expect(unused_crate_dependencies, ...)]` with reason listing placeholder deps |

## Additional Fixes

| Issue | Fix |
|---|---|
| `#[allow(clippy::expect_used)]` on `set_global_default` | Converted to `#[expect(clippy::expect_used, reason="...")]` |
| `std::process::exit(1)` triggers `clippy::exit` (deny) | Changed to `fn main() -> std::process::ExitCode` returning `ExitCode::FAILURE` |
| `use std::fs;` inside function body | Moved to top-level imports to avoid `clippy::items_after_statements` |

## Remaining `#[expect]` Annotations (4 total)

1. `#![expect(unused_crate_dependencies, ...)]` — crate root, placeholder deps for future GUI
2. `#[expect(clippy::single_call_fn, ...)]` — `init_logging()`, logically separate from main
3. `#[expect(clippy::expect_used, ...)]` — `set_global_default()`, panic is correct on failure
4. `#[expect(clippy::print_stderr, ...)]` — `main()`, placeholder banner intentionally uses stderr

## Validation

Clippy and test validation commands could not complete due to a **pre-existing polars-arrow SIMD build failure** (`LaneCount<N>: SupportedLaneCount` not satisfied). This is a known issue unrelated to uffs-gui changes. The polars-arrow crate is a transitive dependency via `uffs-polars` and blocks all compilation of uffs-gui.

The changes were verified by:
- Manual lint analysis against all workspace lint rules
- Confirming zero `#[allow(...)]` remaining via grep
- Confirming all `#[expect]` have reason strings
- Reviewing against uffs-tui (already cleaned) as reference pattern
