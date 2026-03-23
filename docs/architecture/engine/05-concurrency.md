# Concurrency Model

## Introduction

UFFS uses a layered concurrency model that combines async I/O (tokio), thread-level parallelism (rayon), and Windows I/O Completion Ports (IOCP) to maximize throughput across diverse storage types. After reading this document, you should be able to:

1. Understand every concurrency primitive used in the pipeline
2. Reason about thread safety of any code path
3. Extend the codebase without introducing race conditions
4. Debug concurrency issues effectively

---

## Threading Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     UFFS Thread Architecture                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌──────────────────┐                                                    │
│  │  Tokio Runtime    │  - Multi-threaded runtime (rt-multi-thread)        │
│  │  (main + workers) │  - Drives async orchestration                     │
│  │                    │  - spawn_blocking for CPU-bound MFT reads         │
│  └────────┬───────────┘                                                  │
│           │                                                              │
│           │ spawn_blocking (per drive)                                    │
│           ▼                                                              │
│  ┌──────────────────┐                                                    │
│  │  Blocking Thread  │  - One per drive being read                       │
│  │  (MFT Reader)     │  - Owns VolumeHandle, IOCP, MftIndex             │
│  │                    │  - Runs IOCP event loop                          │
│  └────────┬───────────┘                                                  │
│           │                                                              │
│           │ IOCP completions                                             │
│           ▼                                                              │
│  ┌──────────────────┐                                                    │
│  │  IOCP I/O Thread  │  - Managed by Windows kernel                     │
│  │  (per volume)      │  - Issues ReadFile, receives completions         │
│  │                    │  - N reads in flight (sliding window)            │
│  └────────┬───────────┘                                                  │
│           │                                                              │
│           │ (optional, NVMe only)                                        │
│           ▼                                                              │
│  ┌──────────────────┐                                                    │
│  │  Rayon Thread Pool│  - For parallel parsing (NVMe drives)             │
│  │  (N CPU cores)    │  - Shared global pool                             │
│  │                    │  - Processes completed buffers in parallel        │
│  └──────────────────┘                                                    │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Layer 1: Tokio Async Runtime

### Role

Tokio provides the top-level async orchestration. It manages:
- Multi-drive parallel reading (one task per drive)
- Bounded concurrency via semaphores
- Task spawning and cancellation

### Configuration

```rust
// From main.rs — runtime setup
#[tokio::main]
async fn main() {
    // Uses rt-multi-thread with default worker count
    // (number of CPU cores)
}
```

### Key Pattern: spawn_blocking

MFT reading is CPU-bound (parsing) and uses blocking Windows APIs (IOCP). It runs on `spawn_blocking` threads to avoid starving the tokio executor:

```rust
// reader/index_read.rs
pub async fn read_all_index_live(&self) -> Result<MftIndex> {
    let volume = self.volume;
    let mode = self.mode;
    // ... capture all config ...

    tokio::task::spawn_blocking(move || {
        let handle = VolumeHandle::open(volume)?;
        let reader = MftReader { volume, source: LiveVolume(handle), mode, ... };
        reader.read_mft_index_internal(None::<fn(MftProgress)>)
    })
    .await
    .map_err(|e| MftError::from_join_error("read_all_index", &e))?
}
```

**Why `spawn_blocking`?** The IOCP event loop calls `GetQueuedCompletionStatus` which blocks the calling thread. Running this on a tokio worker thread would block other async tasks.

---

## Layer 2: Windows IOCP (I/O Completion Ports)

### Role

IOCP is the core I/O mechanism. It provides:
- Multiple overlapped (async) reads in flight simultaneously
- Kernel-managed completion queue
- Zero-copy notification of completed reads

### Architecture

```
┌───────────────────────────────────────────────────────────┐
│                   IOCP Event Loop                          │
│                                                            │
│  ISSUE READS:                                              │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐                    │
│  │ReadFile  │  │ReadFile  │  │ReadFile  │  (N concurrent)   │
│  │buf[0]    │  │buf[1]    │  │buf[2]    │                   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘                  │
│       │              │              │                        │
│       └──────────────┴──────────────┘                       │
│                      │ Windows kernel queues completions     │
│                      ▼                                       │
│  WAIT:  GetQueuedCompletionStatus(iocp, INFINITE)           │
│                      │                                       │
│                      ▼                                       │
│  PROCESS:                                                    │
│  ┌──────────────────────────────────────────────────┐       │
│  │ 1. Recover OverlappedRead from OVERLAPPED*       │       │
│  │ 2. Parse each 1KB record in buffer               │       │
│  │ 3. Build into MftIndex directly (inline parse)   │       │
│  │ 4. Return buffer to pool                         │       │
│  │ 5. Issue next ReadFile (maintain N in-flight)    │       │
│  └──────────────────────────────────────────────────┘       │
│                                                              │
│  REPEAT until all chunks processed                           │
└───────────────────────────────────────────────────────────────┘
```

