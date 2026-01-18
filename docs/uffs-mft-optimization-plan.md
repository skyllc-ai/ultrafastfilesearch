# UFFS MFT Optimization Plan

_Last updated: 2026-01-18_

This document describes an **experiment-driven roadmap** to make `uffs-mft` the **fastest possible MFT reader and parser**, while preserving correctness and compatibility with the existing C++ behavior.

**Core philosophy**: Every optimization follows the cycle **hypothesis → measurement → accept/reject**. We never "feel" improvements—we prove them with data.

---

## 1. Scope and Goals

**Scope**
- Windows NTFS Master File Table (MFT) reading and parsing, primarily in:
  - `crates/uffs-mft/src/reader.rs`
  - `crates/uffs-mft/src/io.rs`
  - Supporting modules: `platform.rs`, `ntfs.rs`, `raw.rs`

**Goals**
- Minimize **wall-clock time** from start of read to DataFrame ready.
- Keep **memory usage** bounded and predictable on very large volumes.
- Maintain or improve **correctness** and parity with the C++ implementation.
- Provide clear hooks for testing, benchmarking, and future tuning.

**Non-goals (for now)**
- Changing on-disk raw MFT dump format.
- Changing public CLI surface in incompatible ways.

**Workload context**
- Typical target systems have multiple large NTFS volumes, ranging from hundreds of GB to multi-TB.
- Example real-world runs:
  - ~2.7M files / 0.45M dirs on a 1.76 TB SSD (C:) in ~3m20s.
  - ~4.46M files / 0.30M dirs on a 7.28 TB HDD (D:) in ~49m40s.
  - ~7.16M files / 1.12M dirs on a 7.28 TB HDD (S:) in ~1h52m.
- Across all volumes, totals exceed 21M files and 3.2M directories.
- At this scale, even tiny inefficiencies per record (extra atomics, extra passes, extra copies) compound into many minutes of wall-clock time.

---

## 2. Bottleneck Analysis by Drive Type

Before optimizing, we must understand **where time is spent** on each drive type.

### 2.1 Actual Benchmark Data (January 2026)

Benchmark run on MASTER-PC (24 CPU cores, v0.1.30):

| Drive | Type | MFT Size | Records | Total Time | Throughput | Primary Bottleneck |
|-------|------|----------|---------|------------|------------|-------------------|
| **C:** | SSD | 4.5 GB | 3.0M | 11.3s | 402 MB/s | DF Build (40%) |
| **D:** | HDD | 4.8 GB | 4.8M | 46.7s | 103 MB/s | Read (51%) |
| **E:** | HDD | 2.9 GB | 2.9M | 54.7s | 53 MB/s | Read (64%) |
| **F:** | SSD | 4.5 GB | 2.2M | 8.3s | 547 MB/s | DF Build (35%) |
| **G:** | ??? | 44 MB | 45K | 0.3s | 152 MB/s | (trivial) |
| **M:** | HDD | 2.4 GB | 1.9M | 33.2s | 74 MB/s | Read (65%) |
| **S:** | HDD | 11.5 GB | 8.3M | 160.6s | 71 MB/s | Read (59%) |

**Total benchmark time**: 315 seconds across all 7 drives.

### 2.2 Phase Breakdown by Drive Type

**SSD Drives (C:, F:) - CPU Bound**
```
Phase         | C: (ms) | C: (%) | F: (ms) | F: (%)
--------------|---------|--------|---------|--------
Read          |   2,048 |   18%  |   1,616 |   19%
Parse         |   3,413 |   30%  |   2,693 |   32%
Merge         |   1,365 |   12%  |   1,077 |   13%
DF Build      |   4,492 |   40%  |   2,927 |   35%
--------------|---------|--------|---------|--------
Total         |  11,320 |  100%  |   8,316 |  100%
```

**HDD Drives (D:, E:, M:, S:) - I/O Bound**
```
Phase         | D: (ms) | D: (%) | E: (ms) | E: (%) | S: (ms) | S: (%)
--------------|---------|--------|---------|--------|---------|--------
Read          |  23,683 |   51%  |  35,163 |   64%  |  94,040 |   59%
Parse         |   6,766 |   14%  |  10,046 |   18%  |  26,868 |   17%
Merge         |   3,383 |    7%  |   5,023 |    9%  |  13,434 |    8%
DF Build      |  12,854 |   28%  |   4,467 |    8%  |  26,297 |   16%
--------------|---------|--------|---------|--------|---------|--------
Total         |  46,688 |  100%  |  54,702 |  100%  | 160,642 |  100%
```

### 2.3 Key Insights from Benchmark

1. **SSD vs HDD Bottleneck Confirmed**:
   - SSD: DF Build (35-40%) + Parse (30-32%) dominate → CPU optimization priority
   - HDD: Read (51-65%) dominates → I/O optimization priority

2. **~4x Throughput Difference**: SSD (400-550 MB/s) vs HDD (50-100 MB/s)

