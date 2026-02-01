# C++ I/O Pipeline Port - Implementation Guide

> **Goal**: Port the C++ I/O pipeline synchronization model to Rust, fixing the ~40 missing files issue caused by incorrect bitmap-based skip range calculation.

**Branch**: `feature/cpp-io-pipeline-port`
**Status**: 🚧 IN PROGRESS (Scaffolding Complete)
**Date**: 2026-02-01
**Last Updated**: 2026-02-01

## Usage

To enable the C++ I/O pipeline (once implemented), use:

```bash
# Via environment variable
UFFS_IO_ALGO=cpp_port uffs index

# Via CLI flag
uffs --io-algo cpp *.txt
```

Currently, selecting `cpp_port` will print a message indicating the implementation is coming soon.

---

## Problem Statement

The Rust implementation is missing ~40 files compared to C++ because of a fundamental difference in how bitmap-based skip ranges are calculated:

| Aspect | C++ Implementation | Rust Implementation |
|--------|-------------------|---------------------|
| **Bitmap Reading** | Async IOCP (same as data) | Synchronous (before data) |
| **Skip Range Calculation** | After ALL bitmap chunks complete | At chunk generation time |
| **Skip Range Storage** | Atomic (updated dynamically) | Static (fixed at generation) |
| **Synchronization** | Explicit barrier after bitmap | None |

**Root Cause (conceptual)**: the current Rust path treats skip ranges as *static per-chunk fields* computed up-front, while the C++ code treats them as *atomic values* that are only finalized **after** the bitmap phase has fully completed. The C++ approach guarantees that:

- Skip ranges are based on the **complete** bitmap, not partial state.
- Every data chunk sees the **final** skip_begin/skip_end values when its I/O is queued.

Even if the Rust implementation reads the whole bitmap synchronously, its skip range logic lives in the **chunk generation** helpers (e.g. `generate_read_chunks`, `generate_precise_read_chunks`) instead of behind a clear "bitmap complete" barrier. The C++ port pipeline we design below will mirror the C++ model instead of trying to tweak the existing helpers.

### Current Rust I/O (what exists today)

Today the Rust I/O stack is centered around `ReadChunk` in `crates/uffs-mft/src/io.rs`:

- `ReadChunk { disk_offset, start_frs, record_count, skip_begin, skip_end }` is a **static** description of a chunk.
- `generate_read_chunks` and `generate_precise_read_chunks`:
  - Take `MftExtentMap` + optional `MftBitmap`.
  - Compute `skip_begin`/`skip_end` during chunk generation.
  - Return a `Vec<ReadChunk>` where skip ranges never change afterwards.
- All current sliding-window / bulk readers (`read_all_sliding_window_iocp*`, `read_all_sliding_window_iocp_to_index*`, including the current `*_cpp_port` variant) consume these `ReadChunk`s directly.

This design works reasonably well but makes it **hard** to precisely mirror the C++ behavior around:

- Bitmap completion as a synchronization barrier.
- Updating skip ranges for **all** data chunks at once.
- Keeping a clear separation between:
  - Data chunk layout (`vcn`, `cluster_count`, `lcn`).
  - Bitmap-driven skip computation.

### Design decision: whole new I/O pipeline (not a patch)

For the C++ I/O pipeline port we will implement a **separate** I/O path instead of trying to patch `ReadChunk` and `generate_*` helpers.

**We keep (and reuse):**

- `MftExtentMap` + extent discovery for `$MFT::$DATA`.
- `MftBitmap` from `crates/uffs-mft/src/platform.rs` (reading `$MFT::$BITMAP`).
- `IoCompletionPort` and overlapped I/O helpers from `crates/uffs-mft/src/io.rs`.
- `CppParsePipeline` and existing parsing logic.

**We do *not* rely on for the new C++ I/O path:**

- `ReadChunk` and `generate_read_chunks` / `generate_precise_read_chunks`.
- Any logic that bakes `skip_begin`/`skip_end` into static structs at generation time.