### Thread Safety

The IOCP event loop runs on a **single blocking thread** per volume. There is no shared mutable state between volumes:

| Resource | Owned By | Thread Safety |
|----------|----------|---------------|
| `VolumeHandle` | Blocking thread | Single-owner (no sharing) |
| `IoCompletionPort` | Blocking thread | Single-owner |
| `MftIndex` | Blocking thread | Single-owner during build |
| `AlignedBuffer` pool | Blocking thread | Single-owner |
| `OverlappedRead` | Pinned, single owner | Not shared |

**No locks required** during the MFT read loop — everything is owned by the single blocking thread.

### Concurrency Tuning

The sliding window size (reads in flight) is tuned per drive type:

```rust
// Number of concurrent ReadFile operations
match drive_type {
    Nvme    => 32,  // NVMe queue depth: saturate the device
    Ssd     => 8,   // Moderate parallelism
    Hdd     => 2-6, // More seeks = slower; extent-aware tuning
    Unknown => 4,   // Conservative default
}
```

---

## Layer 3: Rayon (Parallel Parsing)

### Role

For NVMe drives, I/O completes faster than parsing. Rayon's work-stealing thread pool parallelizes record parsing across CPU cores.

### When Used

```rust
impl DriveType {
    pub fn benefits_from_parallel_parsing(&self) -> bool {
        matches!(self, Nvme)  // Only NVMe
    }
}
```

### Pipelined Parallel Architecture

**Source:** `io/readers/pipelined.rs`

```
┌──────────────────────────────────────────────────────────────┐
│                 Pipelined Parallel Reader                      │
│                                                                │
│  I/O Thread (single):                                          │
│  ┌──────────────────────────────────────────────────────┐     │
│  │ Read chunk 0 → Read chunk 1 → Read chunk 2 → ...     │     │
│  │                    │                                   │     │
│  │                    ▼ (crossbeam channel)                │     │
│  └──────────────────────────────────────────────────────┘     │
│                       │                                        │
│                       ▼                                        │
│  Parse Workers (rayon pool, N cores):                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                     │
│  │ Worker 0 │  │ Worker 1 │  │ Worker 2 │  ...                 │
│  │ Parse    │  │ Parse    │  │ Parse    │                      │
│  │ chunk 0  │  │ chunk 1  │  │ chunk 2  │                      │
│  └──────────┘  └──────────┘  └──────────┘                     │
│       │              │              │                           │
│       └──────────────┴──────────────┘                          │
│                      │ (results channel)                       │
│                      ▼                                         │
│  Merge Thread:                                                 │
│  ┌──────────────────────────────────────────────────────┐     │
│  │ Collect MftIndexFragments → merge_fragments()         │     │
│  └──────────────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────────────┘
```

### Rayon Configuration

```rust
// Global rayon pool — shared across all drives
rayon::ThreadPoolBuilder::new()
    .num_threads(num_cpus::get())
    .build_global()
    .ok();

// Usage in parallel reader:
chunks.par_iter()
    .map(|chunk| parse_chunk_to_fragment(chunk))
    .collect::<Vec<MftIndexFragment>>()
```

---

## Layer 4: Multi-Drive Orchestration

### Bounded Parallelism

Multiple drives are read in parallel, but bounded to prevent system overload:

```rust
// reader/multi_drive/mod.rs
const MAX_CONCURRENT_DRIVE_READERS: usize = 4;

fn drive_reader_budget(total_drives: usize) -> usize {
    let hw = available_parallelism();
    total_drives.min(hw).min(MAX_CONCURRENT_DRIVE_READERS)
}
```

### Tokio Semaphore Pattern