3. **Anomaly: Drive E is 2x slower than Drive D**:
   - Both HDDs, similar sizes, but E: 53 MB/s vs D: 103 MB/s
   - Possible causes: older drive, different spindle speed, more fragmentation

4. **🚨 Bitmap Not Skipping Records**:
   - All drives show `in_use_records == total_records`
   - This means **zero records are being skipped**
   - Either MFTs are 100% utilized (unlikely) or bitmap logic has a bug
   - **Investigation required** (see Section 6.1)

5. **24 CPU Cores Available**: Massive parallelism potential not fully utilized

### 2.4 Optimization Priority Matrix

Based on actual data:

**For SSDs (C:, F:) - Focus on CPU**
| Priority | Target | Expected Gain |
|----------|--------|---------------|
| **P0** | DF Build optimization (SoA layout, fold/reduce) | 20-30% |
| **P1** | Parse optimization (zero-alloc, SIMD) | 10-15% |
| **P2** | Rayon tuning (utilize 24 cores) | 5-10% |

**For HDDs (D:, E:, M:, S:) - Focus on I/O**
| Priority | Target | Expected Gain |
|----------|--------|---------------|
| **P0** | Prefetch / overlapped I/O | 30-50% |
| **P1** | Larger chunk size for HDD (4MB → 8-16MB?) | 10-20% |
| **P2** | Read-ahead buffer | 10-15% |

**Cross-Drive**
| Priority | Target | Expected Gain |
|----------|--------|---------------|
| **P0** | Fix/investigate bitmap skip logic | Unknown (potentially significant) |
| **P1** | Parallel drive scanning | Linear speedup |

### 2.5 Realistic Target Performance

Based on benchmark data and optimization potential:

| Drive | Current | Target | Speedup |
|-------|---------|--------|---------|
| C: (SSD) | 11.3s | **6-7s** | 1.6x |
| D: (HDD) | 46.7s | **25-30s** | 1.7x |
| S: (HDD, 11.5GB) | 160.6s | **80-100s** | 1.8x |
| **Total (all drives)** | 315s | **~180s** | **1.75x** |

---

## 3. Current Architecture (High Level)

Main live path (simplified):
- `MftReader::read_all` / `read_with_progress` (in `reader.rs`).
- `read_mft_internal`:
  - Opens `VolumeHandle` for `\\.\C:`.
  - Builds `MftExtentMap` and optional `MftBitmap`.
  - Chooses chunk size based on `DriveType`.
  - Instantiates `ParallelMftReader::new_optimized`.
- `ParallelMftReader::read_all_parallel_with_progress` (in `io.rs`):
  - Uses `generate_read_chunks` to plan I/O.
  - Reads all chunks sequentially with aligned buffers.
  - Then parses records in parallel using Rayon and `parse_record_zero_alloc`.
  - Merges extension records using `MftRecordMerger`.
- `MftReader` converts `Vec<ParsedRecord>` into a `uffs_polars::DataFrame`.

Other existing readers:
- `StreamingMftReader`: single reusable aligned buffer, parse-while-reading.
- `PrefetchMftReader`: double-buffering with a background prefetcher.
- `read_raw_internal` + `RawMftData`: raw MFT dump workflows.

---

## 4. Principles and Constraints

- **Experiment-driven**: Every change has a hypothesis, measurement method, and accept/reject criteria.
- **Zero cheating**: No disabling lints, no skipping tests, no ignoring errors.
- **Surgical changes**: Prefer minimal, well-scoped optimizations.
- **Behavior preservation**: Public behavior and schemas must remain compatible.
- **Visibility**: Use `tracing` for key performance metrics (bytes, records, timings).
- **Measure before optimizing**: Phase timings must exist before any "quick win" is attempted.

---

## 5. Correctness and Parity Harness

Before optimizing, we need a **repeatable validation loop** to ensure we don't break correctness.

### 5.1 Golden Datasets

- Maintain a small raw MFT dump (or synthetic test data) as a test asset.
- Store expected normalized output alongside it.
- CI runs against this golden dataset on every PR.

### 5.2 Normalization Rules

Before comparing outputs:
- Sort records by a stable key (e.g., record number).
- Use canonical timestamp format (ISO 8601).
- Strip run-specific fields (e.g., absolute paths that vary by machine).
- Normalize invalid/deleted entry representation.

### 5.3 Diff Tooling

- Produce a human-friendly "first mismatch" report.
- Show context around mismatches (5 records before/after).
- Exit with clear error code on mismatch.

**Success criteria**: Any optimization PR must pass the parity harness with zero diffs.

---

## 6. Milestones Overview

Each milestone has explicit **success criteria** and **measurement methods**.

