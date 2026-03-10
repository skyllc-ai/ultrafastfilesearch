# LIVE Tree Metrics Parity – Evidence Collection Report

**Date:** 2026-02-04  
**Version:** v0.2.181  
**Status:** 3 of 4 items verified ✅, 1 item requires attention ⚠️

This document presents evidence collected against the checklist in
`LIVE_TREE_METRICS_PARITY_FINAL_CHECKLIST.md` without modifying any code.

---

## Checklist Item 1: Tree Dispatch (No "org" Shim)

**Requirement:** `MftIndex::compute_tree_metrics_cpp_port()` must dispatch to the
*current* `cpp_tree` module, not `cpp_tree_org`.

### Evidence

| Source | Location | Evidence |
|--------|----------|----------|
| `index.rs` | Lines 2346-2347 | `fn compute_tree_metrics_cpp_port(&mut self, debug: bool) { crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug); }` |
| `lib.rs` | Line 90 | Only module declaration: `pub mod cpp_tree;` |
| grep search | Entire `crates/uffs-mft/src/` | "No cpp_tree_org references found" |

### Verdict: ✅ CORRECT

The dispatch correctly calls `crate::cpp_tree::compute_tree_metrics_cpp_port`.
No `cpp_tree_org` module exists or is referenced anywhere in the codebase.

---

## Checklist Item 2: Directory Descendants Use Printed Channel (Channel B)

**Requirement:** Junction directories (no children) must have `descendants = 1`.

**Required formula:**
```rust
rec.descendants = children_agg.treesize + 1;
```

**NOT acceptable:**
- `rec.descendants = result.treesize` (Channel A)
- `rec.descendants = children_agg.treesize` (missing +1 → leaf dirs become 0)

### Evidence

**From `cpp_tree.rs` lines 199-202:**
```rust
if is_directory {
    // Printed descendants:
    //   - Use children Channel-A stream-count + 1 (directory itself).
    rec.descendants = children.treesize.saturating_add(1);
```

### Verdict: ✅ CORRECT

Uses `children.treesize.saturating_add(1)` which is the correct printed channel
formula. Junction directories with no children will correctly get
`descendants = 0 + 1 = 1`.

---

## Checklist Item 3: Orphan Sweep After ROOT Traversal

**Requirement:**
1. Track `visited: Vec<bool>` for record indices
2. Set `visited[idx] = true` in `preprocess()`
3. After ROOT traversal, scan all records and call `preprocess()` for unvisited

### Evidence

| Location | Code | Purpose |
|----------|------|---------|
| Line 55 | `seen: Vec<bool>,` | Dedicated tracking array in `CppTreeTraversal` struct |
| Line 92 | `self.seen[record_idx] = true;` | Marks record as visited at start of `preprocess()` |
| Lines 67-69 | `let _: Agg = self.preprocess(root_idx, 0, 1);` | Primary ROOT traversal |
| Lines 79-83 | `for idx in 0..self.index.records.len() { if !self.seen[idx] { let _: Agg = self.preprocess(idx, 0, 1); } }` | Orphan sweep |

**Full orphan sweep code from `cpp_tree.rs` lines 76-83:**
```rust
// Orphan sweep: ensure every record has its printed tree metrics
// initialized. This prevents LIVE scans from leaving some
// directories with Size/Desc = 0 due to transient linkage gaps.
for idx in 0..self.index.records.len() {
    if !self.seen[idx] {
        let _: Agg = self.preprocess(idx, 0, 1);
    }
}
```

### Verdict: ✅ CORRECT

Uses dedicated `seen: Vec<bool>` array (not `descendants == 0` check), marks
visited in `preprocess()`, and sweeps all unvisited records after ROOT traversal.

---

## Checklist Item 4: Delta Function Must Match C++ Exactly

**Requirement:** The delta function must use the exact C++ floor-division formula.

**Required C++ formula (from checklist lines 99-104):**
```rust
const fn delta(value: u64, i: u32, n: u32) -> u64 {
    if n <= 1 { return value; }
    let n64 = n as u64;
    let i64 = i as u64;
    value * (i64 + 1) / n64 - value * i64 / n64
}
```

### Evidence

**Current code in `cpp_tree.rs` lines 27-35:**
```rust
const fn delta(value: u64, name_info: u32, total_names: u32) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let total = total_names as u64;
    let base = value / total;
    let rem = value % total;
    base + if (name_info as u64) < rem { 1 } else { 0 }
}
```

**Checklist warning (lines 107-113):**
> The common shortcut: `base + if i < rem { 1 } else { 0 }` is **NOT equivalent**
> to the C++ formula (e.g. with `n=2`, the extra byte goes to the *second* link
> in C++).

### Numerical Example: Why the Formulas Differ

For `n=2` and `value=5`:

| Link Index (i) | C++ Formula | Shortcut Formula |
|----------------|-------------|------------------|
| i=0 | `floor(5×1/2) - floor(5×0/2)` = `2 - 0` = **2** | `2 + (0 < 1 ? 1 : 0)` = **3** |
| i=1 | `floor(5×2/2) - floor(5×1/2)` = `5 - 2` = **3** | `2 + (1 < 1 ? 1 : 0)` = **2** |

**Key difference:** The extra byte goes to the **second** link (i=1) in C++,
but to the **first** link (i=0) in the shortcut formula.

### Verdict: ⚠️ MISMATCH

Current implementation uses the shortcut formula that the checklist explicitly
states is NOT equivalent to the C++ formula. This could cause 1-4 byte
discrepancies in directory sizes for files with multiple hardlinks.

---

## Summary

| # | Checklist Item | Status | Notes |
|---|----------------|--------|-------|
| 1 | Tree dispatch (no "org" shim) | ✅ CORRECT | Dispatches to `crate::cpp_tree` |
| 2 | Descendants use printed channel | ✅ CORRECT | Uses `children.treesize + 1` |
| 3 | Orphan sweep after ROOT | ✅ CORRECT | Uses `seen: Vec<bool>`, sweeps unvisited |
| 4 | Delta function matches the legacy baseline | ⚠️ **MISMATCH** | Uses shortcut, not C++ floor-division |

---

## Recommended Fix for Item 4

Replace the current delta function with the exact C++ formula:

```rust
/// Computes the delta share for a hardlink using the C++ floor-division formula.
///
/// Distributes `value` across `total_names` hardlinks, returning the share for
/// hardlink index `name_info`. This matches the legacy implementation exactly.
#[inline]
const fn delta(value: u64, name_info: u32, total_names: u32) -> u64 {
    if total_names <= 1 {
        return value;
    }
    let n64 = total_names as u64;
    let i64 = name_info as u64;
    value * (i64 + 1) / n64 - value * i64 / n64
}
```

---

## References

- `crates/uffs-mft/src/cpp_tree.rs` - Tree metrics implementation
- `crates/uffs-mft/src/index.rs` - Dispatch function (lines 2346-2348)
- `crates/uffs-mft/src/lib.rs` - Module declarations (line 90)
- `docs/architecture/Investigation/LIVE_TREE_METRICS_PARITY_FINAL_CHECKLIST.md` - Source checklist