Instead, the new pipeline lives in `crates/uffs-mft/src/cpp_io_pipeline.rs` and is wired into
`ParallelMftReader::read_all_sliding_window_iocp_to_index_cpp_port` **only when**
`IoPipelineAlgorithm::CppPort` (via `UFFS_IO_ALGO=cpp_port` or `--io-algo=cpp`) is selected.

The existing Rust I/O modes continue to use `ReadChunk` unchanged; the C++ I/O port is a
**clean, parallel implementation** that follows the C++ architecture.

---

## C++ I/O Pipeline Architecture

### The Two-Phase I/O Model

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                    C++ MFT I/O Pipeline (mft_reader.hpp)                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  PHASE 1: Bitmap Reading (async IOCP)                                │   │
│  │  ────────────────────────────────────────────────────────────────    │   │
│  │  • queue_next() issues bitmap reads first (jbitmap < bitmap_ret_ptrs)│   │
│  │  • Each completion copies bitmap data to mft_bitmap vector           │   │
│  │  • Counts valid_records using popcount                               │   │
│  │  • nbitmap_chunks_left tracks remaining bitmap chunks                │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│         │                                                                    │
│         ▼ (when nbitmap_chunks_left == 0)                                   │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  SYNCHRONIZATION POINT (lines 245-296)                               │   │
│  │  ────────────────────────────────────────────────────────────────    │   │
│  │  • lock(q->p)->reserve(valid_records) - pre-allocate index           │   │
│  │  • FOR EACH data chunk:                                              │   │
│  │    - Calculate skip_records_begin from bitmap (bit-by-bit scan)      │   │
│  │    - Calculate skip_records_end from bitmap (bit-by-bit scan)        │   │
│  │    - Convert to skip_clusters_begin/end                              │   │
│  │    - Store atomically: i->skip_begin.store(), i->skip_end.store()    │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│         │                                                                    │
│         ▼                                                                    │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │  PHASE 2: Data Reading (async IOCP)                                  │   │
│  │  ────────────────────────────────────────────────────────────────    │   │
│  │  • queue_next() issues data reads (jdata < data_ret_ptrs)            │   │
│  │  • Uses UPDATED skip_begin/skip_end from synchronization point       │   │
│  │  • Each completion: preload_concurrent() + lock()->load()            │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key C++ Code Sections

| Section | File | Lines | Description |
|---------|------|-------|-------------|
| `RetPtr` struct | mft_reader.hpp | 40-63 | Chunk with atomic skip_begin/skip_end |
| `ReadOperation::operator()` | mft_reader.hpp | 204-313 | Bitmap/data completion handler |
| Bitmap sync point | mft_reader.hpp | 245-296 | Recalculate ALL skip ranges |
| `queue_next()` | mft_reader.hpp | 321-386 | Issues next read (bitmap first, then data) |
| Concurrency init | mft_reader.hpp | 482-485 | Starts with 2 concurrent reads |

---

## Implementation Plan (step‑by‑step)

This section is written so that a **junior SWE** can implement the pipeline by following
concrete steps. The goal is to end up with:

- A new C++-style I/O pipeline implemented in `crates/uffs-mft/src/cpp_io_pipeline.rs`.
- `ParallelMftReader::read_all_sliding_window_iocp_to_index_cpp_port` calling this
  pipeline when `IoPipelineAlgorithm::CppPort` is selected.

At a high level, the work breaks down into four phases.

### Phase 1: CppDataChunk with atomic skip ranges

**File:** `crates/uffs-mft/src/cpp_io_pipeline.rs`

This phase is mostly done already. We mirror the C++ `RetPtr` struct:

```rust
/// MFT data chunk with atomic skip ranges (matches C++ RetPtr).
///
/// Skip ranges are initially 0 and updated after bitmap completes.
pub struct CppDataChunk {
    pub vcn: u64,
    pub cluster_count: u64,
    pub lcn: i64,
    pub skip_begin: AtomicU64,
    pub skip_end: AtomicU64,
}
```

**What you need to know / do:**

1. This struct already exists with helper methods like `effective_cluster_count`,
   `effective_lcn`, `start_frs`, etc. in `cpp_io_pipeline.rs`.