| Milestone | Focus | Success Criteria | Priority |
|-----------|-------|------------------|----------|
| **M0** | Instrumentation Foundation | Phase timings logged, baseline established | ✅ DONE |
| **M0.5** | Bitmap Investigation | Understand why no records are skipped | ✅ DONE |
| **M1** | Quick Wins (CPU) | ≥10% reduction in parse_time on SSD | ✅ DONE |
| **M1.5** | DF Build Optimization | ≥20% reduction in df_build_time on SSD | ✅ DONE |
| **M2** | Streaming & Prefetch | ≥15% reduction in wall-clock on HDD | ✅ DONE |
| **M3** | Overlapped I/O | ≥10% additional reduction on HDD | **P2** |
| **M4** | Data Layout Overhaul | SoA layout, direct-to-columns | **P1** |
| **M5** | Benchmarks & Auto-Tuning | Reproducible benchmark suite, CI integration | **P2** |

**Benchmark Matrix** (required for validation):
- SSD baseline: Drive C: (3.0M records, 11.3s, 402 MB/s)
- HDD baseline: Drive D: (4.8M records, 46.7s, 103 MB/s)
- Large HDD: Drive S: (8.3M records, 160.6s, 71 MB/s)

---

## 6.1 Milestone 0.5 – Bitmap Investigation (NEW - P0)

**Status**: ✅ COMPLETE (v0.1.37)

**Resolution**: Fixed two critical bugs in bitmap reading:
1. Volume handle was opened with wrong access permissions (share flags instead of access flags)
2. `ReadFile` with `FILE_FLAG_NO_BUFFERING` required sector-aligned reads

After fix, bitmap shows ~67% utilization (was 100%), enabling skip optimization.

### Problem

Benchmark data shows `in_use_records == total_records` for ALL drives:
```
Drive C: in_use=4,656,384, total=4,656,384 (100%)
Drive D: in_use=4,917,248, total=4,917,248 (100%)
Drive S: in_use=11,758,592, total=11,758,592 (100%)
```

This is suspicious—real MFTs typically have 10-30% free records.

### Possible Causes

1. **Bitmap logic bug**: `MftBitmap::count_in_use()` may be counting wrong
2. **Bitmap not being used**: Records may not be filtered by bitmap during read
3. **Bitmap retrieval failing silently**: `get_mft_bitmap().ok()` may return `None`
4. **Bitmap interpretation wrong**: Bit order or byte order may be inverted

### Investigation Tasks

1. [ ] Add logging to `get_mft_bitmap()` to confirm it succeeds
2. [ ] Log `bitmap.count_in_use()` vs `bitmap.len()` to verify counts
3. [ ] Manually inspect a few bitmap bytes to verify interpretation
4. [ ] Check if `generate_read_chunks` actually uses the bitmap to skip ranges
5. [ ] Compare parsed record count vs in_use count

### Expected Impact

If bitmap is working correctly and we can skip 20% of records:
- **Read time**: 20% reduction (fewer bytes to read)
- **Parse time**: 20% reduction (fewer records to parse)
- **Overall**: 15-20% improvement across all drives

---

## 7. Milestone 0 – Instrumentation Foundation

**Status**: ✅ COMPLETE (v0.1.30)

The `bench` and `bench-all` commands now provide phase-level timing.

### Goal

Establish phase-level timing instrumentation so we can measure the impact of every subsequent change.

### Success Criteria

- [ ] All five phases (read, parse, merge, df_build, total) are timed and logged.
- [ ] Baseline measurements recorded for at least one SSD and one HDD volume.
- [ ] Metrics are emitted in a structured format (JSON or structured tracing).

### 7.1 Add Phase Timing Instrumentation

**Tasks**
1. Add timing spans around each phase in `read_mft_internal`:
   - `read_time`: Time in I/O operations (all `ReadFile` calls).
   - `parse_time`: Time in `parse_record_zero_alloc` loop.
   - `merge_time`: Time in `MftRecordMerger`.
   - `df_build_time`: Time building DataFrame columns.
2. Log a summary line at end of run:
   ```
   MFT scan complete: read=1234ms parse=5678ms merge=123ms df_build=456ms total=7491ms records=2700000
   ```
3. Add optional `--json-metrics` flag to emit structured JSON for scripting.

### 7.2 Establish Baselines

**Tasks**
1. Run on representative SSD volume (e.g., C:) and record all phase timings.
2. Run on representative HDD volume (e.g., D: or S:) and record all phase timings.
3. Document baselines in a `BENCHMARKS.md` or similar.

### 7.3 Add Memory Tracking (Optional)

**Tasks**
1. Track peak RSS using platform APIs or `jemalloc` stats.
2. Log peak memory alongside phase timings.

### 7.4 Extension Record Merging Metrics

**Rationale**: The merger can become a hidden O(n) or worse cost if it does lots of hashmap work.

**Tasks**
1. Add separate timing for `MftRecordMerger` operations.
2. Log extension record count and merge time.
3. Track hashmap operations if significant.

---

## 8. Milestone 1 – Quick Wins (CPU Optimization)

**Status**: ✅ COMPLETE (v0.1.37)

### Goal

Reduce CPU overhead in the hot path without changing I/O strategy.

### Baseline (from benchmark)
```
Drive C: (SSD)
  Parse:    3,413 ms (30%)
  DF Build: 4,492 ms (40%)  ← BIGGEST TARGET
  Total:   11,320 ms
```

