# CHANGELOG_HEALING - 2026-02-05 14:00

## Summary

Implementing tree metrics parity fixes from `UFFS_TREE_METRICS_PARITY_DEEP_DIVE.md` to achieve 100% parity between Rust LIVE and C++ reference output.

## Changes Made

### Fix #1 & Fix #2 - Tree metrics for root and reparse directories
**File: `crates/uffs-cli/src/commands.rs`**

The LIVE path (`results_to_dataframe`) now uses the record's `tree_metrics()` method for all records (when available), ensuring:
- Root row (FRS=5) gets correct metrics (Fix #1)
- Reparse directories (junctions/symlinks) get `Desc=1` not `Desc=0` (Fix #2)

### Fix #3 - Single source of truth for tree metrics
**File: `crates/uffs-mft/src/index.rs`**

Added a new `tree_metrics()` method to `FileRecord` that returns `(descendants, treesize, tree_allocated)`. Both OFFLINE (`MftIndex::to_dataframe`) and LIVE (`results_to_dataframe`) paths now use this method as the single source of truth.

### Release-mode diagnostics
**File: `crates/uffs-mft/src/cpp_tree.rs`**

Removed the `#[cfg(debug_assertions)]` gate from the diagnostic that warns about directories with `descendants==0` after tree metrics computation. This now runs in release mode to help diagnose LIVE scan issues.

## CI Pipeline Runs

### Run 1 - Initial
- **Status**: FAILED
- **Command**: `rust-script scripts/ci-pipeline.rs go -v`
- **Error**: Clippy lint error - doc comment missing backticks around `tree_allocated`
  ```
  error: item in documentation is missing backticks
     --> crates/uffs-mft/src/index.rs:1510:64
      |
  1510 |     /// Returns the tree metrics tuple (descendants, treesize, tree_allocated).
      |                                                                ^^^^^^^^^^^^^^
  ```
- **Fix**: Added backticks around `tree_allocated` in doc comment

### Run 2 - After lint fix
- **Status**: Starting...
- **Command**: `rust-script scripts/ci-pipeline.rs go -v`

## Files Modified

1. `crates/uffs-cli/src/commands.rs` - Updated `results_to_dataframe()` to use `tree_metrics()` method
2. `crates/uffs-mft/src/index.rs` - Added `tree_metrics()` method to `FileRecord`, updated `to_dataframe()` to use it
3. `crates/uffs-mft/src/cpp_tree.rs` - Removed `#[cfg(debug_assertions)]` gate from diagnostic logging

