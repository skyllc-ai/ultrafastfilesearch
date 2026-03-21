# Inline IOCP Output Architecture: C++ vs Rust

## Overview

This document compares how C++ and Rust handle the MFT scan → output pipeline,
and proposes an architecture for Rust to write output rows **during** IOCP disk
reads, eliminating the sequential post-I/O output phase.

## Current Architectures

### C++ Pipeline (ntfs_index_load.hpp + cli_main.hpp)

```
┌─────────────────────────────────────────────────────────────────┐
│  IOCP Completion Loop (per 1MB chunk)                           │
│                                                                 │
│  1. GetQueuedCompletionStatus()  ← wait for disk chunk          │
│  2. load() → parse MFT records into NtfsIndex                  │
│  3. Requeue next read                                           │
│  └── (repeat until all chunks read)                             │
│                                                                 │
│  After ALL chunks:                                              │
│  4. preprocessor() from root FRS 5  ← tree metrics (DFS)       │
│  5. Match + format + write output   ← Writer class              │
└─────────────────────────────────────────────────────────────────┘
```

**Key C++ detail**: The Writer class (cli_main.hpp lines 596-654) uses:
- A `line_buffer` (std::string) that accumulates formatted rows
- Flushes to WriteFile when buffer ≥ 32KB or timer triggers
- Uses raw Win32 `WriteFile` on the stdout HANDLE (not printf)
- For non-console: writes directly to the output file HANDLE

**C++ does NOT write output during IOCP reads.** It writes output AFTER
`preprocessor()` completes — same sequential architecture as Rust.

### Rust Pipeline (current, v0.3.51+)

```
┌─────────────────────────────────────────────────────────────────┐
│  Single-drive:                                                  │
│  1. IOCP sliding window + inline process_record() → MftIndex   │
│  2. compute_tree_metrics()                                      │
│  3. build_extension_index()                                     │
│  4. write_index_streaming() → BufWriter(1MB) → stdout/file     │
│                                                                 │
│  Multi-drive:                                                   │
│  1. Spawn tokio tasks (parallel IOCP reads per drive)           │
│  2. As each drive's index completes: stream rows to shared      │
│     BufWriter (write_index_streaming_no_header)                 │
│  3. Other drives continue IOCP reads while output writes        │
└─────────────────────────────────────────────────────────────────┘
```

## The Proposed "Inline Output" Architecture

### Goal

Write output rows **during** the IOCP completion loop, overlapping disk I/O
with output formatting and writing. This would eliminate the separate output
phase entirely.

### Why It's Hard

Tree metrics (`descendants`, `treesize`, `tree_allocated`) require the FULL
directory tree to be computed. A directory at depth 1 needs to know the total
size of ALL its descendants — which aren't known until every record is parsed.

| Record Type | Can Output During IOCP? | Reason |
|---|---|---|
| **Files** | ✅ Partially | `descendants=0`, `treesize=own_size` — no tree dependency |
| **Files (path)** | ⚠️ Maybe | Path needs parent chain to be parsed. Parents arrive before children in FRS order on sequential disk. |
| **Directories** | ❌ No | `descendants`, `treesize`, `tree_allocated` need full tree |

### Proposed Two-Phase Inline Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Phase 1: IOCP Read + Parse + Inline File Output                │
│                                                                 │
│  For each IOCP completion:                                      │
│    parse records → MftIndex                                     │
│    for each FILE record just parsed:                            │
│      if parent already in index (path resolvable):              │
│        format row → write to output buffer                      │
│      else:                                                      │
│        defer to Phase 2                                         │
│    requeue next disk read                                       │
│                                                                 │
│  Phase 2: Tree Metrics + Directory Output                       │
│                                                                 │
│  compute_tree_metrics()                                         │
│  for each DIRECTORY record:                                     │
│    format row with tree metrics → write to output               │
│  for each deferred FILE record:                                 │
│    format row → write to output                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Challenges and Solutions

#### 1. Path Resolution During IOCP

**Problem**: To write a file's row, we need its full path (`D:\foo\bar\file.txt`).
This requires walking the parent chain, which needs parent records to already
be in the index.

**Solution**: On HDD with sequential FRS order, parent directories (lower FRS)
are parsed before child files (higher FRS). So for ~90% of files, the parent
chain is already available when the file is parsed.

For the ~10% where the parent isn't ready (extension records, out-of-order
chunks), defer to Phase 2.

#### 2. Tree Metrics for Files

**Problem**: Files have `descendants=0` and `treesize=own_size`. But the output
format includes these columns — do they need tree metrics?

**Solution**: No! File tree metrics are trivial:
- `descendants = 0`
- `treesize = first_stream.size.length`
- `tree_allocated = first_stream.size.allocated`

These are known immediately after parsing, no tree computation needed.

#### 3. Output Ordering

**Problem**: Writing files inline during IOCP produces output in FRS order
(disk order), not the tree-walk order that the current streaming writer uses.

**Solution**: The verify_parity script already does sorted comparison. C++ and
Rust already produce different row orders. Row order doesn't affect correctness.

#### 4. Shared Writer Between IOCP Thread and Output

