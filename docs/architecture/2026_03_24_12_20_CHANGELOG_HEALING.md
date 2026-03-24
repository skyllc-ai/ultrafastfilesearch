# Changelog Healing — 2026-03-24 12:20

## Context
TUI Wave 1 shipped with commit `88389c040`. Pipeline failed with ~40 pedantic
clippy errors in `uffs-tui` crate.

## What Failed
`04-parallel-validation` step — production clippy linting with `-D clippy::pedantic`
flagged multiple issues in the new TUI code:

### Error Categories
1. **`min_ident_chars`**: Single-char variables (`r`, `g`, `b`) in color functions
2. **`doc_markdown`**: Missing backticks in doc comments
3. **`float_arithmetic`**: Float division in `format_ms_compact`
4. **`cast_precision_loss`**: `u128 as f64` in time formatting
5. **`indexing_slicing`**: String indexing in `devicon_color` and `highlight_matches`
6. **`struct_field_names`**: `LoadTiming` fields all end in `_ms`
7. **`unnecessary_wrapping_result`**: `build_drive_index` returns `Result` but never errors
8. **`single_call_fn`**: New helper functions called once
9. **`too_many_lines`**: `run_app` function exceeds 100 lines
10. **`wildcard_enum_match_arm`**: Missing explicit match arms
11. **`unfulfilled_lint_expectations`**: Old `#[expect]` that no longer applies
12. **`iter_over_hash_type`**: Iterating over HashMap in `build_drive_colors`

## Fixes Applied
- Renamed single-char variables to descriptive names
- Added backticks to doc comments for code references
- Added `#[expect]` for intentional float arithmetic and precision loss
- Replaced string indexing with safe `.get()` or byte iteration
- Renamed `LoadTiming` fields to remove `_ms` postfix (use doc instead)
- Changed `build_drive_index` to not wrap in unnecessary `Result`
- Added appropriate `#[expect]` for single_call_fn, too_many_lines
- Fixed wildcard match arms
- Removed unfulfilled lint expectations
- Sorted HashMap iteration for deterministic output
