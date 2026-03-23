# Performance Deep Dive

## Introduction

This document captures the engineering decisions that make UFFS fast, the real-world benchmark data that validates them, and the evolution from initial prototype to production performance. This is the "secret sauce" document — every optimization is explained with its measured impact.

---

## The Optimization Journey

UFFS performance evolved through three major phases:

| Phase | Version | 7-Drive Total | Key Wins |
|-------|---------|---------------|----------|
| **Baseline** | v0.1.30 | 315s | Working prototype, no tuning |
| **Phase 1** | v0.1.39 | 142s (55% faster) | SoA layout, extension skip, I/O overlap |
| **Phase 2** | v0.2.66 | ~100s | Adaptive concurrency, NVMe tuning |
| **Production** | v0.3.54 | 72s (77% faster than baseline) | IOCP inline parsing, streaming output |

---

## Real-World Benchmarks

### Test Environment

**System**: MASTER-PC — 24 CPU cores
**Drives**: 7 NTFS volumes (2× NVMe, 5× HDD/USB), mix of sizes
**Protocol**: 5 rounds per drive, cold start, anomalous first-run outliers excluded
**Binary**: v0.3.54 release build (`--profile release`, LTO=fat, codegen-units=1)

### Full Scan (`*`) — v0.3.54

| Drive | Type | Est. MFT Records | MFT Size | Rust | Speedup vs C++ |
|-------|------|-------------------|----------|------|----------------|
| C: | NVMe (990 PRO) | ~5M | 4.5 GB | **8.3s** | 3.0× |
| D: | HDD (7200 RPM) | ~5M | 4.8 GB | **30.9s** | 2.1× |
| E: | HDD/USB | ~6M | 2.9 GB | **41.1s** | 1.3× |
| F: | NVMe (980 PRO) | ~2M | 4.5 GB | **5.2s** | 3.0× |
| G: | USB (small) | ~50K | 44 MB | **0.42s** | 0.9× |
| M: | HDD/NAS | ~4M | 2.4 GB | **26.5s** | 1.1× |
| S: | HDD (7200 RPM) | ~10M+ | 11.5 GB | **71.8s** | 1.3× |
| **ALL** | **parallel** | **~32M** | **~30 GB** | **72.3s** | **1.36×** |

**Key observations:**
- **NVMe: 3× faster** — Inline parsing and mimalloc dominate when I/O is not the bottleneck
- **HDD: 1.3–2.1×** — I/O-bound, but bitmap skip and IOCP tuning still help
- **ALL parallel ≈ slowest single drive** — Multi-drive parallelism works; total limited by S: (72s)
- **Consistency**: < 1% variance across 5 rounds on most drives

### Filtered Scan (`*.rs`) — v0.3.54

| Drive | Rust | Speedup vs C++ | Notes |
|-------|------|----------------|-------|
| C: | 7.8–13.8s | 1.0–1.8× | Variance due to OS MFT cache |
| D: | 24.2s | 2.1× | |
| F: | 2.9–7.6s | 1.1–2.9× | Cache-warm: 2.9s; cold: 7.6s |
| S: | 63.7s | 1.0× | I/O dominated |
| **ALL parallel** | **78.5s** | **0.53× ⚠️** | Known regression target |

**Known issue:** Multi-drive parallel filtered scan is slower than full scan (78s > 72s). Root cause: streaming writer contention and per-drive extension index build overhead. This is a documented optimization target.

### Throughput by Drive Type

| Drive Type | Throughput (v0.1.30) | Throughput (v0.3.54) | Improvement |
|------------|---------------------|---------------------|-------------|
| NVMe | 400–550 MB/s | **540–865 MB/s** | ~1.7× |
| HDD | 53–103 MB/s | **155–160 MB/s** | ~1.8× |

Note: NVMe throughput is limited by parsing speed, not I/O bandwidth. The raw device can deliver 3–7 GB/s, but UFFS achieves ~0.5–0.9 GB/s because record parsing is the bottleneck.

---