**Problem**: The IOCP completion handler runs on a single thread. Writing output
from that thread adds latency between IOCP completions, potentially stalling
the disk read pipeline.

**Solution**: Use a channel-based design:
```
IOCP thread → format row → send to channel → writer thread drains channel → BufWriter
```
The IOCP thread never blocks on I/O. The writer thread runs independently,
flushing the BufWriter as rows arrive. This keeps the IOCP sliding window
at maximum throughput.

#### 5. Multi-Drive Inline Output

**Problem**: Multiple drives each have their own IOCP completion thread.

**Solution**: All drives send formatted rows to the SAME channel. The single
writer thread drains the channel and writes to BufWriter. Rows from different
drives interleave naturally.

```
Drive D IOCP thread ──┐
Drive F IOCP thread ──┼──→ mpsc channel ──→ Writer thread ──→ BufWriter ──→ stdout
Drive S IOCP thread ──┘
```

### Implementation Plan

#### Step 1: Add `OutputCallback` to `process_record`

```rust
// In to_index.rs IOCP completion handler:
for each record in chunk {
    process_record(data, frs, &mut index, &mut name_buf);
    
    // NEW: if file record and parent is known, format + send
    if let Some(record) = index.records.last() {
        if !record.is_directory() {
            if let Some(path) = try_resolve_path(&index, record) {
                let row = format_row(record, &path, &index);
                output_tx.send(row)?;
            }
        }
    }
}
```

#### Step 2: Writer Thread

```rust
// Spawned before IOCP loop starts:
let (tx, rx) = std::sync::mpsc::channel::<String>();
let writer_handle = std::thread::spawn(move || {
    let mut writer = BufWriter::with_capacity(1024 * 1024, stdout.lock());
    for row in rx {
        writer.write_all(row.as_bytes()).unwrap();
    }
    writer.flush().unwrap();
});
```

#### Step 3: Phase 2 for Directories + Deferred Files

```rust
// After IOCP loop completes:
drop(tx); // Signal no more Phase 1 rows
compute_tree_metrics(&mut index);

// Write directory rows and any deferred files
let (tx2, rx2) = channel();
// ... format directory rows with tree metrics ...
// Writer thread processes Phase 2 rows
```

### Expected Performance Impact

#### Single Drive D (HDD, 4.8M records, 7M output rows)

| Phase | Current | Inline | Savings |
|---|---|---|---|
| IOCP + parse | 22s | 22s | — |
| Output (files) | 8.4s | 0s (overlapped) | -8.4s |
| Tree metrics | 0.3s | 0.3s | — |
| Output (dirs) | ~1s | ~1s | — |
| **Total** | **32.7s** | **~24s** | **~27% faster** |

#### Multi-Drive (D + F + S parallel)

| | Current | Inline |
|---|---|---|
| D total | 32.7s | ~24s |
| F total | 5.7s | ~3s |
| S total | 73s | ~63s |
| **Wall time** | ~73s (parallel) | **~63s** |

### Risks and Tradeoffs

1. **Complexity**: Adds channel-based concurrency, output ordering concerns,
   deferred record tracking. Significantly more complex than current design.

2. **Correctness**: Must ensure no rows are missed (deferred files must be
   tracked and output in Phase 2).

3. **Memory**: Formatted row strings in the channel consume memory. For 7M
   rows at ~200 bytes each, that's ~1.4GB if the channel fills up. Need
   bounded channel with backpressure.

4. **Debugging**: Interleaved output from multiple threads is harder to debug.

5. **Diminishing returns**: Rust is already 2-3× faster than C++. The inline
   approach saves ~8s on D (27% of 32.7s) and ~10s on S (14% of 73s). The
   HDD read time dominates and can't be optimized.

### Comparison with C++ Architecture

| Aspect | C++ | Rust (current) | Rust (proposed inline) |
|---|---|---|---|
| IOCP read | Sequential chunks | Same | Same |
| Parse during IOCP | ✅ load() inline | ✅ process_record() inline | ✅ Same |
| Tree metrics | After all chunks | After all chunks | After all chunks |
| File output | After tree metrics | After tree metrics | **During IOCP** |
| Dir output | After tree metrics | After tree metrics | After tree metrics |
| Output buffering | 32KB WriteFile | 1MB BufWriter | 1MB BufWriter + channel |
| Multi-drive | Not parallel | Parallel load + sequential output | Parallel load + parallel output |

**Key insight**: C++ does NOT output during IOCP either. Both architectures
are sequential: read → tree → output. The inline approach would be a
genuinely novel optimization that neither C++ nor current Rust implements.

### Recommendation

**Ship the current multi-drive parallel streaming first** (parallel loads,
sequential output per drive as completed). This already gives near-optimal
wall time for multi-drive scans.

The inline IOCP output is a **Phase 2 optimization** worth pursuing when:
- Single-drive performance needs to exceed C++ by more than 2-3×
- The 8-10s output phase becomes the dominant bottleneck
- The architecture complexity is justified by production requirements

Current performance is already excellent:
- D: 32.7s vs C++ 69.3s (2.1×)
- F: 5.7s vs C++ 17.2s (3.0×)
- S: 73s vs C++ 96s (1.3×)
