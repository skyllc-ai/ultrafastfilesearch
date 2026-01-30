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

## CI Pipeline Run 3 - v0.2.154 deployed but fix didn't work

Trial run on Windows still showed 40 missing paths. The deferred merge fix addressed
cross-fragment merging but NOT same-fragment ordering issues.

### Deeper Root Cause Analysis

The REAL bug is in `parse_record_to_fragment()` (io.rs lines 1666-1689):

When an extension record is processed BEFORE the base record in the SAME fragment:
1. `parse_extension_to_fragment()` creates a placeholder for `base_frs`
2. Extension names/streams are added to this placeholder
3. Later, `parse_record_to_fragment()` processes the base record
4. It calls `fragment.get_or_create(frs)` which returns the EXISTING placeholder
5. **BUG**: It then OVERWRITES `first_name` and `first_stream`, losing extension data:

```rust
record.first_name = LinkInfo {
    next_entry: NO_ENTRY,  // <-- OVERWRITES extension links!
    name: name_ref,
    parent_frs,
};
record.name_count = 1 + additional_count as u16;  // <-- RESETS count!
```

### Fix (v0.2.155)

Modified `parse_record_to_fragment()` to preserve extension data:
1. Save existing `first_name` and `first_stream` BEFORE overwriting
2. After setting base record data, chain extension names/streams to base record's chain
3. Add extension counts to base record counts instead of resetting

Key changes in io.rs:
- Save `existing_first_name` (full LinkInfo) before overwriting
- Save `existing_name_count` and `existing_stream_count`
- Chain: base first_name → base additional links → extension first_name → extension overflow
- Chain: base first_stream → base ADS → extension ADS
- Calculate total counts: base counts + extension counts

### Verification (macOS offline MFT test)

Tested locally on macOS using the offline MFT file (`D_mft.bin`):

```
# Before fix (v0.2.154):
C++ paths:  7,058,029
Rust paths: 7,057,989
Missing:    40 paths

# After fix:
C++ paths:  7,058,029
Rust paths: 7,058,029
Missing:    0 paths ✅
```

**100% path match achieved!**

The fix correctly preserves extension record data (hard links and ADS) when the base record
is processed after the extension record in the same parallel parsing fragment.

