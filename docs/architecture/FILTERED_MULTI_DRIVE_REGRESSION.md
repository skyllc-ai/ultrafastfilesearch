# Filtered Multi-Drive Regression — Deep Dive

## The Problem

```
*.rs ALL parallel:  Rust 78.5s  vs  C++ 41s   (C++ 1.9× faster)
*    ALL parallel:  Rust 72.3s  vs  C++ 98s    (Rust 1.36× faster)
```

Rust's filtered multi-drive is **6s slower than its own full scan**, which is
logically impossible — fewer results should mean less work.

---

## Architecture Trace

Both `*` and `*.rs` multi-drive go through the **same** function:

```
dispatch_windows_live()
  → run_live_multi_drive_streaming()
      ┌─ ASYNC LOOP (tokio) ──────────────────────────────────┐
      │  for each drive:                                       │
      │    join_set.spawn(load_live_index(drive, no_cache))    │
      │      → read_index_cached() → read_and_cache_index()   │
      │        → read_all_index() [IOCP, spawn_blocking]      │
      │        → save_to_cache()  [serialize + disk write]     │
      └────────────────────────────────────────────────────────┘
                        │
                  sync_channel(2)   ← MftIndex moved through channel
                        │
      ┌─ WRITER THREAD (OS thread) ──────────────────────────────┐
      │  for each (drive, index) from rx:                         │
      │    PathCache::build(index)      ← O(n) ALWAYS             │
      │      → PathResolver::build()    ← vec![UNSEEN; n]         │
      │      → propagate_invalid()      ← O(n) tree walk          │
      │      → validate_remaining()     ← O(n) scan               │
      │      → pre_cache_dir_paths()    ← Vec<String> for ALL n   │
      │    write_index_streaming_with_filter()                    │
      │      → iterate record_indices (ext index) or all records  │
      │      → pattern match + format + write                    │
      └──────────────────────────────────────────────────────────┘
```

### What differs between `*` and `*.rs`

| Step | `*` (full scan) | `*.rs` (filtered) |
|------|-----------------|-------------------|
| Loading (IOCP) | Identical | Identical |
| save_to_cache | Identical | Identical |
| Channel send | Identical | Identical |
| **PathCache::build** | **O(n) per drive** | **O(n) per drive** |
| Output iteration | All records | Extension index only |
| Output volume | Millions of lines | Hundreds of lines |

The loading is **100% identical**. The only difference is in the writer thread.

---

## Root Cause 1: PathCache::build is O(n) Regardless of Result Count

`write_index_streaming_with_filter` (output.rs:633-636) unconditionally builds
a full PathCache for every drive:

```rust
let path_cache = PathCache::build(index, false);   // O(n) ALWAYS
let resolver = path_cache.resolver();
let dir_cache = path_cache.dir_cache();
```

`PathCache::build` does three expensive O(n) passes:

1. **PathResolver::build** — `vec![path_state::UNSEEN; n]` + system metafile
   marking + `propagate_invalid_to_descendants` + `validate_remaining`
2. **pre_cache_directory_paths** — `vec![String::new(); n]` + materializes a
   full path string for every valid directory

For S drive (~10M records), this allocates ~40 MB for the state vector, ~80 MB+
for the dir_cache Vec<String>, and does millions of string allocations for
directory paths. Estimated cost: **3–5 seconds per large drive**.

For `*`, this is amortized: the output phase also takes seconds (streaming
millions of lines), so PathCache::build overlaps with other drives loading.

For `*.rs`, PathCache::build **IS** the output phase — the actual streaming
takes milliseconds. Every second spent in PathCache::build is a second on the
critical path.

---

## Root Cause 2: Writer Thread Serializes PathCache Builds

The writer thread processes drives **sequentially**:

```rust
for (drive, index, load_ms) in rx {
    total_rows += stream_drive(&index, &mut buf_writer)?;  // PathCache inside
}
```

