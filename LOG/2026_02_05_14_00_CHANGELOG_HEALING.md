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
- **Status**: ✅ PASSED
- **Command**: `rust-script scripts/ci-pipeline.rs go -v`
- **Version**: 0.2.195
- **Total pipeline time**: 378s
- **Commit**: `d31889995` - `chore: development v0.2.195 - comprehensive testing complete [auto-commit]`

## Files Modified

1. `crates/uffs-cli/src/commands.rs` - Updated `results_to_dataframe()` to use `tree_metrics()` method
2. `crates/uffs-mft/src/index.rs` - Added `tree_metrics()` method to `FileRecord`, updated `to_dataframe()` to use it
3. `crates/uffs-mft/src/cpp_tree.rs` - Removed `#[cfg(debug_assertions)]` gate from diagnostic logging

---

## Fix #1 (from Proposed Fixes doc) - Make IOCP parsing order deterministic

**Date:** 2026-02-06

### Problem

Live scan mismatches showed large size differences (e.g., `48→8`, `65712→184`, `196800→131272`) with same descendant counts. This was caused by IOCP completions arriving out-of-order, which affected the stateful "last stream wins" logic in `parse_stream`.

When IO completions arrive out-of-order:
- Extension record data could be applied before base record data
- Later record ranges could be applied before earlier ones
- "Secondary stream" parsing could override "primary stream" selection

### Solution

**File: `crates/uffs-mft/src/cpp_io_pipeline.rs`**

Implemented a "sequence numbers + bounded reorder buffer" strategy:

1. **Assign sequence numbers**: Each `IoOp` now has a `seq` field assigned in push order (reflecting increasing `virtual_offset`)

2. **Track processing state**:
   - `next_issue`: Next sequence number to submit for I/O
   - `next_process`: Next sequence number to parse

3. **Reorder buffer**: When an IO completes, store the buffer in `completed_buffers[seq]` instead of processing immediately

4. **In-order processing**: Process buffers strictly in sequence order:
   ```rust
   while next_process < total_ops {
       let Some((buf, bytes_xfer)) = completed_buffers[next_process].take() else {
           break; // Next buffer not ready yet
       };
       pipeline.process_chunk(buffer_slice, io_op.virtual_offset);
       next_process += 1;
   }
   ```

5. **Bounded memory**: Only issue reads up to `next_process + concurrency` to limit memory usage

6. **Diagnostic counters**: Added `out_of_order_count` and `max_reorder_depth` to track how often reordering occurs

### Key Changes

- Changed `io_ops` from `VecDeque<IoOp>` to `Vec<IoOp>` for random access by sequence number
- Added `seq: usize` field to `IoOp` struct
- Changed `InFlightOp` to store `seq: usize` instead of `op: IoOp`
- Added `completed_buffers: Vec<Option<(AlignedBuffer, usize)>>` reorder buffer
- Modified completion handling to store buffers and process in order
- Added bounded lookahead to prevent unbounded memory growth

### Expected Outcome

This fix should eliminate the "48→8 / 65712→184" style mismatches by ensuring `pipeline.process_chunk(...)` is called in ascending logical MFT virtual offset order, making live scan behave like offline scan while still keeping overlapped reads for throughput.

---

## Fix #2 (from Proposed Fixes doc) - Stabilize primary stream selection

**Date:** 2026-02-06

### Problem