2. Treat `skip_begin`/`skip_end` as **cluster counts**, not record counts – this matches
   the C++ code, which stores skips in clusters after computing them from records.
3. Always update skip ranges via the provided `update_skip_ranges(skip_begin, skip_end)`
   helper; it enforces invariants via `debug_assert!`.
4. When you build the pipeline later, you will:
   - Construct `Vec<CppDataChunk>` from the `$MFT::$DATA` extents (`MftExtentMap`).
   - Use these chunks to derive I/O operations for the sliding window loop.

You **do not** need to touch `ReadChunk` for this phase.

### Phase 2: CppBitmapReader (bitmap I/O + bookkeeping)

**Goal:** read `$MFT::$BITMAP` using extents and maintain enough state to:

- Know when **all** bitmap chunks have finished.
- Compute `valid_records` (optional but nice for pre-allocation).
- Later, drive per‑chunk skip range computation.

**Suggested new types (in `cpp_io_pipeline.rs`):**

```rust
/// One physical bitmap read (matches a retrieval pointer range).
struct BitmapChunk {
    vcn: u64,
    cluster_count: u64,
    lcn: i64,
    /// Offset into the final bitmap buffer where this chunk should be copied.
    bitmap_offset_bytes: usize,
    /// How many bytes we actually expect to read for this chunk.
    byte_len: usize,
}

pub struct CppBitmapReader {
    /// All bitmap chunks we plan to read.
    bitmap_chunks: Vec<BitmapChunk>,
    /// Final contiguous `$MFT::$BITMAP` buffer (same layout as `MftBitmap::from_bytes`).
    bitmap_data: Vec<u8>,
    /// How many chunks are still outstanding.
    chunks_remaining: AtomicUsize,
    /// Total number of in‑use records (popcount of bitmap).
    valid_records: AtomicU64,
}
```

**Implementation steps:**

1. **Discover bitmap extents**
   - Reuse the logic in `platform::Volume::get_mft_bitmap_internal`:
     - It already opens `"C:\\$MFT::$BITMAP"` and calls `get_retrieval_pointers`.
     - Either:
       - Factor out a helper that returns the bitmap extents, **or**
       - Re‑call `get_retrieval_pointers` from `CppBitmapReader` using the same flags.

2. **Build `BitmapChunk`s**
   - For each extent returned by `get_retrieval_pointers`:
     - Compute how many bytes that extent contributes to the bitmap file (same math as
       in `get_mft_bitmap_internal` – full clusters, then truncate overall to file size).
     - Assign `bitmap_offset_bytes` (running total into `bitmap_data`).
   - Allocate `bitmap_data` large enough to hold the full bitmap file.

3. **Wire bitmap reads through `IoCompletionPort`**
   - Use the existing `IoCompletionPort` wrapper in `crates/uffs-mft/src/io.rs`.
   - For each `BitmapChunk`, queue an overlapped `ReadFile` against the *volume* handle
     (like C++ does):
     - Disk offset = `extent.lcn * bytes_per_cluster`.
     - Buffer = a per‑chunk buffer or a slice into `bitmap_data`.
     - On completion, copy bytes into `bitmap_data[bitmap_offset_bytes..]` if you used
       a temporary buffer.
   - Initialize `chunks_remaining = bitmap_chunks.len()`.

4. **Track completions and valid record count**
   - Each time a bitmap read completes:
     - Decrement `chunks_remaining` with `fetch_sub`.
     - Popcount the bytes you just read (use the existing `MftBitmap::count_in_use` or
       a small lookup table) and add to `valid_records`.

5. **Expose a simple API:**
   - `fn start_bitmap_reads(&self, iocp: &IoCompletionPort, volume_handle: HANDLE)` –
     queues all bitmap reads.
   - `fn on_chunk_complete(&self, chunk_index: usize, bytes_read: u32)` – called from
     the IOCP loop when a bitmap operation completes.
   - `fn is_complete(&self) -> bool` – true when `chunks_remaining == 0`.
   - `fn to_mft_bitmap(self) -> MftBitmap` – once complete, wrap `bitmap_data` in the
     existing `MftBitmap` type for downstream use.

