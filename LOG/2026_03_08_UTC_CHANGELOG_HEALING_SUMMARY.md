# Lint Cleanup Summary

**Date:** 2026-03-08 UTC
**Branch:** `rust-lint-cleanup`

## Before/After

| Metric | Before | After |
|--------|--------|-------|
| Blanket `#[allow]` attributes | ~430 | 0 (in-scope crates) |
| Targeted `#[expect]` attributes | 0 | 608 |
| Remaining `#[allow]` in `uffs-legacy/` (out of scope) | 3 | 3 |

## Per-Crate Summary

| Crate | Before (`#[allow]`) | After (`#[expect]` outer) | After (`#![expect]` inner) | After (`#[allow]`) |
|-------|---------------------|---------------------------|----------------------------|--------------------|
| uffs-cli | 32 | 55 | 0 | 0 |
| uffs-core | 72 | 43 | 35 | 0 |
| uffs-diag | 48 | 38 | 34 | 0 |
| uffs-gui | 7 | 3 | 1 | 0 |
| uffs-mft | 246 | 371 | 10 | 0 |
| uffs-tui | 19 | 17 | 1 | 0 |
| uffs-polars | 1 | 0 | 0 | 0 |

**Note:** Counts shifted because blanket `#[allow]` on structs/enums were split into
targeted `#[expect]` on individual fields, methods, or variants — increasing the attribute
count while making each suppression specific and justified.

## Commits

```
95174be8d fix(uffs-polars): remove blanket allow for module_name_repetitions
0e5f2e3d8 fix(uffs-tui): remove blanket allows and fix underlying lint issues
7dc38914f fix(uffs-gui): remove blanket allows and fix underlying lint issues
b81804ff4 style: apply rustfmt to Wave 1 changes
1f5248101 fix(uffs-diag): convert all 48 blanket #[allow] to targeted #[expect] with reasons
6e441bd5d fix(uffs-cli): convert all blanket #[allow] to targeted #[expect] with reasons
c399aacab fix(uffs-core): convert all blanket #[allow] to targeted #[expect] with reasons
0a186c22a style: apply rustfmt to Wave 2 changes
52b61110f fix(uffs-mft): convert all blanket #[allow] to targeted #[expect] with reasons
4012d774c style: apply rustfmt to Wave 3 changes
```

## Known Issues

- polars-arrow SIMD build failure on nightly-2025-12-15 prevents full workspace clippy/test validation
- This is pre-existing and NOT caused by lint cleanup changes
- All per-crate static analysis (suppression audit, fmt check, config integrity) passes