### Success Criteria

- [ ] ≥10% reduction in `parse_time` on SSD baseline (3,413ms → <3,100ms)
- [ ] ≥20% reduction in `df_build_time` on SSD baseline (4,492ms → <3,600ms)
- [ ] No regression in `read_time` or correctness
- [ ] All parity tests pass

### Target Performance

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Parse time | 3,413 ms | <3,100 ms | 10%+ |
| DF Build time | 4,492 ms | <3,600 ms | 20%+ |
| Total (C:) | 11,320 ms | <9,000 ms | 20%+ |

### Measurement Method

Compare phase timings before/after on Drive C:. Run 3x and take median.

---

### 8.1 Eliminate Per-Record Atomics with Rayon fold/reduce

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: Replaced per-record atomics with Rayon `fold` → `reduce` pattern in `ParallelMftReader::read_all_parallel_with_progress`. Each worker accumulates `(processed_count, skipped_count, Vec<ParseResult>)` locally, then reduces at the end.

**Problem**
- In `ParallelMftReader::read_all_parallel_with_progress`, each record updates atomics (`records_processed`, `skipped_records`).
- On very large MFTs this causes cache-line ping-pong and slows parsing.

**Plan (Preferred: Zero Atomics in Hot Loop)**
- Use Rayon's `fold` → `reduce` pattern:
  - Each worker returns `(processed_count, skipped_count, Vec<ParsedRecord>)` for its chunk.
  - Reduce counters at the end with a single aggregation.
- For progress reporting, use a **separate** `AtomicU64` updated once per chunk (or every ~8192 records), not per record.

**Why this is better than batching**
- Batching (local counters + periodic `fetch_add`) still has atomic operations in the hot loop.
- `fold/reduce` moves all counting to the reduction phase, completely eliminating contention during parsing.

**Expected Impact**
- 5-15% reduction in parse_time on CPU-bound SSD workloads.
- Negligible impact on HDD (I/O-bound), but no regression.

**Tasks**
1. ~~Refactor the Rayon `par_iter().flat_map(...)` to use `fold` → `reduce`.~~
2. ~~Each fold closure returns `(processed, skipped, records)`.~~
3. ~~Final reduce aggregates counts and flattens record vectors.~~
4. ~~Add a coarse progress counter (per-chunk or every N records) for UI feedback.~~
5. ~~Verify counts match previous behavior in tests.~~

---

### 8.2 Pre-size All Hot Vectors

**Problem**
- Column vectors and parsed record storage grow dynamically.
- At 20M+ records, capacity misses cause many reallocations.

**Plan**
- Use `Vec::with_capacity(estimated_records)` for:
  - All column vectors in DataFrame building.
  - The `parsed_records` vector (if still used).
  - Any intermediate buffers.

**Expected Impact**
- Reduces allocator pressure and memory fragmentation.
- 5-10% reduction in df_build_time.

**Tasks**
1. Compute `estimated_records` from MFT size / record size (or bitmap popcount).
2. Pre-size all column vectors with this estimate.
3. Pre-size `parsed_records` vector in parallel reader.
4. Verify no change in output.

---

### 8.3 Fuse Stats with DataFrame Building

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: Added `MftStats` struct and fused stats computation with DataFrame column building in a single pass. Stats are now computed inline during the loop that builds column vectors.

**Problem**
- `MftReader::read_mft_internal` walks `Vec<ParsedRecord>` multiple times:
  - Once for stats (counts, sizes, flag breakdowns).
  - Once to fill column `Vec`s for the DataFrame.

**Plan**
- Merge stats calculation into the same loop that builds the columns.
- Consider a "stats-only" fast path for users who don't need a DataFrame.

**Expected Impact**
- Halves the number of linear passes over records.
- 10-20% reduction in df_build_time.

**Tasks**
1. ~~Identify the stats loop over `parsed_records`.~~
2. ~~Identify the loop that constructs column vectors.~~
3. ~~Merge into a single loop that pushes fields and updates counters.~~
4. ~~Remove the redundant loop.~~
5. ~~Verify logs and DataFrame content remain correct.~~

---

### 8.4 Reuse Aligned Buffer (No Mutex)

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: Added `buffer: RefCell<AlignedBuffer>` field to `ParallelMftReader`. Buffer is pre-allocated in constructors and reused across chunks, only reallocating if a chunk is larger than the current buffer.

**Problem**
- `read_chunk` allocates a fresh `AlignedBuffer` for every chunk.
- This causes extra allocations and churn.

**Plan (Corrected: No Mutex Needed)**
- Since reads are sequential (not concurrent), use `&mut self` and store `AlignedBuffer` directly in `ParallelMftReader`.
- Resize with `ensure_capacity` as needed, but avoid reallocating for every chunk.

**Why not Mutex?**
- The read phase is single-threaded; parsing is parallel but happens after reading.
- A `Mutex` adds overhead for no benefit in this architecture.
- If we later pipeline multiple in-flight reads, allocate N buffers (one per in-flight read) rather than mutex-serializing.