```rust
// Multi-drive index reading
pub async fn read_all_index(&self) -> Result<Vec<MftIndex>> {
    let semaphore = Arc::new(Semaphore::new(drive_reader_budget(self.drives.len())));

    let tasks: Vec<_> = self.drives.iter().map(|&drive| {
        let sem = semaphore.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire().await;
            MftReader::open(drive)?.read_all_index().await
        })
    }).collect();

    // Join all tasks
    let results = futures::future::join_all(tasks).await;
    // ... collect results ...
}
```

### Drive Isolation

Each drive gets its own:
- `VolumeHandle` (separate file descriptor)
- `IoCompletionPort` (separate completion queue)
- `MftIndex` (separate index)
- I/O tuning parameters (based on detected drive type)

There is **zero shared mutable state** between drive readers.

---

## Synchronization Primitives Used

| Primitive | Location | Purpose |
|-----------|----------|---------|
| `tokio::spawn_blocking` | Reader entry point | CPU-bound work off async runtime |
| `tokio::task::spawn` | Multi-drive | Per-drive async tasks |
| `tokio::sync::Semaphore` | Multi-drive | Bound concurrent drives |
| `rayon::par_iter` | Parallel parsing | Multi-core parsing on NVMe |
| Windows IOCP | I/O reader | Async disk reads |
| `Pin<Box<OverlappedRead>>` | IOCP | Pinned memory for overlapped I/O |
| `Arc<Mutex<>>` | (not used!) | No mutexes in the hot path |

### Lock-Free Hot Path

The critical insight: the IOCP read+parse loop is **entirely lock-free**:

1. Single thread owns the `MftIndex` during building
2. No shared state between volumes
3. Buffers are owned, not shared
4. IOCP completions arrive on the calling thread

This eliminates all synchronization overhead from the performance-critical path.

---

## Error Handling in Concurrent Code

### Task Failure Isolation

If one drive fails, others continue:

```rust
let results: Vec<Result<MftIndex>> = join_all(tasks).await;
for result in results {
    match result {
        Ok(index) => indices.push(index),
        Err(e) => {
            warn!("Drive {} failed: {}", drive, e);
            // Continue with other drives
        }
    }
}
```

### Cancellation

UFFS supports graceful cancellation:
- Tokio tasks can be dropped (reads in flight will complete and be ignored)
- IOCP loop checks a cancellation flag between completions
- `spawn_blocking` tasks are awaited with timeout

### Panic Recovery

```rust
tokio::task::spawn_blocking(move || {
    // ... MFT reading ...
})
.await
.map_err(|error| MftError::from_join_error("read_all_index", &error))
// JoinError indicates panic in the blocking task
```

---

## Memory Model

### Ownership Flow

```
tokio task (async)
  └─► spawn_blocking thread
       ├── VolumeHandle (owned, dropped on completion)
       ├── IoCompletionPort (owned, RAII drop closes handle)
       ├── MftIndex (owned, moved out as return value)
       └── Buffer pool (owned, recycled during read loop)
            └── OverlappedRead (pinned, recycled)
```

### No `Arc`/`Mutex` in Index Building

The `MftIndex` is built by a single thread and then **moved** (not shared) to the caller. This is the key design: ownership transfer instead of shared access.

```rust
// Build phase: single-owner
let mut index = MftIndex::new('C');
// ... populate index on blocking thread ...

// Transfer phase: move to caller
Ok(index)  // Moves out of spawn_blocking
```

After building, the `MftIndex` is immutable (no more writes). It can be shared via `&MftIndex` for concurrent reads during search.

---

## Performance Characteristics

### Thread Counts (Typical 8-Core System)

| Component | Threads | Notes |
|-----------|---------|-------|
| Tokio workers | 8 | One per CPU core |
| Blocking threads | 1-4 | One per drive being read |
| Rayon pool | 8 | Shared, for NVMe parallel parse |
| IOCP kernel threads | 1+ | Managed by Windows |
| **Total** | ~12-20 | During multi-drive scan |

### Contention Analysis

| Resource | Contention Risk | Mitigation |
|----------|----------------|------------|
| CPU cores | Medium (many threads) | Rayon work-stealing balances load |
| Disk I/O | Low (per-volume IOCP) | Each drive has independent I/O |
| Memory allocator | Low | mimalloc handles multi-threaded alloc |
| MftIndex | None | Single-owner during build |

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
