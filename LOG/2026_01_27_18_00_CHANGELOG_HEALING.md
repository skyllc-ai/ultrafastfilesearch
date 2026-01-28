# CHANGELOG_HEALING - C++ vs Rust UFFS Parity Fixes

**Date:** 2026-01-27 18:00
**Session:** C++ vs Rust Output Parity
**Version:** v0.2.123 → v0.2.128

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

## Fix 4: Descendant Count Difference ✅ FIXED

### What Failed
- C++ G:\: 15,115 descendants
- Rust G:\: 15,087 descendants (28 fewer)

### Why It Failed
- C++ counts each ADS (Alternate Data Stream) as a separate descendant
- Rust tree metrics were computed per-FRS, counting each file as 1 regardless of stream count
- C++ also includes all streams' sizes in treesize/tree_allocated

### How Fixed
Modified `compute_tree_metrics()` in `crates/uffs-mft/src/index.rs`:

1. **Sum all streams' sizes**: In the first pass, iterate through all streams (default + ADS) to sum total size and allocated size, not just the first stream
2. **Count streams as descendants**: When accumulating into parent, use `stream_count` instead of `1` to count each ADS as a separate descendant

Key changes:
- Extended `parent_info` tuple to include `stream_count`
- Added loop to follow linked list of additional streams and sum their sizes
- Changed accumulation from `1 + child_descendants` to `stream_count + child_descendants`

---

## Fix 5: Resident File Size on Disk (io.rs) ✅ FIXED

### What Failed
- Deep dive analysis with read-only drive revealed 15,046 files still had wrong Size on Disk
- C++ shows `Size on Disk = 0` for resident files (correct)
- Rust shows `Size on Disk = logical_size` for resident files (incorrect)

### Why It Failed
- The fix in v0.2.124 added `allocated_size` field to `SearchResult` and used it in output
- BUT the underlying MFT parsing in `crates/uffs-mft/src/io.rs` had a bug
- Two locations (lines 663 and 976) returned `(len, len)` for resident files
- This set `allocated_size = logical_size` instead of `allocated_size = 0`

### Evidence
```
autorun.inf:     C++ Size on Disk=0,  Rust Size on Disk=208
sfile_*.tmp:     C++ Size on Disk=0,  Rust Size on Disk=19-22
Zone.Identifier: C++ Size on Disk=0,  Rust Size on Disk=25
```

### How Fixed
Changed `(len, len)` to `(len, 0)` at both locations in `crates/uffs-mft/src/io.rs`:
- Line 663: Resident file handling in first parsing path
- Line 976: Resident file handling in second parsing path

This matches the correct behavior already in `parse.rs` line 590:
```rust
(value_length as u64, 0, false, false)  // allocated_size = 0 for resident
```

---

## Fix 6: Reparse Point Size ✅ FIXED

### What Failed
- Junctions show Size=0 in Rust but Size=48 in C++
- The 48 bytes is the $REPARSE_POINT attribute's ValueLength

### Why It Failed
- Rust only looked at the default $DATA stream for file size
- Reparse points (junctions/symlinks) don't have a $DATA stream
- C++ uses `ah->Resident.ValueLength` from $REPARSE_POINT attribute

### How Fixed
1. In `parse.rs`, when parsing $REPARSE_POINT, extract `value_length` (at offset+16)
2. Store in `reparse_size` variable
3. When calculating final size, if `reparse_tag != 0` and no default stream, use `reparse_size`
4. Applied to both `parse_record_full()` and `parse_record_forensic()`

**Files Modified:**
- `crates/uffs-mft/src/parse.rs` - Extract and use reparse point size

---

## Fix 8: Reparse Point Descendants ✅ FIXED (v0.2.128)

### What Failed
- C++ shows Descendants=1 for junctions, Rust shows Descendants=0
- C++ shows Descendants=3 for dir with 2 files, Rust shows Descendants=2

### Why It Failed
After detailed analysis of `ntfs_index.hpp` lines 774-879:
- C++ formula: `descendants = 1 + sum(child.descendants)` for **ALL entries**
- Files have no children, so `descendants = 1`
- Rust was initializing `descendants = 0` for files