Even with deterministic IOCP ordering (Fix #1), the core stream update logic could still produce different results due to:
- Attribute ordering differences across Windows builds
- Records where a non-default stream appears last

The "last stream wins" behavior in `parse_stream` could cause:
- Directories to have a non-index stream as `first_stream` (wrong size)
- Files to have an ADS as `first_stream` instead of unnamed `$DATA`

### Solution

**File: `crates/uffs-mft/src/cpp_types.rs`**

1. **Added `is_default_stream` classifier** (const fn):
   - For directories: returns true if `type_name_id == 0 && name_len == 0` (directory index stream)
   - For files: returns true if `type_name_id == 8` (0x80 >> 4, which is `$DATA`) and `name_len == 0` (unnamed)

2. **Modified `parse_stream` eviction logic**:
   - When `!new_is_default && first_is_default`: Keep default stream as primary, push new stream to overflow instead
   - For all other cases: Use existing "last wins" behavior (push first to overflow, new becomes first)

3. **Added `update_overflow_stream_sizes` helper**:
   - Similar to `update_stream_sizes` but updates a stream in the overflow list (by index) instead of `first_stream`
   - Used when a non-default stream is pushed to overflow to preserve default stream stability

### Expected Outcome

This fix ensures:
- Directories always have their directory-index stream as `first_stream`
- Files always have unnamed `$DATA` as `first_stream` (not an ADS)
- Prevents "wrong stream becomes primary" from collapsing directory sizes to small resident fragments

---

## Fix #3 (from Proposed Fixes doc) - Remove offline ±1..±8 drift by enforcing hardlink invariants

**Date:** 2026-02-06

### Problem

Offline mismatches showed tiny ±1 to ±8 byte differences with identical descendant counts. This is consistent with "hardlink remainder drift" - when Rust and C++ disagree about:
- Which hardlink gets which `i` (name_index)
- What `N` (total names) is

A few remainder bytes land in a different parent directory, producing small differences at directory boundaries.

### Solution

**File: `crates/uffs-mft/src/index.rs`**

1. **Added environment variable gate for forced rebuild**:
   - Set `UFFS_REBUILD_CHILDREN_ALWAYS=1` to force `rebuild_children_from_names()` before tree metrics computation
   - This removes parse-order artifacts and stabilizes name_index mapping
   - Production fast-path is preserved (only runs when env var is set)

**File: `crates/uffs-mft/src/cpp_tree.rs`**

2. **Added `compute_name_info_checked` function**:
   - Debug-aware version of `compute_name_info` that logs when clamping occurs
   - Logs warning when `name_index >= total_names` (parity risk indicator)
   - Helps identify if offline drift is coming from invariant violations

### Expected Outcome

- With `UFFS_REBUILD_CHILDREN_ALWAYS=1`, parity validation runs will have deterministic child lists
- Logging will expose any out-of-range name_index issues that could cause remainder drift
- Production performance is unaffected (env var is opt-in)

---

## Files Modified (Fix #2 and Fix #3)

1. `crates/uffs-mft/src/cpp_types.rs`:
   - Added `is_default_stream()` const fn
   - Modified `parse_stream()` with default stream stability logic
   - Added `update_overflow_stream_sizes()` helper

2. `crates/uffs-mft/src/index.rs`:
   - Added `UFFS_REBUILD_CHILDREN_ALWAYS` env var check in `compute_tree_metrics_cpp_port()`

3. `crates/uffs-mft/src/cpp_tree.rs`:
   - Added `compute_name_info_checked()` with debug logging
   - Updated traversal to use checked version

---

## Fix #4 (from Proposed Fixes doc) - Prevent "double-parse adds sizes twice"

**Date:** 2026-02-06

### Problem

The stream merge logic adds sizes when a stream repeats. If the same record or extension record is parsed twice (due to IO overlap, retry, or ordering bug), the size can be added again while stream_count remains stable. This can inflate directory totals (including root) without obvious stream-count/descendant changes.

The f_disk parity report showed live row-count drift (+19) while path set still matched, which is consistent with "a small number of duplicates".

### Solution

**File: `crates/uffs-mft/src/cpp_types.rs`**

1. **Added `parsed_record_seen` field to `CppMftIndex`**:
   - `pub parsed_record_seen: Vec<bool>` - tracks which FRS have been parsed
   - `pub duplicate_parse_skipped: u64` - diagnostic counter for skipped duplicates

2. **Added `mark_record_seen` method to `CppMftIndex`**:
   - Returns `true` if record was already seen (duplicate), `false` if first time
   - Expands the seen table as needed
   - Increments `duplicate_parse_skipped` counter on duplicates

3. **Updated `load` function in `CppParsePipeline`**:
   - Calls `index.mark_record_seen(frs)` before parsing each record
   - Skips parsing if the record was already seen
   - Logs warning in debug mode when skipping duplicates
   - Logs debug message in release mode if any duplicates were skipped in a chunk

**File: `crates/uffs-mft/src/cpp_io_pipeline.rs`**

4. **Added alignment assertions in `CppIoPipeline::run`**:
   - `debug_assert!(io_op.virtual_offset % bytes_per_record == 0)` - virtual offset must be record-aligned
   - `debug_assert!(bytes_xfer % bytes_per_record == 0 || is_last_chunk)` - buffer size must be record-aligned (except last chunk)
   - If any are violated, it means we are slicing the buffer such that record boundaries are broken

### Expected Outcome

This fix prevents:
- Same record being parsed twice due to IO overlap/retry
- Size inflation from duplicate stream merges
- Row-count drift in live scans

Combined with Fix #1 (deterministic IOCP ordering), this should eliminate live-only discrepancies.

## Files Modified (Fix #4)

1. `crates/uffs-mft/src/cpp_types.rs`:
   - Added `parsed_record_seen` and `duplicate_parse_skipped` fields to `CppMftIndex`
   - Added `mark_record_seen()` method
   - Updated `load()` to skip duplicate records

2. `crates/uffs-mft/src/cpp_io_pipeline.rs`:
   - Added alignment assertions before `process_chunk` call

---

## Fix #5 (from Proposed Fixes doc) - Make the tripwire unmissable

**Date:** 2026-02-06

### Problem

The parity reports claimed "cpp_tree tripwire NOT FOUND", but the runtime trace log clearly contained the [TRIP] lines. This was because the analyzer was searching the wrong log file (e.g., `*_mft_save*.log` vs the `rust_live_trace_*.txt`).

### Solution

Put a tripwire string into the output file that the parity harness always reads, rather than depending on trace logs.

**File: `crates/uffs-core/src/output.rs`**

1. **Added `tripwire` field to `OutputConfig`**:
   - `pub tripwire: Option<String>` - optional tripwire string to write at top of output

2. **Added `with_tripwire` method**:
   - Builder method to set the tripwire string

3. **Updated `write` method**:
   - Writes `# TRIPWIRE: <value>` comment line before the header when tripwire is set

**File: `crates/uffs-cli/src/commands.rs`**

4. **Set tripwire in search command**:
   - Tripwire format: `UFFS cpp_tree FIXED v<version> tree_metrics_parity`
   - Written at the top of `rust_live_*.txt` and `rust_offline_*.txt` files

### Expected Outcome

The parity harness will always find the tripwire in the output file itself, avoiding any dependence on trace logs. Example output:

```
# TRIPWIRE: UFFS cpp_tree FIXED v0.2.196 tree_metrics_parity
"Path","Name","Parent",...
```

## Files Modified (Fix #5)

1. `crates/uffs-core/src/output.rs`:
   - Added `tripwire` field to `OutputConfig`
   - Added `with_tripwire()` builder method
   - Updated `write()` to output tripwire comment

2. `crates/uffs-cli/src/commands.rs`:
   - Set tripwire in `OutputConfig` for search command

---

## Fix: CI Compilation Errors (2026-02-06)

### Problem
Windows cross-compilation failed with two errors:
1. `AlignedBuffer` doesn't implement `Clone` - `vec![None; total_ops]` requires Clone
2. `process_chunk` expects `&mut [u8]` but receiving `&[u8]`

### Solution

1. `crates/uffs-mft/src/cpp_io_pipeline.rs` (line 449):
   - Changed `vec![None; total_ops]` to `(0..total_ops).map(|_| None).collect()`
   - This avoids requiring Clone on AlignedBuffer

2. `crates/uffs-mft/src/cpp_io_pipeline.rs` (line 598-599):
   - Changed `&buf.as_slice()[..bytes_xfer]` to `&mut buf.as_mut_slice()[..bytes_xfer]`
   - This provides the mutable reference that `process_chunk` expects