For `*` timeline (writer busy = load overlap):
```
Writer: [G:PathCache+Output 0.5s][F:PathCache+Output 2s][C:PathCache+Output 5s]...
Loader:  ──────────────── D,M,E,S still loading in background ──────────────────
                                                               S finishes at 63s
Writer: ...[D 4s][M 3s][E 5s]...........................[S: PathCache 5s + Output 4s]
                                                         Done at ~72s ✅
```

For `*.rs` timeline (writer idle between drives = NO overlap):
```
Writer: [G:PC 20ms][idle 4.6s][F:PC 200ms][idle 2.8s][C:PC 2s][idle 14s][D:PC 2s]...
Loader:  ────────────── drives complete at 0.4, 5, 8, 24, 24.5, 38, 63 ──────────
                                                               S finishes at 63s
Writer: ...[M:PC 1.5s][idle 10s][E:PC 2.5s][idle 22s][S: PathCache 5s]
                                                       Done at ~68s
```

But this gives ~68s, not 78.5s. The extra ~10s comes from:

---

## Root Cause 3: save_to_cache() Blocks Tokio Workers

`read_and_cache_index()` is an async function that calls `save_to_cache()`
**synchronously** after `read_all_index().await` completes:

```rust
async fn read_and_cache_index(&self) -> Result<MftIndex> {
    let index = self.read_all_index().await?;  // spawn_blocking inside

    // These run ON THE TOKIO WORKER THREAD (not spawn_blocking!):
    let handle = VolumeHandle::open(drive)?;          // syscall
    let (usn_journal_id, next_usn) = query_usn_journal(drive)?;  // syscall
    save_to_cache(&index, drive, ...)?;               // serialize + 100MB+ write!

    Ok(index)
}
```

After `read_all_index().await` completes (the spawn_blocking task returns), the
async task resumes on a **tokio worker thread** and runs `save_to_cache()`
synchronously. This:

1. **Serializes** the entire MftIndex (CPU-bound, 100-200MB per drive)
2. **Writes** the serialized data to `%TEMP%\uffs_index_cache\` (disk I/O)

With 7 drives finishing in roughly the same time window, multiple
`save_to_cache()` calls compete for:
- Tokio worker threads (default: num_cpus, typically 8-16)
- Disk I/O bandwidth (all writing to C: temp directory)
- CPU for serialization

For `*`: The writer thread is busy, so `join_set.join_next()` isn't polled as
aggressively. The cache saves complete before the writer needs the next index.
The `sync_channel(2)` acts as **natural backpressure** — even if a cache save
is slow, the writer is occupied with output.

For `*.rs`: The writer thread is fast, so `join_set.join_next()` is polled
immediately after each drive is sent. The async loop is bottlenecked by cache
saves completing on tokio workers. Each cache save delays the next
`join_set.join_next()` return.

---

## Root Cause 4: C++ Uses Lazy Per-Record Path Resolution (No PathCache)

C++ `*.rs` ALL = 41s, but C++ `*.rs` S individual = 64s. Investigation of the
C++ source (`_trash/UltraFastFileSearch-code/src/cli/cli_main.hpp`) reveals:

1. **No disk cache** — C++ creates fresh `NtfsIndex` objects via IOCP every run.
   There is no `save_to_cache` / `load_from_cache` equivalent.
2. **Lazy path resolution** — C++ calls `i->get_path(key, temp, false)` per
   matched record on demand (line ~750). No equivalent of Rust's
   `PathCache::build` that pre-computes ALL directory paths up front.
3. **OS filesystem cache** — the anomalous C++ ALL < C++ S individual timing is
   because the OS keeps MFT data in RAM from prior individual-drive benchmarks.
   Both Rust and C++ benefit equally from this.

The C++ architecture is fundamentally different for filtered queries:
- C++ outputs each matched record immediately with on-demand path resolution
- Rust builds a full PathCache (O(n) over ALL records) before outputting anything

This is exactly what **Fix A** addresses — skip `pre_cache_directory_paths` for
filtered queries with few matches, making Rust's path resolution lazy like C++.

---

## Fix Plan

### Fix A: Skip PathCache for Extension-Index Queries (HIGH impact, ~-5s)

When `record_indices` is `Some` (extension index provides matching records),
build a **lazy PathResolver** without pre_cache_directory_paths. Resolve paths
on-demand only for matching records.

```rust
// In write_index_streaming_with_filter:
let (resolver, dir_cache) = if record_indices.is_some() && record_indices.map_or(false, |r| r.len() < 100_000) {
    // Filtered query with few matches: skip expensive dir path pre-cache
    let resolver = PathResolver::build(index, false);
    (resolver, Vec::new())  // empty dir_cache, paths resolved on demand
} else {
    // Full scan or many matches: pre-cache for speed
    let path_cache = PathCache::build(index, false);
    (path_cache.resolver().clone(), path_cache.dir_cache().to_vec())
};
```

This avoids the ~500K directory path materializations when only a handful of
`.rs` files need paths.

### Fix B: Move save_to_cache into spawn_blocking (MEDIUM impact, ~-3s)

```rust
async fn read_and_cache_index(&self) -> Result<MftIndex> {
    let drive = self.volume;
    let index = self.read_all_index().await?;

    // Save to cache on a blocking thread, not on tokio worker
    let index_clone_for_cache = index.clone();  // or use Arc
    tokio::task::spawn_blocking(move || {
        let handle = VolumeHandle::open(drive).ok();
        if let Some(handle) = handle {
            let vd = handle.volume_data();
            let (jid, nusn) = query_usn_journal(drive).unwrap_or((0, 0));
            let _ = save_to_cache(&index_clone_for_cache, drive, vd.volume_serial_number, jid, nusn);
        }
    });
    // Don't await — fire-and-forget cache save

    Ok(index)
}
```

This frees tokio workers immediately after loading, letting `join_set.join_next()`
return faster. The cache save happens asynchronously in the background.

**Caveat**: Cloning MftIndex is expensive. Alternative: use `Arc<MftIndex>` so
the cache save and the channel send share the same data.

### Fix C: Use tokio channel instead of sync_channel (LOW impact)

Replace `std::sync::mpsc::sync_channel` with `tokio::sync::mpsc::channel` and
`.await` the send. This avoids blocking the tokio worker thread when the channel
is full.

```rust
let (tx, mut rx) = tokio::sync::mpsc::channel::<(char, MftIndex, u128)>(2);