## The Secret Sauce: Why UFFS Is Fast

### Optimization 1: Direct MFT Reading (15× vs Standard APIs)

**Problem:** Windows file enumeration (`FindFirstFile`/`FindNextFile`) requires ~2 syscalls per file, with security checks and handle management for each.

**Solution:** Read the MFT as a raw byte stream via a single volume handle. One `ReadFile` call processes ~1000 files (1MB ÷ 1KB records).

| Approach | Syscalls (2M files) | Time (NVMe) |
|----------|---------------------|-------------|
| Standard APIs | ~4,000,000 | 60–120s |
| Direct MFT read | ~2,000 | **5–8s** |

This is the foundational design decision. Everything else is incremental on top of this.

### Optimization 2: Bitmap Skip (50–80% I/O Reduction)

**Problem:** The MFT contains records for all files ever created, including deleted ones. Typical utilization is 40–60% — more than half the I/O is wasted reading empty slots.

**Solution:** Read `$MFT::$BITMAP` first (~250 KB) to learn which records are in use. Trim read ranges from both ends of each extent to skip contiguous unused regions.

**Measured impact:**

| Drive | MFT Size | Used Records | Skip % | I/O Saved |
|-------|----------|-------------|--------|-----------|
| C: (NVMe) | 4.5 GB | 60% | 40% | 1.8 GB |
| S: (HDD) | 11.5 GB | 45% | 55% | 6.3 GB |

On HDD, this is the single largest optimization — 6 GB of saved disk reads at ~150 MB/s saves ~40 seconds.

### Optimization 3: IOCP Sliding Window (I/O + CPU Overlap)

**Problem:** Sequential reads waste CPU time waiting for I/O. Parse time is wasted waiting for the next buffer.

**Solution:** I/O Completion Ports with a sliding window of N concurrent reads. While buffer N is being parsed, buffers N+1 through N+K are already in flight.

**Tuning per drive type:**

| Drive Type | Window Size | Chunk Size | Rationale |
|------------|-------------|------------|-----------|
| NVMe | 32 | 4 MB | Deep queue to saturate NVMe command queue |
| SSD | 8 | 2 MB | Moderate parallelism, SATA NCQ depth |
| HDD | 2–6 | 1 MB | More concurrent reads = more seeks = slower |

**HDD extent-aware tuning:** For HDDs, concurrency is further reduced when the MFT is heavily fragmented (>50 extents → 2 reads in flight) because each concurrent read on a different extent causes a disk seek.

### Optimization 4: Inline Parsing (Zero Intermediate Copies)

**Problem:** A two-phase approach (read all → then parse all) doubles memory usage and loses cache locality.

**Solution:** `SlidingIocpInline` — parse each completed I/O buffer directly into the `MftIndex` as the IOCP completion arrives. No intermediate `Vec<ParsedRecord>`, no second pass.

**Measured impact:** The inline path eliminated ~15–20s of DataFrame construction overhead that existed in v0.1.x.

### Optimization 5: Compact Memory Layout (224 Bytes/Record)

**Problem:** Per-record overhead at the scale of millions of records becomes significant. Naive Rust structs with `Option<>`, `String`, and `Vec` waste cache lines.

**Solution:** A hand-tuned `FileRecord` at 224 bytes with:
- **Bit-packed flags**: 17 boolean attributes in a single `u32`
- **Inline first_name/first_stream**: No heap allocation for the 95%+ of files with one name and one stream
- **Sentinel values**: `NO_ENTRY = u32::MAX` instead of `Option<u32>` (saves 4 bytes × millions)
- **Contiguous names buffer**: All filenames in one `String` allocation, referenced by `(offset, length)` pairs
- **Extension interning**: 16-bit IDs instead of per-name extension strings

**Memory usage for 2M files:**

| Component | Size | Notes |
|-----------|------|-------|
| Records | 448 MB | 2M × 224 bytes |
| Names buffer | 46 MB | ~23 bytes/name avg |
| FRS lookup | 20 MB | Sparse array for O(1) access |
| Children + overflow | 60 MB | Links, streams, child lists |
| **Total** | **~575 MB** | |