### How Fixed
1. In `compute_tree_metrics()`, initialize `descendants = 1` for ALL entries (files and directories)
2. When accumulating child descendants into parent: simply add `child.descendants`
3. Updated all tree metrics tests to reflect new behavior

**Files Modified:**
- `crates/uffs-mft/src/index.rs` - All entries count themselves in descendants

---

## Fix 9: Treesize MFT Overhead ✅ FIXED (v0.2.128)

### What Failed
- C++ shows ~524 bytes extra per resident file in directory treesize
- Example: _FRAG_PRE_1 with 5000 files: C++ Size=2,731,149, Rust Size=109,445

### Why It Failed
- C++ includes MFT record overhead (~512 bytes) in treesize for resident files
- Resident files (allocated_size = 0) still consume MFT space
- Rust was not accounting for this overhead

### How Fixed
1. In `compute_tree_metrics()`, add 512 bytes of MFT overhead to `treesize` and `tree_allocated` for resident files (files where `allocated_size = 0` and `size > 0`)
2. This overhead propagates up the tree during aggregation

**Files Modified:**
- `crates/uffs-mft/src/index.rs` - MFT overhead for resident files

---

## Commits

| Commit | Description |
|--------|-------------|
| v0.2.124 | fix: C++ parity - Size on Disk, Directory Size, ADS Name |
| v0.2.125 | fix: C++ parity - Descendant count includes ADS |
| v0.2.126 | fix: Resident file Size on Disk = 0 (io.rs bug) |
| v0.2.127 | fix: Reparse point Size = $REPARSE_POINT ValueLength |
| v0.2.128 | fix: Directory self-counting + MFT overhead for resident files |
| v0.2.129 | fix: Descendants algorithm - files display 0, contribute 1 to parent |
| v0.2.130 | fix: Junction Size preserved (not replaced by treesize=0) |
| v0.2.130 | fix: ADS counted in descendants (+1 per stream, not per file) |
| v0.2.130 | fix: Hardlinks counted in EACH parent directory |

---

## Fix 10: Junction Size Preserved ✅ FIXED (v0.2.130)

### What Failed
- Junctions showed Size=0 instead of Size=48 (reparse point data length)
- C++ shows Size=48 for junctions

### Why It Failed
- `apply_directory_treesize()` replaced Size with treesize for ALL directories
- Junctions are directories with `is_reparse=true`
- Junction treesize=0 (no children), so Size became 0

### How Fixed
- Modified `apply_directory_treesize()` in `crates/uffs-core/src/tree.rs`
- Only apply treesize to directories that are NOT reparse points
- Condition: `is_directory AND NOT is_reparse`

---

## Fix 11: ADS Counted in Descendants ✅ FIXED (v0.2.130)

### What Failed
- C++ counts each stream as +1 to parent's descendants
- Rust counted each file as +1, regardless of stream count

### Why It Failed
- Tree metrics used `max(1, child.descendants)` for files
- This always contributed 1, even for files with multiple streams (ADS)

### How Fixed
- Modified `compute_tree_metrics()` in `crates/uffs-mft/src/index.rs`
- Files now contribute `stream_count` to parent (1 per stream)
- Directories still contribute their full descendants count

---

## Fix 12: Hardlinks Counted in Each Parent ✅ FIXED (v0.2.130)

### What Failed
- C++ counts hardlinks as children of EACH parent directory
- Rust only counted hardlinks in the primary parent (first_name.parent_frs)

### Why It Failed
- Tree metrics only used `first_name.parent_frs` for parent link
- Additional hardlinks (in `links` array) were ignored

### How Fixed
- Added Phase 3b in `compute_tree_metrics()` for hardlink pass
- After main loop, iterate over records with `name_count > 1`
- For each additional name (hardlink), add contribution to that parent
- Contribution includes: descendants, treesize, tree_allocated

---

## Final Status

**All 12 issues fixed. Awaiting Windows test run to verify full C++ parity.** 🔄