// Sender:
while let Some(result) = join_set.join_next().await {
    if let Ok(Ok(tuple)) = result {
        tx.send(tuple).await.ok();  // async, doesn't block worker
    }
}
```

The writer thread would need to be refactored to receive from a tokio channel,
or use `blocking_recv()`.

### Fix D: Identify and clear C++ cache in benchmarks (CORRECTNESS)

Find where `uffs.com` stores its cache and clear it alongside the Rust cache in
the benchmark script. Without this, any Rust-vs-C++ comparison for filtered
queries is invalid.

---

## Expected Impact

| Fix | Estimated improvement for `*.rs` ALL |
|-----|--------------------------------------|
| A: Lazy PathCache | **-5 to -8s** (skip pre_cache_dir_paths for 7 drives) |
| B: Background cache save | **-2 to -4s** (unblock tokio workers) |
| C: Tokio channel | **-0 to -1s** (minor, sync_channel rarely blocks for `*.rs`) |
| D: Fair C++ benchmark | N/A (reveals C++ true cold-start time) |

**Combined A+B**: `*.rs` ALL should drop from 78.5s to ~66-68s (in line with
S single-drive at 63.7s + small overhead).

---

## Summary

The regression has **three technical causes** and one **benchmark artifact**:

1. **PathCache::build O(n)** — builds full directory path cache for ALL records
   on every drive, even when only a few files match. This is the dominant cost
   for filtered queries (~5s per large drive).
2. **save_to_cache blocks tokio** — serialization + disk write runs on tokio
   worker threads instead of the blocking pool, starving the async event loop.
3. **Writer serialization** — the single writer thread processes drives
   sequentially; for filtered queries where output is instant, PathCache::build
   becomes the serial bottleneck.
4. **C++ cache not cleared** — benchmark gives C++ an unfair advantage by only
   clearing the Rust cache directory.