**Note**: This still returns a per-chunk `Vec<u8>` copy. The bigger win (avoiding that copy) comes in M2/M4 with streaming/direct parsing.

**Expected Impact**
- Reduces aligned allocation churn.
- 2-5% reduction in read_time on volumes with many chunks.

**Tasks**
1. ~~Change `read_chunk` signature to take `&mut self`.~~
2. ~~Add `buffer: AlignedBuffer` field to `ParallelMftReader`.~~
3. ~~Initialize in `new_optimized` with size `chunk_size + SECTOR_SIZE`.~~
4. ~~In `read_chunk`, ensure capacity and reuse buffer.~~
5. ~~Run tests and validate behavior.~~

---

### 8.5 Tune Raw MFT Chunk Size

**Problem**
- `read_raw_internal` uses a hard-coded 1 MB chunk size, ignoring drive type optimizations.

**Plan**
- Use `DriveType::optimal_chunk_size()` to pick chunk size, mirroring `ParallelMftReader`.

**Guardrail**
- Chunk size must be a multiple of **record size** and **sector size** (and ideally cluster size) to avoid partial-record complexity.

**Expected Impact**
- 5-15% reduction in raw dump time on HDD.

**Tasks**
1. Compute `chunk_size` using drive type.
2. Ensure alignment constraints are met.
3. Pass to `generate_read_chunks` and buffer allocations.
4. Test raw dump on SSD and HDD volumes.

---

### 8.6 Chunk Planner: Merge Adjacent Tiny Ranges

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: Added `merge_adjacent_chunks()` function that merges chunks with gaps < 64 records (~64KB). This reduces the number of I/O operations on fragmented MFTs.

**Problem**
- `generate_read_chunks` may produce many small chunks on fragmented MFTs.
- Each chunk incurs kernel call overhead.

**Plan**
- Merge adjacent or nearly-adjacent ranges when the gap is small.
- Prefer fewer, larger reads even if we "over-read" some slack.

**Guardrails**
- Don't merge across extent boundaries if it would violate alignment.
- Maintain record boundary alignment.

**Expected Impact**
- Reduces kernel call overhead on fragmented volumes.
- 2-10% reduction in read_time on worst-case fragmented MFTs.

**Tasks**
1. ~~Add merge logic to `generate_read_chunks`.~~
2. ~~Define "small gap" threshold (e.g., < 64KB).~~
3. ~~Test on fragmented volume or synthetic test case.~~

---

### 8.7 Micro-Optimizations (Only After Phase Timings Exist)

**Ideas**
- Replace repeated `(skip_begin + i) * record_size` with an incrementing offset.
- Guard expensive stats/logging with level checks.

**Prerequisite**: M0 instrumentation must be in place to measure impact.

**Expected Impact**
- 1-3% reduction in parse_time (marginal but free).

---

## 9. Milestone 2 – Streaming & Prefetch Integration (HDD Optimization)

**Status**: ✅ COMPLETE (v0.1.37)

### Goal

Overlap I/O and parsing to reduce wall-clock time on HDD volumes.

### Baseline (from benchmark)
```
Drive D: (HDD)
  Read:     23,683 ms (51%)  ← BIGGEST TARGET
  Parse:     6,766 ms (14%)
  Total:    46,688 ms

Drive S: (HDD, large)
  Read:     94,040 ms (59%)  ← BIGGEST TARGET
  Total:   160,642 ms
```

### Success Criteria

- [ ] ≥15% reduction in wall-clock time on HDD baseline (D: 46.7s → <40s)
- [ ] ≥20% reduction on large HDD (S: 160.6s → <130s)
- [ ] Consistent progress reporting across all modes
- [ ] No correctness regressions

### Target Performance

| Drive | Before | After | Improvement |
|-------|--------|-------|-------------|
| D: (HDD) | 46.7s | <40s | 15%+ |
| S: (HDD, large) | 160.6s | <130s | 20%+ |

### Measurement Method

Compare wall-clock time before/after on Drive D: and S:. Run 3x and take median.

---

### 9.1 Add `MftReadMode` Configuration

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: Added `MftReadMode` enum with `Auto`, `Parallel`, `Streaming`, `Prefetch` variants. Added `mode` field to `MftReader` with `with_mode()` builder method. Added `--mode` CLI flag to `read` and `bench` commands.

**Plan**
- Introduce an enum `MftReadMode` in `reader.rs`:
  - `Parallel` (current default).
  - `Streaming`.
  - `Prefetch`.
  - `Overlapped` (future, M3).
- Add `read_mode: MftReadMode` field to `MftReader` with a builder-style `with_read_mode` method.

**Additional Requirements**
- Log "mode chosen" + "chunk size" + "queue depth (if overlapped)" at start of run.
- Expose "merge extensions on/off" if it's a big cost and not always needed.

**Tasks**
1. ~~Define `MftReadMode` enum.~~
2. ~~Add `read_mode` field to `MftReader` with default `Parallel`.~~
3. ~~Add `with_read_mode` to configure it.~~
4. ~~Add CLI flag `--mode parallel|streaming|prefetch|overlapped`.~~

