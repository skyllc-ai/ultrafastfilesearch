# CHANGELOG_HEALING - 2026-02-03 12:15

## Summary

This healing session addresses compile errors and clippy warnings introduced by the internal stream delta fix for tree metrics parity.

## Changes Made

### 1. Missing field `first_internal_stream` in `FileRecord` initializer (cpp_types.rs:1255)

**What failed:** The `FileRecord` struct was extended with a new field `first_internal_stream` for the internal stream linked list, but the initializer in `cpp_types.rs` was not updated.

**Fix:** Added `first_internal_stream: NO_ENTRY` to the `FileRecord` initializer in `convert_to_mft_index()`.

### 2. Missing field `internal_streams` in `MftIndex` initializer (index.rs:7562)

**What failed:** The `MftIndex` struct was extended with a new field `internal_streams: Vec<InternalStreamInfo>`, but the deserialize initializer was not updated.

**Fix:** Added `internal_streams: Vec::new()` to the `MftIndex` initializer in `deserialize()`.

### 3. Clippy: doc_markdown - missing backticks (index.rs:944)

**What failed:** Documentation comment `bit0=is_sparse, bit1=is_resident` was missing backticks.

**Fix:** Changed to `` `bit0=is_sparse`, `bit1=is_resident` ``.

### 4. Clippy: min_ident_chars - single char identifier (index.rs:5973)

**What failed:** Closure parameter `|c|` was flagged as too short.

**Fix:** Changed to `|ch|`.

### 5. Clippy: bool_to_int_with_if (index.rs:5980)

**What failed:** Boolean to int conversion using if/else instead of `u8::from()`.

**Fix:** Changed `(if st.is_sparse { 0x01 } else { 0x00 }) | (if st.is_resident { 0x02 } else { 0x00 })` to `u8::from(st.is_sparse) | (u8::from(st.is_resident) << 1_u8)`.

### 6. Clippy: if_not_else (index.rs:5993)

**What failed:** Unnecessary `!=` operation - prefer positive condition first.

**Fix:** Swapped the if/else branches to check `== NO_ENTRY` first.

### 7. Clippy: too_many_lines (index.rs:6476)

**What failed:** Function `apply_deferred_name_merges` has 113 lines, exceeding the 100 line limit.

**Fix:** Added targeted `#[allow(clippy::too_many_lines)]` to the function. This is justified because the function handles complex merge logic that is clearer as a single unit rather than artificially split.

### 8. Clippy: default_numeric_fallback (index.rs:5980)

**What failed:** Numeric literal `1` in shift operation needs explicit type suffix.

**Fix:** Changed `<< 1` to `<< 1_u8`.

## CI Pipeline Run

Starting CI pipeline run after all fixes applied.

