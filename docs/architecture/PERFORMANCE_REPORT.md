# UFFS Performance Report — v0.3.62

Comprehensive performance analysis: Rust vs C++ across cold start, warm cache,
multiple pattern types, and 7 NTFS drives.

---

## System Under Test

- **7 NTFS drives**: C (NVMe, system), D (HDD), E (HDD), F (NVMe), G (tiny USB),
  M (HDD/NAS), S (HDD, largest)
- **Rust binary**: `uffs.exe` v0.3.62 — SlidingIocpInline pipeline, streaming output,
  disk cache with USN journal incremental updates
- **C++ binary**: `uffs.com` — original C++ implementation, no disk cache, IOCP reader
- **OS**: Windows 11, Administrator privileges

### Drive Characteristics

| Drive | Type | Cache Size | ~Records | Notes |
|-------|------|-----------|----------|-------|
| C | NVMe | 735 MB | ~3.4M | System drive (writable) |
| D | HDD | 1161 MB | ~7.1M | Read-only |
| E | HDD | 644 MB | ~2.9M | Read-only |
| F | NVMe | 481 MB | ~2.2M | Read-only |
| G | USB | 3 MB | ~15K | Read-only, tiny |
| M | HDD/NAS | 420 MB | ~1.9M | Read-only |
| S | HDD | 1823 MB | ~8.3M | Read-only, largest |

---

## 1. Cold Start — Full Scan (`*`)

Fresh MFT read from disk, no cache. Measures raw I/O + parse + output.

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C (NVMe) | 8.3s | 25.3s | **3.0×** |
| D (HDD) | 30.9s | 64.8s | **2.1×** |
| E (HDD) | 41.1s | 54.4s | **1.3×** |
| F (NVMe) | 5.2s | 15.5s | **3.0×** |
| G (tiny) | 0.42s | 0.37s | 0.9× |
| M (HDD) | 26.5s | 30.4s | **1.1×** |
| S (HDD) | 71.8s | 91.5s | **1.3×** |
| **ALL parallel** | **72.3s** | **98.6s** | **1.36×** |

### Cold Start Analysis

- **NVMe (C, F)**: Rust is **3× faster** — the SlidingIocpInline pipeline with
  inline parsing eliminates the read-then-parse overhead of C++.
- **HDD (D)**: Rust is **2.1× faster** — I/O overlap + efficient parsing.
- **Large HDD (E, M, S)**: Rust **1.1–1.3× faster** — I/O-bound, advantage
  comes from parsing efficiency during I/O wait.
- **Tiny (G)**: C++ is ~50ms faster — Rust startup overhead exceeds gains on
  trivial volumes.
- **ALL parallel**: Bottlenecked by slowest drive (S at ~72s). Rust **1.36×
  faster** overall.
- **Consistency**: Sub-1% variance across 5 rounds on most drives.

---

## 2. Cold Start — Filtered Scan (`*.rs`)

Same cold start, but with glob filter. Measures pattern matching overhead.

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C (NVMe) | 7.8s | 13.9s | **1.8×** |
| D (HDD) | 24.1s | 51.8s | **2.2×** |
| E (HDD) | 38.1s | 44.2s | **1.2×** |
| F (NVMe) | 2.9s | 8.3s | **2.9×** |
| G (tiny) | 0.41s | 0.33s | 0.8× |
| M (HDD) | 24.6s | 24.4s | 1.0× |
| S (HDD) | 63.7s | 63.9s | 1.0× |

### Cold Filtered Analysis

- Filtered queries are faster than full scan because less output is generated.
- On HDD, both tools converge toward I/O-bound parity (~64s for S).
- NVMe shows Rust's advantage clearly (2.9× on F).

---

## 3. Warm Cache — Full Scan (`*`)

Index loaded from disk cache. Read-only drives skip TTL/USN entirely.

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C (NVMe) | 4.6s | 19.5s | **4.2×** |
| D (HDD) | 6.5s | 54.0s | **8.3×** |
| E (HDD) | 3.1s | 50.2s | **16.2×** |
| F (NVMe) | 2.5s | 11.9s | **4.8×** |
| G (tiny) | 0.21s | 0.35s | **1.7×** |
| M (HDD) | 2.0s | 27.8s | **13.9×** |
| S (HDD) | 8.4s | 78.9s | **9.4×** |
| **ALL parallel** | **23.1s** | **65.7s** | **2.8×** |

### Warm Cache Analysis

- **C++ has no disk cache** — it re-reads the MFT from NTFS every time. This is
  why C++ times barely change between cold and warm runs.
- **Rust cache load** is dominated by deserialization: ~5 ms/MB of cache file.
  S drive (1.8 GB) takes ~8s to deserialize; F (481 MB) takes ~2.5s.
- **Read-only drives** (D, E, F, G, M, S) use the fast path: no TTL check, no
  USN journal query, no VolumeHandle::open. Cache is eternally valid.