---

### 9.2 Wire Up `StreamingMftReader`

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: `read_mft_internal` now branches on `MftReadMode` and uses `StreamingMftReader` when mode is `Streaming`.

**Plan**
- In `read_mft_internal`, when `read_mode == Streaming`:
  - Construct `StreamingMftReader::new(extent_map.clone(), bitmap.clone(), drive_type)`.
  - Call `read_all_streaming(handle, merge_extensions = true, progress_callback)`.

**Expected Impact**
- Lower peak memory usage on large volumes.
- Useful for memory-constrained environments.

**Tasks**
1. ~~Branch on `self.read_mode` in `read_mft_internal`.~~
2. ~~For `Streaming`, use `StreamingMftReader` to produce `Vec<ParsedRecord>`.~~
3. ~~Ensure extension merging is handled via `MftRecordMerger`.~~
4. ~~Run tests and compare results with the `Parallel` mode.~~

---

### 9.3 Wire Up `PrefetchMftReader`

**Status**: ✅ COMPLETE (v0.1.37)

**Implementation**: `read_mft_internal` now uses `PrefetchMftReader` when mode is `Prefetch`. Auto mode selects `Prefetch` for HDD drives.

**Plan**
- In `read_mft_internal`, when `read_mode == Prefetch`:
  - Construct `PrefetchMftReader` similarly.
  - Call `read_all_prefetch` with merge and progress callback.

**Expected Impact**
- Overlaps I/O latency with CPU work.
- On HDDs, can approach 2× improvement over strict "read-then-parse".

**Important**: Ensure progress reporting is consistent across modes, or users will think a mode "hangs".

**Tasks**
1. ~~Add a branch for `Prefetch` in `read_mft_internal`.~~
2. ~~Pass the same `extent_map`, `bitmap`, `drive_type`, and `HANDLE`.~~
3. ~~Ensure DataFrame content matches the `Parallel` path on test volumes.~~

---

### 9.4 Heuristics and Configuration

**Default Mode Selection** (data-driven, after benchmarking):
- HDD default: **Prefetch** (overlaps I/O with parsing)
- SSD/NVMe default: **Parallel** (if parsing dominates) OR **Streaming** (if memory matters)

**Tasks**
1. Add simple CLI flag to select mode.
2. Implement auto-detection based on `DriveType`.
3. Allow manual override via CLI/config.

---



## 10. Milestone 3 – Advanced I/O Parallelism (Overlapped I/O)

### Goal

Allow multiple in-flight reads using Windows overlapped I/O to overlap disk latency and parsing.

### Success Criteria

- [ ] ≥10% additional reduction in wall-clock time on HDD (beyond M2).
- [ ] Stable under various queue depths.
- [ ] Feature-flagged and opt-in until mature.

### Key Risks

- Too much queue depth on HDD can degrade throughput (seeks/queue thrash).
- Alignment rules must remain consistent (sector alignment, record boundaries).
- Error handling becomes complex (partial reads, cancellation, device-specific quirks).

---

### 10.1 Stepped Implementation Approach

**Do NOT jump straight to full IOCP. Follow these steps:**

1. **Step 1: Validate Prefetch Helps**
   - Confirm M2 prefetch mode shows measurable overlap benefit.
   - If prefetch doesn't help, overlapped I/O won't either.

2. **Step 2: Overlapped-Lite (Fixed Queue Depth 2-4)**
   - Simple `OVERLAPPED` + `GetOverlappedResult`.
   - No IOCP complexity.
   - Validate correctness and measure improvement.

3. **Step 3: IOCP (Only If Needed)**
   - Only if overlapped-lite shows promise but needs more queue depth.
   - Consider using `tokio` or `mio` for async I/O abstraction.

---

### 10.2 Implementation Details

**High-Level Design**
- New reader `OverlappedMftReader` that:
  - Opens the volume handle with `FILE_FLAG_OVERLAPPED`.
  - Uses `generate_read_chunks` for offsets.
  - Issues N overlapped `ReadFile` calls with distinct `OVERLAPPED` structs.
  - Waits for completions via `GetOverlappedResult`.
  - Feeds completed buffers into a parsing pool.

**Tasks**
1. Build a tiny Rust prototype outside UFFS:
   - Opens a regular file with `FILE_FLAG_OVERLAPPED`.
   - Issues 2–4 overlapped reads.
   - Waits for all and verifies data.
2. Port to `uffs-mft`:
   - Implement `OverlappedMftReader` with fixed in-flight reads.
   - Integrate with `generate_read_chunks` and parsing logic.
3. Add `MftReadMode::Overlapped` and wire through `MftReader`.
4. Add feature flag `overlapped-io` (off by default).
5. Test on real NTFS volumes and compare timings.

---

## 11. Milestone 4 – Parsing & Data Layout Overhaul

### Goal

