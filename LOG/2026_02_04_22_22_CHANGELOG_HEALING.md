# CHANGELOG_HEALING - 2026-02-04 22:22

## CI Pipeline Run

**Timestamp:** 2026-02-04 22:22
**Command:** `rust-script scripts/ci-pipeline.rs go -v`

---

## Errors Found

### Error 1: Type mismatch in `index.rs:5782`
**File:** `crates/uffs-mft/src/index.rs`
**Line:** 5782
**Error:** `mismatched types - expected u32, found Option<_>`
**Code:** `next_entry: None,`
**Root Cause:** Test code using `Option<_>` for `next_entry` field which is now `u32` (using `NO_ENTRY` sentinel)
**Fix:** Change `None` to `NO_ENTRY`

### Error 2: Type mismatch in `index.rs:5785`
**File:** `crates/uffs-mft/src/index.rs`
**Line:** 5785
**Error:** `mismatched types - expected u32, found Option<u32>`
**Code:** `rec.first_name.next_entry = Some((index.links.len() - 1) as u32);`
**Root Cause:** Test code wrapping value in `Some()` when field is now `u32`
**Fix:** Remove `Some()` wrapper

### Error 3: Unknown field `child_idx` in `index.rs:5807`
**File:** `crates/uffs-mft/src/index.rs`
**Line:** 5807
**Error:** `no field child_idx on type &index::ChildInfo`
**Code:** `assert_eq!(child1.child_idx, file_idx as u32);`
**Root Cause:** Field was renamed from `child_idx` to `child_frs`
**Fix:** Change `child_idx` to `child_frs`

### Error 4: Unknown field `child_idx` in `index.rs:5817`
**File:** `crates/uffs-mft/src/index.rs`
**Line:** 5817
**Error:** `no field child_idx on type &index::ChildInfo`
**Code:** `assert_eq!(child2.child_idx, file_idx as u32);`
**Root Cause:** Field was renamed from `child_idx` to `child_frs`
**Fix:** Change `child_idx` to `child_frs`

---

## Fixes Applied

| # | File | Line | Fix Description |
|---|------|------|-----------------|
| 1 | `index.rs` | 5782 | Changed `next_entry: None` to `next_entry: NO_ENTRY` |
| 2 | `index.rs` | 5785 | Changed `Some((index.links.len() - 1) as u32)` to `(index.links.len() - 1) as u32` |
| 3 | `index.rs` | 5807 | Changed `child1.child_idx` to `child1.child_frs` (also fixed type: `u32` → `u64`) |
| 4 | `index.rs` | 5817 | Changed `child2.child_idx` to `child2.child_frs` (also fixed type: `u32` → `u64`) |
| 5 | `index.rs` | 5750-5790 | Restructured test to avoid borrow conflicts - moved `add_name()` calls before `get_or_create()` |
| 6 | `index.rs` | 5809, 5819 | Fixed assertion: `child_frs` stores FRS not index, changed `file_idx as u64` to `file_frs` |

### Error 5: Borrow checker errors (E0499, E0502)
**File:** `crates/uffs-mft/src/index.rs`
**Lines:** 5753-5785
**Error:** `cannot borrow index as mutable more than once at a time`
**Root Cause:** Test code called `index.get_or_create()` (returns `&mut FileRecord`) and then called `index.add_name()` while still holding the mutable reference
**Fix:** Restructured test to call `add_name()` and create `LinkInfo` before calling `get_or_create()`, avoiding overlapping mutable borrows

### Error 6: Wrong assertion comparing FRS to index
**File:** `crates/uffs-mft/src/index.rs`
**Lines:** 5809, 5819
**Error:** `assertion left == right failed: left: 200, right: 2`
**Root Cause:** Test assertions compared `child_frs` (which stores the FRS = 200) to `file_idx as u64` (which is the index = 2). The `child_frs` field stores the FRS, not the index.
**Fix:** Changed `assert_eq!(child1.child_frs, file_idx as u64)` to `assert_eq!(child1.child_frs, file_frs)` (and same for child2)

---

## CI Pipeline Run 2 - Clippy Errors

### Error 7: Clippy `option_if_let_else` in `TreeAlgorithm::from_env()`
**File:** `crates/uffs-mft/src/index.rs` (lines 87-100)
**Error:** `use Option::map_or_else instead of an if let/else`
**Fix:** Refactor to use `map_or` pattern (simpler since we only need the Ok value)

### Error 8: Clippy `min_ident_chars` for single-char variable `s`
**File:** `crates/uffs-mft/src/index.rs` (line 3597)
**Error:** `this ident consists of a single char`
**Fix:** Rename `s` to `root_path`

### Error 9: Clippy `semicolon_outside_block` in test blocks
**File:** `crates/uffs-mft/src/index.rs` (lines 5752, 5761, 5773)
**Error:** `consider moving the ; outside the block for consistent formatting`
**Fix:** Move semicolons outside the blocks

---

## Fixes Applied (Round 2)

| # | File | Line | Fix Description |
|---|------|------|-----------------|
| 7 | `index.rs` | 87-93 | Refactored `TreeAlgorithm::from_env()` to use `map_or` pattern |
| 8 | `index.rs` | 3597 | Renamed `s` to `root_path` |
| 9 | `index.rs` | 5742-5780 | Removed blocks entirely - avoids conflicting `semicolon_inside_block` vs `semicolon_outside_block` lints |

---

## Verification

- [x] All fixes applied (Round 1)
- [x] All fixes applied (Round 2)
- [x] CI pipeline re-run
- [x] All tests pass
- [x] Committed and pushed as v0.2.192

**Final CI Pipeline Result:** ✅ SUCCESS (354s total)