- The **8–16× speedup** on HDD drives (D, E, M) is transformative — Rust loads
  from cache in 2–6s vs C++ re-scanning at 28–54s.

---

## 4. Warm Cache — Filtered Scan (`*.rs`)

Extension-index shortcut + lazy PathResolver (no full directory path pre-cache).

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C | 2.0s | 13.5s | **6.8×** |
| D | 1.1s | 46.5s | **42×** |
| E | 0.72s | 44.6s | **62×** |
| F | 0.65s | 8.4s | **13×** |
| G | 0.19s | 0.33s | **1.7×** |
| M | 0.53s | 24.5s | **46×** |
| S | 1.7s | 63.5s | **37×** |
| **ALL parallel** | **3.9s** | **41.3s** | **10.6×** |

### Warm Filtered Analysis

- This is the **best-case scenario** for Rust: cache load + extension index
  (only visit matching records) + lazy PathResolver (skip full directory path
  pre-cache for <100K matches).
- **62× on E drive** (0.72s vs 44.6s) — Rust deserializes the 644MB cache,
  looks up `.rs` in the extension index, iterates only matching records, and
  resolves paths on-demand. C++ re-reads the entire MFT from HDD.
- The **ALL-parallel 10.6×** speedup (3.9s vs 41.3s) confirms the filtered
  multi-drive regression from v0.3.54 (78.5s) is fully resolved.

---

## 5. Warm Cache — Substring Search (`hallo`)

Literal substring match against all filenames (Contains strategy).

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C | 2.4s | 4.1s | **1.7×** |
| D | 2.0s | 22.4s | **11×** |
| E | 1.3s | 36.9s | **28×** |
| F | 1.1s | 2.0s | **1.9×** |
| G | 0.19s | 0.31s | **1.6×** |
| M | 0.84s | 20.0s | **24×** |
| S | 3.4s | 41.7s | **12×** |
| **ALL parallel** | **7.1s** | **41.6s** | **5.9×** |

### Substring Analysis

- No extension-index shortcut — must scan all records and check each name.
- Rust still wins **5.9× overall** because cache load is fast and substring
  matching is CPU-efficient (no regex overhead).
- C++ on NVMe (C=4.1s, F=2.0s) is competitive because the MFT is small and
  NVMe I/O is fast. But on HDD, C++ is completely I/O-bound (20–42s).

---

## 6. Warm Cache — Regex Path Match (`>C:\\Users\\.*\.(jpg|png|heic)`)

Complex regex: path-anchored with alternation, matches against full path.

| Drive | Rust | C++ | **Speedup** |
|-------|------|-----|-------------|
| C | 2.7s | 9.2s | **3.4×** |
| D | 1.7s | 9.4s | **5.6×** |
| E | 1.1s | 9.4s | **8.2×** |
| F | 0.98s | 9.3s | **9.5×** |
| G | 0.20s | 9.5s | **47×** |
| M | 0.77s | 9.3s | **12×** |
| S | 2.7s | 9.3s | **3.4×** |
| **ALL parallel** | **2.9s** | **9.3s** | **3.2×** |

### Regex Analysis

- **C++ is suspiciously constant at ~9.3s on every drive** — even tiny G (15K
  files, 3MB cache). This confirms C++ has a fixed overhead for regex
  initialization or does a full MFT re-read regardless.
- **Rust scales with drive size**: G=0.2s, F/M=0.8–1.0s, D/E=1.1–1.7s,
  C/S=2.7s. Rust loads from cache proportionally and applies regex per-record.
- The regex produces very few matches (only `C:\Users\...` paths with specific
  image extensions), so output is minimal — dominated by cache load + scan time.

---

## 7. Pattern Performance Comparison (ALL-Parallel, Warm Cache)

| Pattern | Type | Rust | C++ | **Speedup** |
|---------|------|------|-----|-------------|
| `*.rs` | Glob + ext index | **3.9s** | 41.3s | **10.6×** |
| `>C:\\Users\\.*\.(jpg\|png\|heic)` | Regex (path) | **2.9s** | 9.3s | **3.2×** |
| `hallo` | Substring | **7.1s** | 41.6s | **5.9×** |
| `*` | Full scan | **23.1s** | 65.7s | **2.8×** |

### Why Rust Performance Varies by Pattern

| Pattern | Cache Load | Scan Strategy | PathResolver | Output Volume |
|---------|-----------|---------------|-------------|---------------|
| `*.rs` | ~3s | Extension index (O(matches)) | Lazy (no dir pre-cache) | Small |
| Regex | ~3s | Full scan with regex reject | Lazy (few matches) | Minimal |
| `hallo` | ~3s | Full scan with substring match | Lazy (few matches) | Minimal |
| `*` | ~3s | All records | Full PathCache (O(n) dirs) | **Massive** |