Move toward struct-of-arrays (SoA) layout and direct-to-columns parsing to reduce allocations and copies.

### Success Criteria

- [ ] ≥20% reduction in `df_build_time`.
- [ ] No increase in `parse_time`.
- [ ] All parity tests pass.

### Key Insight

The current approach parses into `ParsedRecord` structs (array-of-structs), then walks them again to populate 20+ column vectors (struct-of-arrays) for Polars. This double-handling wastes CPU and memory bandwidth.

---

### 11.1 Incremental Approach (Recommended)

**Do NOT rewrite everything at once. Follow these steps:**

1. **Step 1: `ParsedColumns` + Conversion**
   - Define `ParsedColumns` struct holding column vectors directly.
   - Write `parsed_records_to_columns(Vec<ParsedRecord>) -> ParsedColumns`.
   - Switch DataFrame construction to use `ParsedColumns`.
   - Validate correctness.

2. **Step 2: Direct-to-Columns for Easy Fields**
   - Parse fixed fields (record number, flags, sizes) directly into columns.
   - Keep complex fields (timestamps, names, extensions) in `ParsedRecord` for now.

3. **Step 3: Direct-to-Columns for Complex Fields**
   - Handle timestamps, names, and extension attributes directly.
   - This is where correctness bugs are most likely—test heavily.

---

### 11.2 Hot vs Cold Column Grouping

**Idea**: SoA doesn't have to mean "all columns at once."

- **Hot columns** (always used): record_number, parent_record, filename, flags, size
- **Cold columns** (rarely used): all timestamps, security_id, reparse_point, etc.

**Plan**
- Optionally compute cold columns behind a flag.
- Reduces work for use cases that only need hot columns.

---

### 11.3 Tasks

1. Define `ParsedColumns` in `reader.rs` mirroring the current DataFrame schema.
2. Write conversion function `parsed_records_to_columns`.
3. Switch DataFrame construction to use `ParsedColumns`.
4. Measure df_build_time improvement.
5. Once stable, explore direct-to-columns parsing.

---

## 12. Milestone 5 – Benchmarks, Auto-Tuning, and Validation

### Goal

Ensure we can measure improvements and keep them over time.

### Success Criteria

- [ ] Reproducible benchmark suite exists.
- [ ] CI runs benchmarks against golden datasets.
- [ ] Auto-tuning selects optimal mode per drive type.

---

### 12.1 Benchmarking Tool

**Tasks**
1. Add a small benchmarking binary that:
   - Accepts a volume letter or raw MFT file.
   - Accepts read mode and options.
   - Measures end-to-end time and phase timings.
   - Outputs JSON for scripting (`--json` flag).
2. Add separate "read only", "parse only", "df build only" toggles if feasible.
3. Document how to run benchmarks on typical SSD/HDD setups.

---

### 12.2 CI Integration

