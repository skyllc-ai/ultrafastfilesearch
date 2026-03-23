# Performance & Benchmarking

## Introduction

This document describes the performance characteristics of UFFS, the optimization techniques employed, and how to benchmark and profile the engine. After reading this document, you should be able to:

1. Understand why UFFS is fast and where time is spent
2. Profile and benchmark specific code paths
3. Identify optimization opportunities

---

## Benchmark Results (v0.3.54)

### Full Scan (`*`) — Cold Start, 5 Runs Per Drive

| Drive | Type | Files | Rust avg | C++ avg | Speedup |
|-------|------|-------|----------|---------|---------|
| C: | NVMe | 2.3M | 8.2s | 25.7s | **3.1×** |
| D: | HDD | 1.5M | 30.7s | 64.2s | **2.1×** |
| E: | HDD | 1.8M | 41.1s | 54.4s | **1.3×** |
| F: | NVMe | 1.1M | 5.1s | 15.2s | **3.0×** |
| G: | tiny | 5K | 0.42s | 0.37s | 0.9× |
| M: | HDD | 1.2M | 26.5s | 30.4s | **1.1×** |
| S: | HDD | 3.2M | 71.6s | 90.1s | **1.3×** |
| ALL | parallel | - | 72.3s | 98.6s | **1.36×** |

### Key Observations

- **NVMe drives: ~6-8s for 2M+ files** — Inline parsing and mimalloc shine when I/O is not the bottleneck
- **HDD drives: 30-72s** — I/O-bound, but bitmap skip and IOCP tuning reduce wall-clock time
- **Tiny drives: <0.5s** — Startup overhead dominates for small datasets
- **Multi-drive parallel: ~72s** — Bounded concurrency avoids I/O contention

---

## Optimization Layers

### Layer 1: I/O Optimization

| Technique | Impact | Description |
|-----------|--------|-------------|
| **Direct MFT reading** | ~15× vs FindFirstFile | Bypass file system APIs entirely |
| **Bitmap skip** | 50-80% I/O reduction | Skip deleted records using $MFT::$BITMAP |
| **IOCP async I/O** | Overlaps I/O with CPU | Multiple reads in flight simultaneously |
| **LCN-ordered reads** | 20-30% HDD improvement | Minimize disk seeks by reading in physical order |
| **Drive-type tuning** | 10-50% per drive | NVMe: 32 concurrent reads, 4MB chunks; HDD: 2-6, 1MB |
| **Aligned buffers** | Required for NO_BUFFERING | Sector-aligned allocation avoids extra copies |

### Layer 2: Memory Optimization

| Technique | Impact | Description |
|-----------|--------|-------------|
| **Compact FileRecord** | 224 bytes/record | Bit-packed flags, inline first_name/first_stream |
| **Contiguous names buffer** | Cache-friendly | All names in one `String`, no per-name allocation |
| **Pre-allocated vectors** | Eliminates resizing | Sized from bitmap popcount before parsing starts |
| **Extension interning** | 8 bytes per name ref | 16-bit extension ID instead of string per record |
| **mimalloc** | ~10-15% throughput | Reduces fragmentation for millions of small allocs |
| **NO_ENTRY sentinel** | No Option overhead | `u32::MAX` instead of `Option<u32>` saves 4 bytes |

### Layer 3: Algorithm Optimization

| Technique | Impact | Description |
|-----------|--------|-------------|
| **Extension index** | 50× for *.ext queries | O(matches) instead of O(all) for extension patterns |
| **Inline parsing** | No intermediate copies | Records parsed directly into MftIndex during I/O |
| **Zero-alloc case compare** | Eliminates 8M allocs | Byte-level ASCII comparison instead of `.to_lowercase()` |
| **Leaf-peeling tree metrics** | O(n) no recursion | Array-based Kahn sort instead of recursive DFS |
| **Lazy path resolution** | Only for matched records | Paths computed after all filters applied |
| **Pattern classification** | Optimal matcher per type | `*.txt` → suffix check; `*foo*` → substring; etc. |

### Layer 4: Concurrency Optimization

| Technique | Impact | Description |
|-----------|--------|-------------|
| **IOCP sliding window** | Saturates I/O device | N reads always in flight (N tuned per drive type) |
| **Lock-free hot path** | Zero contention | Single-owner MftIndex during build, no mutexes |
| **Multi-drive parallelism** | Near-linear scaling | Bounded tokio tasks, independent IOCP per drive |
| **Rayon parallel parsing** | NVMe: overlaps CPU/IO | Parse completed buffers on worker threads |
| **Buffer recycling** | Zero allocs after warmup | Completed buffers returned to pool, not freed |

---

## Where Time Is Spent

### NVMe Drive (C:, 2.3M files, 8.2s total)

```
Phase                Time     %    Notes
───────────────────────────────────────────────
Volume open          <1ms   <1%   CreateFile + FSCTL
Metadata collection    5ms   <1%   Volume data + retrieval pointers
Bitmap read           10ms   <1%   ~250KB bitmap
Chunk planning         1ms   <1%   In-memory calculation
IOCP read + parse   6.5s    79%   ★ DOMINANT — parsing is bottleneck
Tree metrics        0.8s    10%   Leaf-peeling O(n)
Extension index     0.3s     4%   Build interned lookup
Stats + finalize    0.5s     6%   Recompute, cleanup
```

