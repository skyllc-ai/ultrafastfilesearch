# CHANGELOG_HEALING - 2026-01-30 14:00

## Issue: Missing 40 paths in Rust output compared to C++

### Symptoms
- Trial run comparison showed 40 paths present in C++ output but missing in Rust output
- All 40 paths were hard links (same file at multiple paths)
- Match rate was 99.9994% (7,057,989 vs 7,058,029 records)

### Root Cause Analysis

The issue was in the **parallel parsing fragment merge** logic in `merge_fragment_records()`.

When using parallel parsing (enabled by default for NVMe drives):
1. Worker threads each build their own `MftIndexFragment`
2. If a base record is processed by worker A and its extension record (containing additional hard link names) is processed by worker B:
   - Worker A creates the base record with the primary name
   - Worker B creates a placeholder record with the extension names attached
3. During fragment merge, `merge_fragment_records()` would **discard** the placeholder from worker B because the base record from worker A already had a name
4. The additional hard link names from the extension record were lost

### The Bug (lines 5513-5521 in index.rs)

```rust
} else {
    // Record exists - merge (keep the one with more data)
    let existing = &mut self.records[existing_idx as usize];
    // If existing is a placeholder (no name), replace with new
    if !existing.has_name() && record.has_name() {
        *existing = record;
    }
    // Otherwise keep existing (first wins) <-- BUG: discards additional names!
}
```

### Fix

Replaced `merge_fragment_records()` with `merge_fragment_records_with_deferred_merge()` which:
1. Tracks records that need name/stream merging instead of discarding them
2. After all links/streams are merged (with correct offset adjustments), calls `apply_deferred_name_merges()` to chain the additional names/streams from discarded records to the kept records

Key changes:
- `merge_single_fragment()` now calculates link/stream offset adjustments upfront
- `merge_fragment_records_with_deferred_merge()` adjusts link/stream indices in records and returns deferred merges
- `apply_deferred_name_merges()` chains additional names/streams from discarded records to existing records

### Files Modified
- `crates/uffs-mft/src/index.rs`: Fixed fragment merge logic

### Testing
- All 82 existing tests pass
- Awaiting trial run on Windows to verify the 40 missing paths are now found

## CI Pipeline Run 1 - Failed (clippy lints)

**Errors:** 13 clippy lints in `apply_deferred_name_merges()`:
- `clippy::unnecessary_map_or`: Use `is_some_and` instead of `map_or(false, ...)`
- `clippy::min_ident_chars`: Single-char identifiers `l` and `s` in closures
- `clippy::if_not_else`: Prefer `== NO_ENTRY` with swapped branches
- `clippy::if_then_some_else_none`: Use `bool::then` or `bool::then_some`

**Fix:** Refactored `apply_deferred_name_merges` to use idiomatic Rust patterns:
- Changed `if cond { Some(x) } else { None }` to `cond.then(|| x)` or `cond.then_some(x)`
- Changed `map_or(false, |l| ...)` to `is_some_and(|link| ...)`
- Changed single-char identifiers `l` and `s` to `link` and `stream`

## CI Pipeline Run 2 - Failed (more clippy lints)

**Errors:** 3 clippy lints:
- `clippy::doc_markdown`: Doc comment needs backticks around identifiers
- `clippy::std_instead_of_core`: Use `core::mem::replace` instead of `std::mem::replace`

**Fix:**
- Added backticks around `Vec<(existing_record_idx, discarded_record)>` in doc comment
- Changed `std::mem::replace` to `core::mem::replace`

