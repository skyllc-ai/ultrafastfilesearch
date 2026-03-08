# Remaining Lint Suppressions Report

**Date:** 2026-03-08 UTC
**Branch:** `rust-lint-cleanup`
**Scope:** All `.rs` files excluding `crates/uffs-legacy/`, `target/`, `vendor/`

## Summary

| Metric | Count |
|--------|-------|
| Total suppression attributes | 671 |
| `#[expect(...)]` (targeted) | 527 |
| `#[allow(...)]` (blanket) | 62 |
| `cfg_attr(...)` conditional | 0 |

### Per-Crate Breakdown

| Crate | Suppressions | `#[expect]` | `#[allow]` |
|-------|-------------|-------------|------------|
| docs (reference only) | 58 | 0 | 57 |
| scripts (standalone) | 5 | 0 | 5 |
| uffs-cli | 55 | 55 | 0 |
| uffs-core | 78 | 43 | 0 |
| uffs-diag | 72 | 38 | 0 |
| uffs-gui | 4 | 3 | 0 |
| uffs-mft | 381 | 371 | 0 |
| uffs-tui | 18 | 17 | 0 |

## Detailed Listing

### docs (reference only) (58 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./docs/architecture/Investigation/cpp_tree_two_channel_patched.rs` | 55 | `clippy::missing_const_for_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/cpp_tree_two_channel_patched.rs` | 127 | `clippy::missing_const_for_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/cpp_tree_two_channel_patched.rs` | 169 | `clippy::cast_possible_truncation, clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 568 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 640 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 665 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 781 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1161 | `clippy::cast_possible_truncation, clippy::cast_sign_loss` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1175 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1415 | `clippy::cast_possible_truncation, clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1504 | `clippy::string_slice` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1612 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1655 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1788 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1801 | `clippy::print_stdout` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 1806 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 2190 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 2481 | `clippy::missing_const_for_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 2496 | `clippy::missing_const_for_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 2771 | `clippy::while_let_loop` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 3052 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 3204 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 3356 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 3462 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 3514 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 4299 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 4464 | `clippy::cast_possible_truncation, clippy::too_many_lines` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 4695 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5067 | `clippy::cognitive_complexity, clippy::too_many_lines` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5109 | `clippy::cast_possible_truncation, clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5133 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5166 | `clippy::cast_possible_truncation, clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5231 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5249 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5267 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5312 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5504 | `clippy::cast_possible_truncation, clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5653 | `clippy::too_many_lines` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5769 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5775 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5784 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/index.rs` | 5809 | `(multiline/partial)` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 18 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 61 | `clippy::struct_excessive_bools` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 120 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 127 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 260 | `unsafe_code` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 449 | `unsafe_code, clippy::single_call_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 507 | `unsafe_code, clippy::single_call_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 537 | `clippy::missing_asserts_for_indexing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 562 | `clippy::single_call_fn` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 580 | `clippy::missing_asserts_for_indexing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 732 | `unsafe_code, clippy::too_many_lines` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 1230 | `unsafe_code, clippy::too_many_lines, clippy::cognitive_complexity` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 1869 | `clippy::indexing_slicing, clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 1892 | `clippy::indexing_slicing, clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 1965 | `clippy::indexing_slicing` | #[allow] | (see next line / multiline) |
| `./docs/architecture/Investigation/parse.rs` | 2546 | `clippy::cast_possible_truncation` | #[allow] | (see next line / multiline) |

### scripts (standalone) (5 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./scripts/analyze_trial_outputs.rs` | 29 | `dead_code` | #[allow] | (see next line / multiline) |
| `./scripts/analyze_trial_parity.rs` | 385 | `dead_code` | #[allow] | (see next line / multiline) |
| `./scripts/build-cross-all.rs` | 101 | `dead_code` | #[allow] | (see next line / multiline) |
| `./scripts/build-cross-all.rs` | 107 | `dead_code` | #[allow] | (see next line / multiline) |
| `./scripts/ci-pipeline.rs` | 445 | `dead_code` | #[allow] | (see next line / multiline) |

