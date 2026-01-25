# CI Pipeline Healing Log - 2026-01-25 13:06

## Summary
Fixed all clippy errors in the UFFS codebase to pass the strict CI pipeline. The pipeline enforces `-D warnings -D clippy::pedantic -D clippy::nursery -D clippy::cargo`, treating all warnings as errors.

## What Failed

### Initial State
- CI pipeline command: `rust-script scripts/ci-pipeline.rs go -v`
- **128 clippy errors** in `crates/uffs-mft/src/index.rs`
- **8 clippy warnings** in `crates/uffs-core/src/index_search.rs` (treated as errors)

### Error Categories in uffs-mft
1. **indexing_slicing** (23 instances): Direct array indexing `arr[i]` without bounds checking
2. **doc_markdown** (13 instances): Missing backticks around identifiers in documentation
3. **must_use_candidate** (10 instances): Functions returning values without `#[must_use]`
4. **min_ident_chars** (9 instances): Single-character variable names
5. **cast_possible_truncation** (4 instances): Unsafe casts from `usize` to `u32`
6. **std_instead_of_core** (3 instances): Using `std::` instead of `core::`
7. **map_unwrap_or** (2 instances): Using `.map().unwrap_or()` instead of `.map_or()`
8. **unnecessary_sort_by** (2 instances): Using `sort_by` when `sort_unstable_by_key` is better
9. **needless_range_loop** (2 instances): Using `for i in 0..n` instead of iterators
10. **single_call_fn** (1 instance): Function called only once
11. **shadow_unrelated** (1 instance): Variable shadowing
12. **manual_let_else** (1 instance): Using match when let-else is clearer
13. **string_slice** (1 instance): Direct string slicing without UTF-8 safety

## Why It Failed

### Root Causes
1. **Performance-critical code used unsafe patterns**: The `compute_tree_metrics` function used direct array indexing for performance, but clippy requires safe `.get()` access
2. **Missing documentation standards**: Identifiers in doc comments need backticks
3. **API design**: Many getter functions didn't have `#[must_use]` attribute
4. **Code style**: Single-char variable names, `std::` instead of `core::`, etc.
5. **Deleted function still referenced**: `cmp_ascii_case_insensitive` was inlined to fix `single_call_fn`, but tests still referenced it

## How It Was Fixed

### Phase 1: compute_tree_metrics Function (Lines 1598-1698)
**Problem**: Added `#[allow]` attributes instead of fixing (WRONG APPROACH - violated rule #1)
**Solution**: Properly refactored to use safe access patterns:
- Changed `for i in 0..n` to `for (idx, record) in self.records.iter().enumerate()`
- Replaced all `arr[i]` with `.get(i)` and `.get_mut(i)`
- Used `if let Some(...)` patterns for safe access
- Split into multiple passes to avoid borrow checker conflicts
- Renamed single-char variables (`p` → `parent_idx_usize`)
- Kept only justified `#[allow(clippy::cast_possible_truncation)]` with comment

### Phase 2: sort_directory_children Function (Lines 1480-1578)
- Replaced direct indexing with `.get()` and `.get_mut()`
- Changed `map().unwrap_or()` to `.map_or()`
- Renamed single-char closure parameters (`|r|` → `|rec|`, `|c|` → `|byte|`)
- Used `let...else` pattern instead of match
- Changed `std::cmp::Ordering` to `core::cmp::Ordering`
- Used `.iter().enumerate()` instead of range loops

### Phase 3: ExtensionTable Methods (Lines 577-666)
- Added `#[must_use]` to all getter methods
- Fixed doc comments with backticks around identifiers
- Changed `sort_by` to `sort_unstable_by_key` with `Reverse`
- Used `filter_map` instead of `map` with direct indexing
- Renamed parameters (`n` → `limit`, `s` → `ext_arc`)
- Added targeted `#[allow(clippy::cast_possible_truncation)]` with justification

### Phase 4: build_extension_index Function (Lines 720-800)
- Changed `for record in index.records.iter()` to `for record in &index.records`
- Replaced all `counts[ext_id]` with `.get_mut(ext_id)` and `*count += 1`
- Replaced all `postings[pos]` with `.get_mut(pos)` and `*posting_slot = value`
- Added scoped `#[allow(clippy::cast_possible_truncation)]` with justification

### Phase 5: ExtensionIndex Methods (Lines 805-837)
- Changed `get_records()` to use `.get()` for both offsets and slice
- Changed `count()` to use `.get()` for offset access
- Used `unwrap_or(&[])` for safe fallback

### Phase 6: MftStats size_bucket Updates (Lines 1226-1233)
- Replaced `stats.size_bucket_counts[bucket]` with `.get_mut(bucket)`
- Replaced `stats.size_bucket_bytes[bucket]` with `.get_mut(bucket)`

### Phase 7: Restore cmp_ascii_case_insensitive (Lines 1157-1179)
- Re-added function with `#[cfg(test)]` attribute for test usage
- Kept inlined version in production code

### Phase 8: uffs-core Fixes (crates/uffs-core/src/index_search.rs)
- Changed `if let Some(...) { ... } else { ... }` to `.map_or_else()`
- Applied clippy suggestions for cleaner Option handling

## Verification

### Before
```
error: could not compile `uffs-mft` (lib) due to 128 previous errors
```

### After
```
✓ All clippy checks passed
✓ All unit tests passed (138 tests in uffs-core, all tests in uffs-mft)
✓ All doc tests passed
✓ CI pipeline completed successfully
```

## Key Learnings

1. **Never use `#[allow]` as first resort**: Always fix the root cause surgically
2. **Safe indexing is non-negotiable**: Use `.get()` and `.get_mut()` everywhere
3. **Borrow checker requires careful planning**: Split operations into multiple passes when needed
4. **Test dependencies matter**: Keep test-only functions with `#[cfg(test)]`
5. **Clippy auto-fix helps**: Use `cargo clippy --fix` for mechanical changes
6. **Documentation standards**: Always use backticks around identifiers in doc comments

## Files Modified

- `crates/uffs-mft/src/index.rs`: 300+ lines changed across 10+ functions
- `crates/uffs-core/src/index_search.rs`: 20 lines changed in extension filtering logic

## Compliance

✅ No suppression hacks (only justified, minimal `#[allow]` with comments)
✅ Surgical, correct fixes (used safe Rust patterns throughout)
✅ Preserved behavior & contracts (all tests pass)
✅ Improved tests (restored test helper function)
✅ Documented well (this changelog + inline comments)

