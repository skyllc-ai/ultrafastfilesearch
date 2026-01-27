# CHANGELOG_HEALING - C++ vs Rust UFFS Parity Fixes

**Date:** 2026-01-27 18:00  
**Session:** C++ vs Rust Output Parity  
**Version:** v0.2.123 → v0.2.124

## Summary

Fixing identified differences between C++ and Rust UFFS outputs to achieve feature parity.

## Issues Identified

| # | Issue | Severity | Root Cause |
|---|-------|----------|------------|
| 1 | Size on Disk = Size (wrong) | 🔴 HIGH | Output uses `size` instead of `allocated_size` |
| 2 | Directory Size not aggregated | 🟡 MEDIUM | treesize not applied in all output paths |
| 3 | ADS Name missing stream name | 🟡 MEDIUM | Stream name not appended to Name column |
| 4 | Descendant count off by 28 | 🟡 MEDIUM | ADS/hardlink counting differences |
| 5 | "Descendents" typo | 🟢 LOW | Fixed in v0.2.123 |

---

## Fix 1: Size on Disk Calculation ✅ FIXED

### What Failed
- Rust output shows `Size on Disk = Size` for 100% of entries
- C++ correctly shows `Size on Disk = 0` for resident files (data stored in MFT)
- C++ correctly shows `Size on Disk = allocated_size` (cluster-aligned) for non-resident files

### Why It Failed
- The raw MFT parsing is CORRECT (`allocated_size = 0` for resident files)
- `SearchResult` struct lacked `allocated_size` field
- `results_to_dataframe()` used `result.size` for `allocated_sizes` vector

### How Fixed
1. Added `allocated_size: u64` field to `SearchResult` struct in `crates/uffs-core/src/index_search.rs`
2. Updated `SearchResult::from_record()` to populate from `record.first_stream.size.allocated`
3. Updated `SearchResult::from_expanded()` to populate from `stream_info.size.allocated`
4. Fixed `results_to_dataframe()` in `crates/uffs-cli/src/commands.rs` to use `result.allocated_size`

---

## Fix 2: Directory Size Aggregation ✅ FIXED

### What Failed
- C++ shows directory Size = sum of all descendant sizes (treesize)
- Rust shows directory Size = directory's own size (not aggregated)

### Why It Failed
- Code exists in `commands.rs` to apply treesize transformation
- But it was only applied in `results_to_dataframe()`, not in streaming paths
- Streaming paths (`search_multi_drive_filtered`, `search_multi_drive_streaming`) bypassed the transformation

### How Fixed
1. Created `apply_directory_treesize()` helper function in `crates/uffs-core/src/tree.rs`
2. Exported function from `crates/uffs-core/src/lib.rs`
3. Refactored `results_to_dataframe()` to use the helper function
4. Applied transformation in `search_multi_drive_filtered()` streaming path
5. Applied transformation in `search_multi_drive_streaming()` streaming path

---

## Fix 3: ADS Name Column ✅ FIXED

### What Failed
- C++ Name column: `readme.txt:Zone.Identifier` (includes stream name)
- Rust Name column: `readme.txt` (base filename only)

### Why It Failed
- `SearchResult::from_expanded()` only stored base filename in `name` field
- Stream name was stored separately in `stream_name` field but not combined

### How Fixed
1. Updated `SearchResult::from_expanded()` in `crates/uffs-core/src/index_search.rs`
2. For ADS entries (non-empty stream_name), format name as `{base_name}:{stream_name}`
3. This matches C++ behavior where ADS entries show full name with stream suffix

---

## Fix 4: Descendant Count Difference ⏳ DEFERRED

### What Failed
- C++ G:\: 15,115 descendants
- Rust G:\: 15,087 descendants (28 fewer)

### Why It Failed
- Likely due to ADS handling differences
- C++ may count ADS as separate entries in descendant count
- Rust tree metrics are computed per-FRS, not per-stream

### Status
- Deferred for now - requires Windows testing to verify
- The difference is small (0.18%) and may be intentional design difference
- ADS are not separate file records, so not counting them in descendants may be correct

---

## Commits

| Commit | Description |
|--------|-------------|
| v0.2.124 | fix: C++ parity - Size on Disk, Directory Size, ADS Name |