You do **not** need to optimize this perfectly on the first pass; correctness and clear
structure are more important than micro‑optimizations.

### Phase 3: Synchronization point (recompute skip ranges)

Once all bitmap chunks are done, we must:

1. Convert the raw `bitmap_data` into an `MftBitmap`.
2. Recalculate `skip_begin`/`skip_end` for **every** `CppDataChunk`.
3. Store these values atomically via `CppDataChunk::update_skip_ranges`.

**Suggested API in `CppBitmapReader`:**

```rust
impl CppBitmapReader {
    /// Called exactly once, after `is_complete()` becomes true.
    pub fn on_all_chunks_complete(
        self,
        data_chunks: &[Arc<CppDataChunk>],
        extent_map: &MftExtentMap,
    ) {
        // 1) Wrap into MftBitmap
        let bitmap = MftBitmap::from_bytes(self.bitmap_data);

        // 2) For each data chunk, compute record-level skip range and
        //    convert to cluster-level skip_begin/skip_end.
        for chunk in data_chunks {
            // (Pseudo-code; see below for explanation.)
            let (skip_records_begin, skip_records_end) =
                calculate_record_skip_range_for_chunk(chunk, &bitmap, extent_map);

            let (skip_clusters_begin, skip_clusters_end) =
                convert_record_skips_to_clusters(
                    skip_records_begin,
                    skip_records_end,
                    extent_map.bytes_per_cluster,
                    extent_map.bytes_per_record,
                );

            chunk.update_skip_ranges(skip_clusters_begin, skip_clusters_end);
        }
    }
}
```

**How to compute skip ranges (conceptually):**

For each `CppDataChunk`:

1. Compute its **record range**:
   - Use `chunk.start_frs(cluster_size, record_size)` and
     `chunk.record_count(cluster_size, record_size)` (helpers already exist).
   - This gives `[chunk_frs_start, chunk_frs_end)` in record units.
2. Use `MftBitmap::calculate_skip_range(chunk_frs_start, chunk_frs_end)` (already
   implemented and used by `generate_read_chunks`) to get:
   - `skip_records_begin`, `skip_records_end` (in *records*).
3. Convert record-level skips to cluster-level skips to store in `CppDataChunk`:
   - `records_per_cluster = bytes_per_cluster / bytes_per_record`.
   - `skip_clusters_begin = skip_records_begin / records_per_cluster`.
   - `skip_clusters_end = skip_records_end / records_per_cluster`.
4. Call `chunk.update_skip_ranges(skip_clusters_begin, skip_clusters_end)`.

This exactly mirrors the C++ behavior: the bitmap is advisory for I/O (skipping clean
regions) but the **authoritative** in‑use flag is still the record header.

### Phase 4: CppIoPipeline orchestrator + integration

The final phase is to build a small orchestrator that:

1. Builds `Vec<Arc<CppDataChunk>>` from the `$MFT::$DATA` extents.
2. Uses `CppBitmapReader` to read the bitmap via IOCP.
3. After bitmap completion, recomputes skip ranges for all data chunks.
4. Runs a **sliding window IOCP loop** over data chunks, feeding buffers into
   `CppParsePipeline`.

**A. Build data chunks from `MftExtentMap`**

- In `ParallelMftReader::read_all_sliding_window_iocp_to_index_cpp_port`:
  1. Access `self.extent_map` (already present).
  2. For each `MftExtent { vcn, cluster_count, lcn }`:
     - Split into smaller logical chunks based on desired `io_chunk_size`.
     - For each piece, create a `CppDataChunk::new(vcn_piece, clusters_in_piece, lcn_piece)`
       and wrap it in `Arc`.
  3. Collect into `Vec<Arc<CppDataChunk>>`.

**B. Initialize bitmap reader and IOCP**

1. Create an `IoCompletionPort` and associate it with the overlapped volume handle
   (this code already exists in `read_all_sliding_window_iocp_to_index_cpp_port`).
2. Create a `CppBitmapReader` with bitmap extents and call
   `start_bitmap_reads(&iocp, overlapped_handle)`.
3. Queue **no data reads yet** – just like C++ starts with bitmap only.