### Optimization 6: mimalloc Global Allocator

**Problem:** System allocator throughput degrades under heavy small-allocation pressure (millions of strings, records, linked list nodes during parsing).

**Solution:** `mimalloc` as the global allocator. Purpose-built for this workload pattern.

**Measured impact:** ~10–15% throughput improvement on NVMe drives where parsing is the bottleneck.

### Optimization 7: Extension Index (50× for `*.ext` Queries)

**Problem:** Scanning 2M records for `*.txt` via `ends_with(".txt")` takes ~100ms.

**Solution:** Intern all file extensions during parsing (16-bit IDs), then build an inverted index: `ext_id → Vec<record_index>`. For `*.txt`, look up the extension ID and iterate only matching records.

**Measured impact:**

| Query | Full scan | Extension index | Speedup |
|-------|-----------|-----------------|---------|
| `*.txt` (50K results) | 100ms | **2ms** | **50×** |
| `*.rs` (5K results) | 100ms | **0.5ms** | **200×** |

### Optimization 8: Zero-Allocation Case-Insensitive Matching

**Problem:** `.to_ascii_lowercase()` allocates a new `String` for every comparison. For 2M records × case-insensitive match = 2M heap allocations per search.

**Solution:** Byte-level comparison that converts characters inline without allocating:

```rust
fn ends_with_ignore_ascii_case(input: &str, suffix_lower: &str) -> bool {
    input.as_bytes()[start..]
        .iter()
        .zip(suffix_lower.as_bytes())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}
```

**Measured impact:** Eliminates 2–8M heap allocations per search query.

### Optimization 9: Leaf-Peeling Tree Metrics (O(n), No Recursion)

**Problem:** Computing treesize/descendants for millions of directories via recursive DFS risks stack overflow and has poor cache locality.

**Solution:** Array-based Kahn-style topological sort (leaf peeling). Two flat arrays (`parent_idx`, `pending_children`), process bottom-up by pushing leaves to a stack.

- **Time**: O(n) — each node processed exactly once
- **Space**: O(n) — two temporary arrays
- **No recursion** — guaranteed stack safety on any tree depth
- **Cache-friendly** — array-based sequential access

### Optimization 10: LCN-Ordered Reads (HDD Only)

**Problem:** Reading MFT extents in VCN order (logical) may cause excessive disk seeks when the MFT is fragmented, because consecutive VCN ranges may map to distant physical locations.

**Solution:** Sort read chunks by `disk_offset` (LCN order) before issuing reads. This minimizes head movement on HDDs.

**Measured impact:** 20–30% improvement on fragmented HDDs. No effect on NVMe/SSD (random access is fast).

---

## Where Time Is Spent (Phase Breakdown)

### NVMe Drive (C:, ~5M records, 8.3s)

```
Phase                Time     %    Bottleneck
───────────────────────────────────────────────
Volume open          <1ms   <1%
Metadata + bitmap     15ms   <1%
Chunk planning         1ms   <1%
IOCP read + parse   6.5s    78%   ★ PARSING (CPU-bound)
Tree metrics        0.8s    10%
Extension index     0.3s     4%
Stats + finalize    0.5s     6%
```

### HDD Drive (S:, ~10M records, 71.8s)

```
Phase                Time     %    Bottleneck
───────────────────────────────────────────────
Volume open          <1ms   <1%
Metadata + bitmap     25ms   <1%
Chunk planning         2ms   <1%
IOCP read + parse  68.0s    95%   ★ DISK I/O (seek-bound)
Tree metrics        1.5s     2%
Extension index     0.8s     1%
Stats + finalize    1.3s     2%
```

**Key insight:** On NVMe, the bottleneck is CPU (parsing). On HDD, the bottleneck is disk I/O (seeks + sequential read bandwidth). Optimizations must target the right layer for each drive type.

---

## Evolution: Key Milestones

### v0.1.30 → v0.1.39: Phase 1 (55% Faster)