### uffs-cli (55 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-cli/src/commands.rs` | 17 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 298 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 347 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 351 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 355 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 409 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 414 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 418 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 422 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 426 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 603 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 617 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 687 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 691 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 711 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 727 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 809 | `clippy::single_call_fn` | #[expect] | extracted from search() for clarity |
| `./crates/uffs-cli/src/commands.rs` | 810 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 867 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 871 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 875 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 991 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 995 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1127 | `clippy::single_call_fn` | #[expect] | extracted from search() for clarity |
| `./crates/uffs-cli/src/commands.rs` | 1128 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1191 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1195 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1366 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1418 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1422 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1491 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1495 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1499 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1503 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1507 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 1745 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2219 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2223 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2538 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2716 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2720 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2736 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2821 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2855 | `clippy::single_call_fn` | #[expect] | extracted for clarity |
| `./crates/uffs-cli/src/commands.rs` | 2856 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2974 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 2978 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/commands.rs` | 3010 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 77 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 166 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 399 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 469 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 484 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 488 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-cli/src/main.rs` | 575 | `(multiline/partial)` | #[expect] | (see next line / multiline) |

### uffs-core (78 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-core/benches/query.rs` | 11 | `clippy::missing_docs_in_private_items` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/query.rs` | 12 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 16 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 20 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 24 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 28 | `clippy::default_numeric_fallback` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/query.rs` | 29 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 33 | `clippy::semicolon_if_nothing_returned` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/query.rs` | 34 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 38 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 42 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 46 | `clippy::doc_markdown` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/query.rs` | 47 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 51 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/query.rs` | 55 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 14 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 18 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 22 | `clippy::let_underscore_untyped` | inner attribute | benchmarks discard results |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 23 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 27 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 31 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 35 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 39 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 43 | `clippy::missing_docs_in_private_items` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 44 | `clippy::missing_panics_doc` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 45 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 49 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 53 | `clippy::semicolon_if_nothing_returned` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 54 | `clippy::semicolon_inside_block` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 55 | `clippy::semicolon_outside_block` | inner attribute | benchmark code |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 56 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 60 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 64 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 68 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/benches/search_benchmarks.rs` | 72 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-core/src/compiled_pattern.rs` | 129 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/extensions.rs` | 371 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/glob.rs` | 19 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/index_search.rs` | 38 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/index_search.rs` | 568 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/index_search.rs` | 775 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/index_search.rs` | 815 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/index_search.rs` | 1181 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1189 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1199 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1209 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1219 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1229 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1248 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/index_search.rs` | 1365 | `clippy::unwrap_used` | #[expect] | test code — unwrap on controlled data |
| `./crates/uffs-core/src/lib.rs` | 80 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 18 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 270 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 385 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 521 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 572 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 576 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 686 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 690 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/output.rs` | 694 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 51 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 151 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 223 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 244 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 297 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/path_resolver.rs` | 318 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/pattern.rs` | 68 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/pattern.rs` | 102 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 136 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 262 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 621 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 625 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 629 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 633 | `clippy::print_stdout` | #[expect] | benchmark test outputs timing info |
| `./crates/uffs-core/src/tree.rs` | 634 | `clippy::use_debug` | #[expect] | benchmark test outputs debug info |
| `./crates/uffs-core/src/tree.rs` | 635 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 639 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-core/src/tree.rs` | 643 | `clippy::let_underscore_untyped` | #[expect] | test code discards results |

### uffs-diag (72 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 21 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 25 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 31 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 37 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 41 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 45 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 50 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 54 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_diff.rs` | 479 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 20 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 24 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 41 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 83 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 97 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/analyze_mft_parents.rs` | 202 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 12 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 16 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 56 | `dead_code` | #[expect] | parsed from binary format but not yet used |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 95 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 109 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_raw_mft.rs` | 113 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 38 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 44 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 48 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 52 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 62 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 66 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 70 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 74 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 79 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 83 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 87 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 112 | `dead_code` | #[expect] | utility for future column normalization |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 171 | `dead_code` | #[expect] | available for detailed reporting |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 278 | `dead_code` | #[expect] | utility for future drive-level analysis |
| `./crates/uffs-diag/src/bin/compare_scan_parity.rs` | 301 | `dead_code` | #[expect] | utility for future field comparison |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 22 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 26 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 37 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 107 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 131 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 145 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 202 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 206 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 307 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/cross_check_mft_reference.rs` | 381 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_extents.rs` | 16 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_extents.rs` | 20 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 13 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 17 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 33 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 171 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 175 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 179 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 183 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 187 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 387 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/dump_mft_records.rs` | 391 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 9 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 13 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 26 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 131 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 135 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 206 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 210 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 214 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/scan_mft_magic.rs` | 9 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/scan_mft_magic.rs` | 13 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/scan_mft_magic.rs` | 29 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/scan_mft_magic.rs` | 54 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/bin/scan_mft_magic.rs` | 134 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-diag/src/lib.rs` | 6 | `(multiline/partial)` | inner attribute | (see next line / multiline) |