**C. Sliding window loop (queue_next + completion handling)**

Inside `read_all_sliding_window_iocp_to_index_cpp_port`, replace the current
ad‑hoc I/O loop with a structure that mirrors `queue_next()` and the IOCP loop in
`mft_reader.hpp`:

1. Maintain indices:
   - `next_bitmap_idx` into `bitmap_chunks`.
   - `next_data_idx` into `data_chunks`.
   - `concurrency` (e.g. 2 for HDD, more for NVMe).
2. While there is work **or** in‑flight operations:
   - While `in_flight < concurrency`:
     - If bitmap is not complete and there are bitmap chunks left → queue next
       bitmap read.
     - Else if bitmap is complete and there are data chunks left → queue next
       data read (using the updated skip ranges in `CppDataChunk`).
   - Call `GetQueuedCompletionStatus` through `IoCompletionPort::get`.
   - On completion:
     - If it was a bitmap op → call `CppBitmapReader::on_chunk_complete(...)` and
       check `is_complete()`. When it flips to true, call
       `on_all_chunks_complete(&data_chunks, &extent_map)`.
     - If it was a data op → feed the buffer into `CppParsePipeline`:
       - `pipeline.preload_concurrent(...)` then `pipeline.load(...)`, matching the
         comments and existing use in `read_all_sliding_window_iocp_to_index_cpp_port`.

**D. IoPipelineAlgorithm wiring**

- In `crates/uffs-mft/src/index.rs`, `IoPipelineAlgorithm::from_env()` already
  understands `UFFS_IO_ALGO=cpp_port`.
- In `ParallelMftReader::read_all_sliding_window_iocp_to_index_cpp_port`:
  - Read the I/O algorithm via `IoPipelineAlgorithm::from_env()`.
  - If it is `IoPipelineAlgorithm::CppPort`, use the new `CppIoPipeline` code path.
  - Otherwise, you may temporarily fall back to the existing `ReadChunk`‑based path
    to keep experiments incremental.

Once this is in place, the full C++ port configuration will be:

- `--parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp`
- or via env vars: `UFFS_PARSE_ALGO=cpp_port`, `UFFS_TREE_ALGO=cpp_port`,
  `UFFS_IO_ALGO=cpp_port`.

---

## Testing & validation (trial_run.ps1)

We use `docs/architecture/Investigation/trial_run.ps1` as the main comparison harness.
It currently runs four flows per drive:

1. **Rust (current)**    – default Rust implementation.
2. **C++**               – original C++ `uffs.com`.
3. **Rust (new tree)**   – Rust with C++ tree algorithm port.
4. **Rust (cpp full)**   – Rust with both C++ parsing **and** tree algorithm ports.

After implementing the new I/O pipeline, we want a **fifth** flow:

> **Rust (cpp io): drive X** – Rust with C++ parsing, tree, **and** I/O pipeline.

### 1. Add new output/log file names

In the `$runDiskGroup` function inside `trial_run.ps1` (around the existing
`$rustOut`, `$cppOut`, `$rustNewOut`, `$rustCppFullOut` variables), add two more:

```powershell
$rustCppIoOut = "rust_cpp_io_${driveLower}.txt"
$rustCppIoLog = "rust_cpp_io_${driveLower}.log"
```

and extend the `Files` / `Logs` objects accordingly:

```powershell
Files  = [pscustomobject]@{
    Rust        = $rustOut
    Cpp         = $cppOut
    RustNew     = $rustNewOut
    RustCppFull = $rustCppFullOut
    RustCppIo   = $rustCppIoOut
}
Logs   = [pscustomobject]@{
    Rust        = $rustLog
    Cpp         = $cppLog
    RustNew     = $rustNewLog
    RustCppFull = $rustCppFullLog
    RustCppIo   = $rustCppIoLog
}
```

### 2. Add the new run entry

Still inside `$runDiskGroup`, after the existing **Rust (cpp full)** block, add a
new `Run-LoggedLocal` call:

```powershell
if ($HasRust) {
    $runs += Run-LoggedLocal -Title "Rust (cpp io): drive $Drive" `
        -CmdLine ("`"$UffsExe`" `"*`" --drive $Drive " +
                  "--parse-algo=cpp_port --tree-algo=cpp --io-algo=cpp --no-bitmap > `"$rustCppIoOut`"") `
        -LogFileName $rustCppIoLog
} else {
    $runs += [pscustomobject]@{
        Drive=$Drive; Title="Rust (cpp io)"; Command="";
        LogFile=$rustCppIoLog; DurationMs=$null; ExitCode=$null
    }
}
```

This ensures the new pipeline is exercised with the full C++ port configuration
(`parse_algo=cpp_port`, `tree_algo=cpp`, `io_algo=cpp`).

### 3. Wire the new flow into the markdown summary

At the bottom of `trial_run.ps1`, in the section that builds the `Scan Outputs`
table, extend the flow mapping (inside the `foreach ($run in $r.Runs)` loop):

```powershell
if     ($run.Title -like "Rust (current)*")   { $outFile = $r.Files.Rust }
elseif ($run.Title -like "C++*")             { $outFile = $r.Files.Cpp }
elseif ($run.Title -like "Rust (new tree)*") { $outFile = $r.Files.RustNew }
elseif ($run.Title -like "Rust (cpp full)*") { $outFile = $r.Files.RustCppFull }
elseif ($run.Title -like "Rust (cpp io)*")   { $outFile = $r.Files.RustCppIo }
```

This makes the new flow appear alongside the others in `trial_run.md`, with
output size, exit code, and duration columns.

### 4. How to run and what to look for

1. Build the Rust binaries (following the standard project instructions).
2. From PowerShell, run:
   - `.	rial_run.ps1` (optionally with `-Drives C,D` to restrict drives).
3. After completion, open the generated `trial_run.md` and look under
   **Scan Outputs** for each drive:
   - Compare **output sizes** between:
     - `C++: drive X`
     - `Rust (cpp full): drive X`
     - `Rust (cpp io): drive X`
   - The **Rust (cpp io)** output should closely match the C++ output size
     (differences only due to logging / formatting, not tens of files).
4. Optionally, add lightweight post‑processing scripts that:
   - Count lines / records in each `*.txt` file.
   - Assert that `Rust (cpp io)` and `C++` disagree by at most a very small
     tolerance (ideally 0) on record count.

Once this harness shows that **Rust (cpp io)** matches C++ for multiple drives, the
I/O pipeline port is considered functionally correct. You should then run the full
CI pipeline (`rust-script scripts/ci-pipeline.rs go -v`) and document any issues in
`LOG/<<YYY_MM_DD_HH_MM_>>CHANGELOG_HEALING.md` per the project rules.

---

## Implementation Phases Checklist

### Phase 1: CppDataChunk ⬜ NOT STARTED
- [ ] Create `CppDataChunk` struct with atomic skip ranges
- [ ] Implement `Clone` via atomic loads
- [ ] Add helper methods for effective ranges

### Phase 2: CppBitmapReader ⬜ NOT STARTED
- [ ] Create `CppBitmapReader` struct
- [ ] Implement async IOCP bitmap reading
- [ ] Track chunks_remaining with atomic counter
- [ ] Accumulate valid_records count

### Phase 3: Synchronization Point ⬜ NOT STARTED
- [ ] Implement `on_all_chunks_complete()` callback
- [ ] Recalculate skip ranges for all data chunks
- [ ] Use atomic stores with Release ordering

### Phase 4: Integration ⬜ NOT STARTED
- [ ] Create `CppIoPipeline` orchestrator
- [ ] Wire up to `read_all_sliding_window_iocp_to_index_cpp_port()`
- [ ] Add `--io-pipeline cpp_port` flag

---

## References

- `docs/architecture/C++_resources/UltraFastFileSearch-code/src/io/mft_reader.hpp`
- `docs/architecture/CPP_PARSING_PARITY.md` (parsing algorithm port)
- `docs/architecture/CPP_TREE_ALGORITHM_PORT.md` (tree algorithm port)
- `crates/uffs-mft/src/io.rs` (current IOCP implementation)

