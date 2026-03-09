# uffs-cli Lint Cleanup Changelog

**Date:** 2026-03-08
**Crate:** uffs-cli
**Scope:** Remove all 32 blanket `#[allow(...)]` suppressions

## Summary

Converted all 32 `#[allow(...)]` attributes across `main.rs` (6) and `commands.rs` (26) to narrow `#[expect(lint, reason = "...")]` with meaningful justifications. Fixed the `#[cfg_attr(not(windows), allow(unused_variables))]` on `no_cache` parameter by removing the `cfg_attr` and adding `_ = no_cache;` in the non-windows code path.

## Changes

### main.rs (6 suppressions)

| Line | Lint | Fix |
|------|------|-----|
| 77 | `dead_code` on `Personality` enum | `#[expect(dead_code, reason = "...")]` — variants reserved for future modes |
| 163 | `struct_excessive_bools` on `Cli` | `#[expect]` — CLI args struct mirrors many boolean flags from clap |
| 393 | `single_call_fn` on `init_logging` | `#[expect]` — extracted for clarity |
| 460 | `expect_used` on `set_global_default` | `#[expect]` — panic intentional if called twice |
| 472 | `too_many_lines, single_call_fn` on `run()` | Split into separate `#[expect]` with reasons |
| 556 | `print_stderr` on `main()` | `#[expect]` — intentional user-facing error output |

### commands.rs (26 suppressions)

All `#[allow(...)]` converted to `#[expect(lint, reason = "...")]` with appropriate justifications:

- **`single_call_fn`** (15 instances): Justified as extracted for clarity, testability, or line count reduction
- **`print_stderr` / `print_stdout`** (8 instances): Justified as intentional user-facing or C++ compatibility output
- **`too_many_lines`** (3 instances): Justified as top-level orchestrators or structured displays
- **`unsafe_code`** (1 instance): `set_var` called once at startup in main thread
- **`cast_precision_loss` / `float_arithmetic`** (2 instances): Justified for human-readable display
- **`too_many_arguments` / `fn_params_excessive_bools`** (1 instance): CLI entry point passes through all args
- **`unused_async`** (2 instances): Platform stubs matching Windows async signatures
- **`min_ident_chars` / `option_if_let_else`** (1 instance each): DataFrame-conventional short names; clearer if-let chains
- **`semicolon_outside_block` / `semicolon_inside_block`** (2 instances): Mixed styles from cfg/unsafe blocks
- **`cast_possible_truncation`** (1 instance): Small values safe for u16/u32

### cfg_attr fix (line 417)

- **Before:** `#[cfg_attr(not(windows), allow(unused_variables))] no_cache: bool`
- **After:** Removed `cfg_attr`, added `_ = no_cache;` in the `#[cfg(not(windows))]` block to consume the variable

## Validation

- `cargo fmt -p uffs-cli` — passed
- `cargo clippy -p uffs-cli --all-targets --all-features --no-deps -- -D clippy::pedantic ...` — see verification
- `cargo test -p uffs-cli --all-features --locked` — see verification
- `rg '#[allow|#![allow' crates/uffs-cli/src/` — 0 matches