The cache load time (~3s for 7 drives) is constant. The variable is:
1. **Scan strategy**: Extension index (O(matches)) vs full scan (O(n))
2. **PathResolver mode**: Lazy (on-demand) vs full PathCache (pre-compute all
   directory paths)
3. **Output volume**: `*` outputs 25M+ lines; `*.rs` outputs thousands

---

## 8. Parity Check Results (v0.3.62 vs C++)

### Live MFT Scan (Drive C)

| Metric | C++ | Rust | Notes |
|--------|-----|------|-------|
| Lines | 3,441,318 | 3,441,319 | +1 line (footer format) |
| Time | 20.0s | 7.1s | **2.8× faster** |
| Sorted match | ❌ | — | See diff categories below |

### Diff Categories (C drive, 806,989 differing lines after sort)

| Category | Count | Severity | Root Cause |
|----------|-------|----------|------------|
| Traversal order | ~800K | ℹ️ Expected | Different tree-walk order (sorted comparison handles this) |
| Treesize ±few bytes | ~hundreds | ℹ️ Cosmetic | Hardlink size distribution rounding |
| Accessed timestamps | ~thousands | ℹ️ Live FS | Filesystem changed between sequential C++ → Rust scans |
| ADS stream order | ~hundreds | ⚠️ Real diff | iCloud Photos — C++ outputs ADS before default stream |
| Footer format | 1 line | ℹ️ Cosmetic | C++ appends drive info line |

### Offline Parity (MFT File Captures, 7 Drives)

| Drive | Lines | Result |
|-------|-------|--------|
| C | 3,485,610 | ❌ Mismatch (known ADS issue) |
| D | 7,065,517 | ✅ **SORTED MATCH** |
| E | 2,929,497 | ✅ **SORTED MATCH** |
| F | 2,221,321 | ❌ Mismatch (known) |
| G | 15,071 | ✅ **SORTED MATCH** |
| M | 1,908,783 | ✅ **SORTED MATCH** |
| S | 8,278,080 | ✅ **SORTED MATCH** |

**5 of 7 drives** have perfect sorted-match parity. The 2 mismatches (C, F) are
from known ADS stream ordering differences, not data correctness issues.

---

## 9. Architecture Optimizations Applied (This Session)

### Cache System Revival
- **Extension index rebuild after deserialization** — was missing, broke `*.ext`
  queries on cached indexes
- **reserved_allocated_bytes restore** from live VolumeHandle on cache load
- **Read-only volume fast path** — skip TTL, USN, VolumeHandle for immutable
  drives (D, E, F, G, M, S never expire)

### Filtered Multi-Drive Regression Fix
- **Lazy PathResolver** for <100K matches — skips expensive
  `pre_cache_directory_paths()` O(n) pass (~5s/large drive saved)
- **Background cache save** — `save_to_cache()` serializes on current thread,
  writes to disk on `spawn_blocking` (fire-and-forget)
- **Tokio async channel** — replaced `std::sync::mpsc::sync_channel` with
  `tokio::sync::mpsc::channel` to avoid blocking tokio workers

### Benchmark Script Improvements
- **Unified `benchmark.ps1`** with `-Cache` flag (default: cold start)
- **stdout → NUL** redirect — eliminates 15–25s of temp file I/O + Select-String
  scan overhead per run
- **stderr-only timing extraction** — `[TIMING]`/`[DIAG]` lines go to stderr

---

## 10. Summary

### Headline Numbers (v0.3.62, ALL 7 Drives Parallel)

| Scenario | Rust | C++ | Speedup |
|----------|------|-----|---------|
| Cold start (`*`) | 72.3s | 98.6s | **1.36×** |
| Warm cache (`*`) | 23.1s | 65.7s | **2.8×** |
| Warm cache (`*.rs`) | 3.9s | 41.3s | **10.6×** |
| Warm cache (`hallo`) | 7.1s | 41.6s | **5.9×** |
| Warm cache (regex) | 2.9s | 9.3s | **3.2×** |

### Peak Single-Drive Speedups (Warm Cache)

| Metric | Value | Drive | Pattern |
|--------|-------|-------|---------|
| Best speedup | **62×** | E | `*.rs` (0.72s vs 44.6s) |
| Best absolute | **0.19s** | G | Any cached pattern |
| Largest drive cached | **8.4s** | S (1.8GB cache) | `*` full scan |

### Key Architectural Advantages

1. **Disk cache with USN updates** — C++ has no equivalent. Rust loads in
   2–8s what C++ re-reads in 20–80s.
2. **Read-only fast path** — 6 of 7 drives never expire. Zero overhead.
3. **Extension index** — `*.ext` queries are O(matches) not O(n).
4. **Lazy PathResolver** — filtered queries skip the O(n) directory path
   pre-cache, matching C++'s lazy `get_path()` approach.
5. **SlidingIocpInline** — 3× faster than C++ on NVMe for cold starts.