| Optimization | Impact | Technique |
|-------------|--------|-----------|
| Structure-of-Arrays layout | 18% faster | Skip extension merging in fast path |
| Bitmap skip | 30–50% I/O reduction | Read $BITMAP first, trim read ranges |
| I/O overlap | 15% faster | Pipelined reads with IOCP |

Result: 315s → 142s across 7 drives.

### v0.1.39 → v0.2.66: Phase 2 (NVMe Tuning)

| Optimization | Impact | Technique |
|-------------|--------|-----------|
| Adaptive concurrency | Automatic | NVMe=32, SSD=8, HDD=2–6 based on drive detection |
| Adaptive chunk size | Automatic | NVMe=4MB, SSD=2MB, HDD=1MB |
| NVMe throughput | 2.1–3.4 GB/s | Queue depth 32–64 saturates NVMe command queue |

Result: NVMe C: dropped from 3.1s to 2.16s (raw I/O). HDD unchanged (already at physical limit).

### v0.2.66 → v0.3.54: Production (Inline + Streaming)

| Optimization | Impact | Technique |
|-------------|--------|-----------|
| SlidingIocpInline | Eliminated DF build | Parse directly into MftIndex during I/O completion |
| Streaming output | Immediate results | Channel-based per-drive output-as-ready |
| Extension index | 50× for *.ext | Interned extension IDs + inverted index |
| mimalloc | ~10–15% | Purpose-built allocator for many-small-alloc workloads |

Result: 142s → 72s across 7 drives (parallel). NVMe C: end-to-end 8.3s including tree metrics and output.

---

## Benchmark Methodology

### Cold Start Protocol

Each benchmark run:
1. Flush OS file system cache (`sync` / restart benchmark harness)
2. 5 sequential rounds per drive, per pattern
3. Exclude anomalous first-run outliers (OS MFT cache warming)
4. Report: average, min, max

### What Is Measured

The benchmark measures **end-to-end wall-clock time** including:
- Volume handle opening
- NTFS metadata retrieval
- Bitmap reading and chunk planning
- Full MFT read + parse
- Tree metrics computation
- Extension index building
- Output formatting and writing to stdout

This is the time the user experiences from pressing Enter to seeing results.

### Reproducibility

- Use `--benchmark` flag to skip output formatting (isolates MFT reading)
- Use `--profile` flag for per-phase timing breakdown
- Use `--no-cache` to ensure fresh MFT reads
- All benchmarks use release builds (`cargo build --release`)

---

## Physical Limits and Theoretical Bounds

### HDD Sequential Read

A 7200 RPM HDD achieves ~150–200 MB/s sequential read. For an 11.5 GB MFT:
```
Theoretical minimum = 11.5 GB / 200 MB/s = 57.5s
UFFS achieved:       71.8s (with bitmap skip reducing effective reads)
Utilization:         ~80% of theoretical max
```

Further HDD optimization has diminishing returns — we're within 25% of the physical limit.

### NVMe Sequential Read

A Samsung 990 PRO achieves 7 GB/s sequential read. For a 4.5 GB MFT:
```
Theoretical minimum = 4.5 GB / 7 GB/s = 0.64s
UFFS achieved:       8.3s (including parsing, tree metrics, output)
I/O time only:       ~1.5s
Parsing time:        ~5s (the real bottleneck)
```

NVMe performance is **CPU-bound**, not I/O-bound. Future gains require faster parsing (SIMD, parallel parse across cores).

---

## Known Optimization Targets

| Target | Current Impact | Potential | Difficulty |
|--------|---------------|-----------|------------|
| Multi-drive filtered scan regression | 78s vs 72s (filtered > full) | Fix to match full-scan time | Medium |
| SIMD record parsing | N/A | 2–3× parsing speed on NVMe | High |
| Warm-cache search (< 1s) | Cache + USN implemented | Sub-second repeated queries | Done ✅ |
| Parallel tree metrics | Single-threaded O(n) | Marginal — already < 1s on NVMe | Low priority |

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62*
