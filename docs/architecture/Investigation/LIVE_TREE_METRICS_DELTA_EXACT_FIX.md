# LIVE Tree Metrics Parity – Delta Function Exact-Match Fix

**Date:** 2026-02-04  
**Goal:** Eliminate the remaining tree-metrics parity risk caused by a non-equivalent hardlink delta formula.

## What this fixes

Your checklist correctly calls out that the commonly-used shortcut:

```rust
base + if i < rem { 1 } else { 0 }
```

is **not equivalent** to the legacy baseline implementation:

```rust
value * (i + 1) / n - value * i / n
```

When `value % n != 0`, the shortcut assigns the “extra” units to the **first** links, while the C++ floor-division formula can assign them to **later** links (e.g. `n=2` sends the extra unit to `i=1`).

In practice, this manifests as small (often 1–4 byte) discrepancies in directory tree sizes when:
- a file has multiple hardlinks (multiple `FILE_NAME` attributes that are treated as distinct links), and
- directory totals depend on the per-link distributed contribution.

## Drop-in replacement provided

A drop-in replacement for `crates/uffs-mft/src/cpp_tree.rs` is provided:

- Uses the **exact** C++ delta formula
- Keeps the **orphan sweep** (LIVE robustness)
- Keeps **per-stream delta distribution** for internal streams and overflow streams
- Uses `tracing::warn!` instead of `eprintln!` and addresses common clippy issues (`let_underscore_untyped`, short idents, etc.)

**File:** `cpp_tree_delta_exact_fixed.rs`  
Replace your repo’s `crates/uffs-mft/src/cpp_tree.rs` with this file.

## Minimal required companion check

Make sure `crates/uffs-mft/src/index.rs` dispatches to the *current* module:

```rust
fn compute_tree_metrics_cpp_port(&mut self, debug: bool) {
    crate::cpp_tree::compute_tree_metrics_cpp_port(self, debug);
}
```

If you still have any `cpp_tree_org` shim, the new `cpp_tree.rs` will not be used.

## How to verify

1. Rebuild and run the CI pipeline:
   - `rust-script scripts/ci-pipeline.rs go -v`
2. Re-run the parity trial where you previously saw 1–4 byte discrepancies (the “hardlink stress” dataset).
3. Confirm the parity report shows **0 mismatches** in tree-metric columns:
   - `Size` (directory tree size)
   - `Descendants`

If LIVE previously showed ROOT/junction `Descendants=0`, this should also be resolved as long as the executable you’re testing is built from the updated sources and the dispatch points at `crate::cpp_tree`.

## Notes

- The delta function is implemented as `const fn` and matches the C++ formula directly.
- This change is intentionally narrow: it does not change the traversal order, orphan sweep behavior, or printed/propagation channel split.