On NVMe, **parsing is the bottleneck** (not I/O). The disk can deliver data faster than the CPU can parse it. This is why parallel parsing helps on NVMe.

### HDD Drive (S:, 3.2M files, 71.6s total)

```
Phase                Time     %    Notes
───────────────────────────────────────────────
Volume open          <1ms   <1%
Metadata collection    8ms   <1%
Bitmap read           20ms   <1%
Chunk planning         2ms   <1%
IOCP read + parse  68.0s    95%   ★ DOMINANT — I/O is bottleneck
Tree metrics        1.5s     2%
Extension index     0.8s     1%
Stats + finalize    1.3s     2%
```

On HDD, **I/O is the bottleneck**. The disk head seek time dominates. Bitmap skip and LCN-ordered reads are critical here.

---

## Profiling

### Built-in Profiling (`--profile`)

```bash
uffs * --drive C --profile
```

Outputs detailed timing for each phase:
```
Phase                    Duration
─────────────────────────────────
Volume open              0.3ms
Get volume data          1.8ms
Get retrieval pointers   4.2ms
Read bitmap              8.5ms
Generate chunks          0.9ms
IOCP read + parse        6,423ms
Build child lists        312ms
Tree metrics             845ms
Extension index          287ms
Total                    7,884ms
Records: 2,312,456  Files: 2,089,321  Dirs: 223,135
```

### Benchmark Mode (`--benchmark`)

Skips output formatting to isolate MFT reading performance:

```bash
uffs * --drive C --benchmark
# Only measures: volume open → index built
# No path resolution, no formatting, no stdout I/O
```

### Flamegraph Profiling

```bash
# Build with profiling profile (debug symbols, no LTO)
cargo build --profile profiling

# Use cargo-flamegraph or perf
cargo flamegraph --profile profiling -- uffs * --drive C --benchmark
```

The `profiling` Cargo profile enables debug symbols while keeping optimizations:

```toml
[profile.profiling]
inherits = "release"
debug = true
strip = false
lto = false
codegen-units = 16
```

### Tracing

UFFS uses `tracing` for structured logging. Enable with environment variables:

```bash
RUST_LOG=debug uffs * --drive C    # Info + debug messages
RUST_LOG=trace uffs * --drive C    # Maximum verbosity (very noisy)
```

Key trace points:
- `[TRIP]` markers at function entry/exit for trip-wire debugging
- I/O chunk progress (bytes read, records parsed)
- Drive detection and tuning decisions
- Extension record processing

---

## Memory Usage

### Typical Memory Footprint (2M files)

| Component | Size | Notes |
|-----------|------|-------|
| `records: Vec<FileRecord>` | 448 MB | 2M × 224 bytes |
| `frs_to_idx: Vec<u32>` | 20 MB | 5M × 4 bytes (sparse) |
| `names: String` | 46 MB | 2M × 23 bytes avg |
| `links: Vec<LinkInfo>` | 3 MB | ~125K hardlinks |
| `streams: Vec<IndexStreamInfo>` | 15 MB | ~500K ADS |
| `children: Vec<ChildInfo>` | 42 MB | 3M entries |
| I/O buffers (peak) | 32 MB | 32 × 1MB (NVMe) |
| **Total** | **~600 MB** | |

### Memory Reduction Techniques

1. **Compact types**: 224 bytes per record vs ~400+ bytes with naive layout
2. **Inline first_name/first_stream**: Avoids heap allocation for 95%+ of records
3. **Shared names buffer**: One allocation instead of 2M `String` objects
4. **Extension interning**: 2 bytes per name ref instead of string per extension
5. **NO_ENTRY sentinel**: `u32::MAX` instead of `Option<u32>` (saves 4 bytes × millions)

---

## Why UFFS Is Fast on NVMe

The dominant performance advantages come from architectural decisions in the Rust engine:

1. **Inline parsing**: Records parsed directly into `MftIndex` during IOCP completion — no intermediate copies or staging buffers.
2. **mimalloc**: Purpose-built allocator reduces fragmentation for millions of small objects.
3. **Compact `FileRecord`**: 224 bytes per record with bit-packed flags and inline first-name/first-stream.
4. **Zero-copy NTFS parsing**: `zerocopy` crate reads NTFS headers directly from I/O buffers without memcpy.
5. **Contiguous names buffer**: Single `String` allocation instead of per-name heap objects.

## Known Optimization Targets

### Multi-Drive Filtered Scan (`*.ext`)

Filtered multi-drive parallel scans currently take longer than full scans (78s > 72s for `*.rs` across all drives). Root cause: overhead in the multi-drive streaming writer and per-drive extension index construction. This is a documented optimization target.

---

## Cargo Build Profiles

| Profile | Use Case | LTO | Debug | Opt |
|---------|----------|-----|-------|-----|
| `dev` | Development | No | Full | 0 |
| `debug-optimized` | Dev with speed | No | Full | 2 |
| `release` | Production | Fat | No | 3 |
| `profiling` | Flamegraphs | No | Full | 3 |
| `bench` | Benchmarks | Thin | No | 3 |
| `dist` | Distribution | Thin | No | 3 |
| `xwin-dev` | Cross-compile dev | No | Full | 0 |

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