### uffs-gui (4 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-gui/src/main.rs` | 20 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-gui/src/main.rs` | 52 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-gui/src/main.rs` | 113 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-gui/src/main.rs` | 124 | `(multiline/partial)` | #[expect] | (see next line / multiline) |

### uffs-mft (381 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-mft/src/cache.rs` | 236 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_io_pipeline.rs` | 317 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_io_pipeline.rs` | 321 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_tree.rs` | 19 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_tree.rs` | 99 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 38 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 71 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 92 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 114 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 133 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 151 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 165 | `clippy::cast_possible_wrap` | #[expect] | values > i64::MAX are clamped |
| `./crates/uffs-mft/src/cpp_types.rs` | 1257 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1453 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1641 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1724 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1888 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1899 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 1996 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2034 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2120 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2125 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2129 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2227 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2232 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2279 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2284 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2403 | `clippy::single_call_fn` | #[expect] | helper function for readability |
| `./crates/uffs-mft/src/cpp_types.rs` | 2514 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2515 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2737 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2808 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/cpp_types.rs` | 2809 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2877 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2904 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2908 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2912 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2916 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 2920 | `clippy::semicolon_outside_block` | #[expect] | test code style |
| `./crates/uffs-mft/src/cpp_types.rs` | 2921 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3081 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3085 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3089 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3093 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3097 | `clippy::semicolon_outside_block` | #[expect] | test code style |
| `./crates/uffs-mft/src/cpp_types.rs` | 3098 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3258 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3262 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3266 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3270 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3274 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3278 | `clippy::semicolon_outside_block` | #[expect] | test code style |
| `./crates/uffs-mft/src/cpp_types.rs` | 3279 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3283 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3287 | `clippy::single_call_fn` | #[expect] | test helpers extracted for clarity |
| `./crates/uffs-mft/src/cpp_types.rs` | 3578 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3582 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3586 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3590 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3594 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3598 | `clippy::semicolon_outside_block` | #[expect] | test code style |
| `./crates/uffs-mft/src/cpp_types.rs` | 3599 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3603 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3607 | `clippy::single_call_fn` | #[expect] | test helpers extracted for clarity |
| `./crates/uffs-mft/src/cpp_types.rs` | 3608 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 3997 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4001 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4005 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4009 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4013 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4017 | `clippy::semicolon_outside_block` | #[expect] | test code style |
| `./crates/uffs-mft/src/cpp_types.rs` | 4018 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4022 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/cpp_types.rs` | 4026 | `clippy::single_call_fn` | #[expect] | test helpers extracted for clarity |
| `./crates/uffs-mft/src/index.rs` | 1059 | `clippy::cast_possible_truncation` | #[expect] | checked: id < u16::MAX |
| `./crates/uffs-mft/src/index.rs` | 1131 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 1159 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 1278 | `clippy::cast_possible_truncation` | #[expect] | record count < u32::MAX |
| `./crates/uffs-mft/src/index.rs` | 1714 | `clippy::cast_possible_truncation` | #[expect] | log2 result fits in usize |
| `./crates/uffs-mft/src/index.rs` | 1715 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 1732 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 1979 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 1983 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2075 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2195 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2199 | `clippy::indexing_slicing` | #[expect] | bounds checked via get_or_create |
| `./crates/uffs-mft/src/index.rs` | 2251 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2255 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2282 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2349 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2485 | `clippy::cast_possible_truncation` | #[expect] | n < u32::MAX in practice |
| `./crates/uffs-mft/src/index.rs` | 2486 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2490 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2504 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2520 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2548 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2689 | `clippy::cast_possible_truncation` | #[expect] | index counts fit in usize |
| `./crates/uffs-mft/src/index.rs` | 2690 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2694 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2698 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2702 | `clippy::map_unwrap_or` | #[expect] | map().unwrap_or() is more readable |
| `./crates/uffs-mft/src/index.rs` | 2703 | `clippy::min_ident_chars` | #[expect] | 'n' for count is idiomatic |
| `./crates/uffs-mft/src/index.rs` | 2704 | `clippy::print_stdout` | #[expect] | intentional: debug output to stdout |
| `./crates/uffs-mft/src/index.rs` | 2705 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 2709 | `clippy::uninlined_format_args` | #[expect] | readability in debug prints |
| `./crates/uffs-mft/src/index.rs` | 2710 | `clippy::unnecessary_sort_by` | #[expect] | explicit sort_by is clearer |
| `./crates/uffs-mft/src/index.rs` | 3125 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 3129 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 3133 | `clippy::too_many_lines` | #[expect] | stats display has many fields |
| `./crates/uffs-mft/src/index.rs` | 3134 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 3138 | `clippy::min_ident_chars` | #[expect] | 'n' for count is idiomatic |
| `./crates/uffs-mft/src/index.rs` | 3423 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 3441 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 3719 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4020 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4024 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4028 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4032 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4036 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4040 | `clippy::print_stdout` | #[expect] | test diagnostics output |
| `./crates/uffs-mft/src/index.rs` | 4041 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4045 | `clippy::std_instead_of_core` | #[expect] | test code uses std types |
| `./crates/uffs-mft/src/index.rs` | 4046 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4050 | `clippy::uninlined_format_args` | #[expect] | test code readability |
| `./crates/uffs-mft/src/index.rs` | 4051 | `clippy::use_debug` | #[expect] | test code uses Debug for assertions |
| `./crates/uffs-mft/src/index.rs` | 4052 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4056 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4201 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4356 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4465 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 4520 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 5309 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6026 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6283 | `clippy::cast_possible_truncation` | #[expect] | index counts fit in usize |
| `./crates/uffs-mft/src/index.rs` | 6284 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6520 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6524 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6528 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 6532 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7052 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7056 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7101 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7105 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7154 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7200 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7204 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7317 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7321 | `clippy::indexing_slicing` | #[expect] | indices validated before access |
| `./crates/uffs-mft/src/index.rs` | 7322 | `clippy::too_many_lines` | #[expect] | name/stream merge has many steps |
| `./crates/uffs-mft/src/index.rs` | 7485 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7506 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7527 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7542 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7590 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7793 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7797 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 7950 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 8071 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 8080 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 8092 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 8120 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/index.rs` | 8124 | `clippy::cast_possible_truncation` | #[expect] | u64→usize safe on 64-bit |
| `./crates/uffs-mft/src/index.rs` | 8125 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 414 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 517 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 521 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 525 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 533 | `unused_imports` | #[expect] | used in inline parsing mode |
| `./crates/uffs-mft/src/io.rs` | 974 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 978 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 1337 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 1341 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 1345 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 1867 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 1871 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 2296 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 3620 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 3892 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 4304 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 4772 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 4795 | `unused_imports` | #[expect] | used in inline parsing mode |
| `./crates/uffs-mft/src/io.rs` | 5157 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 5529 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 5533 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6254 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6359 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6454 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6551 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6669 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6805 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 6991 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7217 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7292 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7315 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7341 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7405 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7489 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7836 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/io.rs` | 7840 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/lib.rs` | 49 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 128 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 132 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 768 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 832 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 843 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 847 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 870 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 874 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 878 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 1010 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 1014 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 1860 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2585 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2719 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2723 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2729 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2733 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2753 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2757 | `clippy::print_stdout` | #[expect] | intentional user-facing cli output |
| `./crates/uffs-mft/src/main.rs` | 2758 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2762 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2766 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2770 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2774 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2859 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2863 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2869 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 2873 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 3085 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 3102 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 3267 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 4171 | `unsafe_code` | #[expect] | required for windows ffi call to CloseHandle |
| `./crates/uffs-mft/src/main.rs` | 5029 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/main.rs` | 5033 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 115 | `clippy::cast_sign_loss` | #[expect] | checked positive above |
| `./crates/uffs-mft/src/ntfs.rs` | 119 | `clippy::cast_sign_loss` | #[expect] | negated negative value is positive |
| `./crates/uffs-mft/src/ntfs.rs` | 128 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 195 | `clippy::indexing_slicing` | #[expect] | bounds checked before each access |
| `./crates/uffs-mft/src/ntfs.rs` | 250 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 585 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 690 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 780 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 814 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 815 | `clippy::missing_const_for_fn` | #[expect] | can't be const due to unsafe |
| `./crates/uffs-mft/src/ntfs.rs` | 869 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 870 | `clippy::indexing_slicing` | #[expect] | bounds checked before access |
| `./crates/uffs-mft/src/ntfs.rs` | 898 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 899 | `clippy::indexing_slicing` | #[expect] | bounds checked before access |
| `./crates/uffs-mft/src/ntfs.rs` | 926 | `clippy::indexing_slicing` | #[expect] | bounds checked before access |
| `./crates/uffs-mft/src/ntfs.rs` | 963 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 964 | `clippy::indexing_slicing` | #[expect] | bounds checked before access |
| `./crates/uffs-mft/src/ntfs.rs` | 1024 | `clippy::cast_sign_loss` | #[expect] | lcn is checked positive before cast |
| `./crates/uffs-mft/src/ntfs.rs` | 1056 | `clippy::similar_names` | #[expect] | vcn and lcn are standard NTFS terms |
| `./crates/uffs-mft/src/ntfs.rs` | 1057 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1107 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1120 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1133 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1137 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1174 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/ntfs.rs` | 1175 | `clippy::indexing_slicing` | #[expect] | bounds checked before access |
| `./crates/uffs-mft/src/ntfs.rs` | 1262 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1350 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/ntfs.rs` | 1418 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 18 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 22 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 26 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 30 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 34 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 38 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 42 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 80 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 142 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 152 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 294 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/parse.rs` | 483 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/parse.rs` | 484 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 545 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS struct |
| `./crates/uffs-mft/src/parse.rs` | 546 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 579 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 607 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 628 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 787 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS structs |
| `./crates/uffs-mft/src/parse.rs` | 788 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 792 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 1331 | `unsafe_code` | #[expect] | FFI: ptr::read for packed NTFS structs |
| `./crates/uffs-mft/src/parse.rs` | 1332 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 1336 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 2286 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 2290 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 2316 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 2320 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 2459 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/parse.rs` | 3111 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 59 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 64 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 121 | `unsafe_code` | #[expect] | FFI: windows API (CreateFileW) |
| `./crates/uffs-mft/src/platform.rs` | 188 | `unsafe_code` | #[expect] | FFI: windows API (DeviceIoControl) |
| `./crates/uffs-mft/src/platform.rs` | 263 | `unsafe_code` | #[expect] | FFI: windows API (CreateFileW) |
| `./crates/uffs-mft/src/platform.rs` | 320 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 369 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 426 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 435 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 443 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 658 | `unsafe_code` | #[expect] | FFI: windows API (CloseHandle) |
| `./crates/uffs-mft/src/platform.rs` | 703 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 824 | `unsafe_code` | #[expect] | FFI: windows API (CloseHandle) |
| `./crates/uffs-mft/src/platform.rs` | 1131 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 1248 | `unsafe_code` | #[expect] | FFI: windows API (GetLogicalDrives) |
| `./crates/uffs-mft/src/platform.rs` | 1280 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/platform.rs` | 1342 | `unsafe_code` | #[expect] | FFI: windows API (GetVolumeInformationW) |
| `./crates/uffs-mft/src/platform.rs` | 1536 | `unsafe_code` | #[expect] | FFI: windows API (DeviceIoControl) |
| `./crates/uffs-mft/src/platform.rs` | 1696 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 221 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 232 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 260 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 271 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 279 | `unused_mut` | #[expect] | mutated only when zstd feature is enabled |
| `./crates/uffs-mft/src/raw.rs` | 281 | `unused_mut` | #[expect] | mutated only when zstd feature is enabled |
| `./crates/uffs-mft/src/raw.rs` | 286 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 335 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 368 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 537 | `dead_code` | #[expect] | used only when zstd feature is enabled |
| `./crates/uffs-mft/src/raw.rs` | 805 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 870 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 921 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 975 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 1021 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/raw.rs` | 1025 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 190 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 194 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 253 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 377 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 381 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 392 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 396 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 439 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 536 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 573 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 857 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 872 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 887 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 905 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1035 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1126 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1224 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1398 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1460 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 1478 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 1786 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 1857 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 1899 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 1943 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 2259 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 2477 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 2548 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 2590 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 2652 | `unsafe_code` | #[expect] | FFI: CloseHandle on valid overlapped handle |
| `./crates/uffs-mft/src/reader.rs` | 2759 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 2934 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3136 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3248 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3252 | `dead_code` | #[expect] | kept as fallback for legacy 8-column schema |
| `./crates/uffs-mft/src/reader.rs` | 3287 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3385 | `clippy::single_call_fn` | #[expect] | extracted for clarity |
| `./crates/uffs-mft/src/reader.rs` | 3483 | `dead_code` | #[expect] | utility for tests and potential future use |
| `./crates/uffs-mft/src/reader.rs` | 3529 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3544 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 3618 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3635 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3811 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 3837 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 3861 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 4069 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/reader.rs` | 4263 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 4294 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 4481 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 4515 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 4545 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/reader.rs` | 4667 | `clippy::unused_async` | #[expect] | async for API parity with windows |
| `./crates/uffs-mft/src/usn.rs` | 154 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-mft/src/usn.rs` | 208 | `(multiline/partial)` | #[expect] | (see next line / multiline) |

### uffs-tui (18 suppressions)

| File | Line | Lint(s) | Form | Reason |
|------|------|---------|------|--------|
| `./crates/uffs-tui/src/app.rs` | 18 | `dead_code` | #[expect] | populated for future detail view feature |
| `./crates/uffs-tui/src/app.rs` | 25 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/app.rs` | 79 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/app.rs` | 186 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 17 | `(multiline/partial)` | inner attribute | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 72 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 137 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 190 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 194 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 207 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 211 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 241 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 245 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 249 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 253 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 342 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 346 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
| `./crates/uffs-tui/src/main.rs` | 350 | `(multiline/partial)` | #[expect] | (see next line / multiline) |