**Tasks**
1. Store benchmark baselines in CI artifacts.
2. Run against raw MFT dumps (CI can't access live volumes).
3. Fail CI if performance regresses beyond threshold (e.g., >5%).

---

### 12.3 Auto-Tuning

**Tasks**
1. Use benchmark data to tune `DriveType::optimal_chunk_size`.
2. Choose default `MftReadMode` per drive type based on measurements.
3. Document tuning decisions and rationale.

---

## 13. PR-Sized Implementation Order

Based on actual benchmark data, here's the prioritized order of PRs:

### Phase 0: Investigation (M0.5) - 🚨 DO FIRST
1. **PR1**: Investigate bitmap skip logic (why in_use == total for all drives?)
2. **PR2**: Add bitmap diagnostic logging

### Phase 1: Foundation (M0) - ✅ COMPLETE
~~3. **PR3**: Add phase timing instrumentation~~ (Done in v0.1.30)
~~4. **PR4**: Add structured metrics output (JSON)~~ (Done: `bench` and `bench-all` commands)
~~5. **PR5**: Establish and document baselines~~ (Done: my_benchmark.json)

### Phase 2: SSD Optimization (M1) - 🎯 HIGH PRIORITY
Target: Drive C: 11.3s → <9s (20% improvement)

6. **PR6**: Replace per-record atomics with fold/reduce (parse: -10%)
7. **PR7**: Pre-size all hot vectors (df_build: -5%)
8. **PR8**: Fuse stats with DataFrame building (df_build: -15%)
9. **PR9**: Reuse aligned buffer (read: -2%)

### Phase 3: HDD Optimization (M2) - 🎯 HIGH PRIORITY
Target: Drive D: 46.7s → <40s (15% improvement)

10. **PR10**: Add `MftReadMode` enum and CLI flag
11. **PR11**: Wire up `PrefetchMftReader` (read: -20% on HDD)
12. **PR12**: Tune HDD chunk size (4MB → 8-16MB?)
13. **PR13**: Implement mode auto-selection heuristics

### Phase 4: Data Layout (M4) - MEDIUM PRIORITY
Target: df_build_time -20% additional

14. **PR14**: `ParsedColumns` + conversion (SoA layout)
15. **PR15**: Direct-to-columns for easy fields
16. **PR16**: Hot/cold column grouping

### Phase 5: Advanced I/O (M3) - LOWER PRIORITY
Only if M2 shows promise

17. **PR17**: Overlapped I/O prototype (feature-flagged)
18. **PR18**: IOCP integration (if needed)

### Phase 6: Polish (M5)
19. **PR19**: CI benchmark integration
20. **PR20**: Auto-tuning based on drive characteristics

---

## 14. How to Work Through This

1. **Start with M0.5** (bitmap investigation). This could be a quick 15-20% win.
2. **M0 is complete** - we have baseline measurements.
3. For each change:
   - State your hypothesis ("I expect X% improvement in Y phase").
   - Make minimal edits.
   - Run `cargo test -p uffs-mft`.
   - Run `uffs_mft bench --drive C --runs 3` (SSD) or `--drive D` (HDD).
   - Accept or reject based on data.
4. **Keep PRs small** (one optimization per PR).
5. **Document results** in commit messages.
6. **Re-run `bench-all`** after each milestone to track cumulative progress.

### Validation Commands

```powershell
# Quick SSD benchmark (Drive C:)
uffs_mft bench --drive C --runs 3

# Quick HDD benchmark (Drive D:)
uffs_mft bench --drive D --runs 3

# Full benchmark (all drives, save to file)
uffs_mft bench-all --output benchmark_after_PR6.json --runs 3
```

### Success Tracking

| Milestone | Target | Baseline (v0.1.30) | Current (v0.1.37) | Improvement | Status |
|-----------|--------|-------------------|-------------------|-------------|--------|
| M0.5 (Bitmap) | Understand issue | 100% util (broken) | 67% util (fixed) | ✅ Fixed | ✅ DONE |
| M1 (SSD C:) | <9s | 11.3s | **10.4s** | **8% faster** | 🟡 PARTIAL |
| M2 (HDD D:) | <40s | 46.7s | **50.3s** | ❌ +8% slower | 🔴 REGRESSED |
| M4 (SoA) | df_build -20% | 4.5s | 4.7s | - | 🔴 TODO |
| **Total (7 drives)** | **<180s** | **315s** | **253s** | **20% faster** | 🟡 IN PROGRESS |

#### Detailed Results (v0.1.37 Benchmark - 2026-01-18)

| Drive | Type | Records | Total (ms) | Read | Parse | Merge | DF Build | MB/s |
|-------|------|---------|------------|------|-------|-------|----------|------|
| **C:** | SSD | 3.06M | 10,413 | 1,708 (16%) | 2,848 (27%) | 1,138 (11%) | 4,660 (45%) | 437 |
| **D:** | HDD | 4.77M | 50,279 | 25,397 (51%) | 7,256 (14%) | 3,627 (7%) | 13,674 (27%) | 96 |
| **E:** | HDD | 2.93M | 45,711 | 28,248 (62%) | 8,070 (18%) | 4,035 (9%) | 4,406 (10%) | 63 |
| **F:** | SSD | 2.17M | 6,808 | 1,161 (17%) | 1,936 (28%) | 774 (11%) | 2,928 (43%) | 668 |
| **G:** | Unk | 45K | 290 | 179 (62%) | 51 (18%) | 25 (9%) | 31 (11%) | 153 |
| **M:** | HDD | 1.91M | 24,004 | 15,081 (63%) | 4,308 (18%) | 2,154 (9%) | 2,425 (10%) | 102 |
| **S:** | HDD | 8.28M | 115,340 | 61,623 (53%) | 17,606 (15%) | 8,802 (8%) | 27,280 (24%) | 100 |

#### Analysis

**SSD Performance (C:, F:):**
- ✅ DF Build is the bottleneck (43-45% of time)
- ✅ Read is fast (16-17% of time)
- 🎯 **Next optimization: M4 SoA layout to reduce DF Build time**

**HDD Performance (D:, E:, M:, S:):**
- ❌ Read is the bottleneck (51-63% of time)
- ❌ D: regressed from 46.7s to 50.3s (need investigation)
- 🎯 **Next optimization: M3 Overlapped I/O for true async reads**

**Implemented Optimizations (v0.1.37)**:
- ✅ M0.5: Fixed bitmap reading (sector alignment + access flags)
- ✅ M1 8.1: Rayon fold/reduce pattern (eliminates per-record atomics)
- ✅ M1 8.3: Fused stats with DataFrame building (single pass)
- ✅ M1 8.4: Reusable aligned buffer in ParallelMftReader
- ✅ M1 8.6: Merge adjacent tiny chunks (reduces I/O ops)
- ✅ M2 9.1: MftReadMode enum with CLI --mode flag
- ✅ M2 9.2-9.3: Wired up StreamingMftReader and PrefetchMftReader
- ✅ Auto mode selection: SSD→Parallel, HDD→Prefetch

This plan is intended to guide us from solid performance today to **best-in-class MFT processing** over several iterations, without sacrificing correctness or maintainability.

---

_End of plan._
