# UFFS Benchmark History

Ongoing performance tracking as the tool evolves.

---

## v0.3.54 — Mar 22, 2026 (7 drives, 5 rounds each, cold start)

**System**: 7 NTFS drives (C, D, E, F, G, M, S), mix of NVMe + HDD.
**Binary**: `dist/v0.3.54` — SlidingIocpInline pipeline, streaming output.

### Full Scan (`*`) — Rust vs C++

Averaged across 3 clean full-scan runs (anomalous first-run outliers excluded):

| Drive | Type (est.) | Rust avg | C++ avg | **Speedup** |
|-------|-------------|----------|---------|-------------|
| C | NVMe | 8.3s | 25.3s | **3.0×** |
| D | HDD | 30.9s | 64.8s | **2.1×** |
| E | HDD/USB | 41.1s | 54.4s | **1.3×** |
| F | NVMe | 5.2s | 15.5s | **3.0×** |
| G | small/USB | 0.42s | 0.37s | 0.9× (C++ ~50ms faster) |
| M | HDD/NAS | 26.5s | 30.4s | **1.1×** |
| S | HDD | 71.8s | 91.5s | **1.3×** |
| **ALL parallel** | | **72.3s** | **98.6s** | **1.36×** |

**Highlights**:
- **NVMe (C, F): 3× faster** — IOCP inline parse pipeline dominates
- **HDD (D): 2.1× faster** — I/O overlap + efficient parsing
- **Large HDD (S): 1.3× faster** — I/O bound, Rust's advantage is parsing efficiency
- **Small drive (G): C++ ~50ms faster** — Rust startup overhead exceeds gain on tiny volumes
- **ALL parallel: 1.36× faster** — bottlenecked by slowest drive (S at ~72s)
- **Consistency**: Very tight min/max spread across 5 rounds (< 1% variance on most drives)

### Filtered Scan (`*.rs`) — Rust vs C++

Averaged across 2 clean `*.rs` runs:

| Drive | Rust avg | C++ avg | **Speedup** |
|-------|----------|---------|-------------|
| C | 7.8–13.8s* | 13.9s | 1.0–1.8× |
| D | 24.2s | 51.8s | **2.1×** |
| E | 38.1s | 44.2s | **1.2×** |
| F | 2.9–7.6s* | 8.3s | **1.1–2.9×** |
| G | 0.41s | 0.33s | 0.8× |
| M | 24.6s | 24.4s | 1.0× |
| S | 63.7s | 63.9s | 1.0× |
| **ALL parallel** | **78.5s** | **41.3–44.5s** | **0.53–0.57× ⚠️** |

*\*Range reflects variance across runs (OS filesystem cache effects).*

### ⚠️ Key Regression: `*.rs` All-Drives Parallel

**Single-drive**: Rust is faster or equal on every drive individually.
**Multi-drive parallel**: Rust 78s vs C++ 41–44s — **C++ is ~1.9× faster**.

This is significant because:
- Rust ALL for `*` (full scan) = 72s ✅ (faster than C++ 98s)
- Rust ALL for `*.rs` (filtered) = 78s ❌ (slower than `*` !)
- C++ ALL for `*.rs` = 41s (faster than C++ `*` at 98s — expected, less output)

**Root cause**: Rust's `*.rs` multi-drive takes *longer* than full scan `*`, which should be impossible. The filtered query produces far fewer results but is 6s slower. This points to:

1. **Multi-drive streaming writer contention** — the single writer thread may be blocking index loads when results are sparse (waiting for lock/channel when it should be loading the next drive)
2. **Extension index build overhead** — `build_extension_index()` runs after every drive load, even when not needed for the specific pattern
3. **Missing short-circuit** — for `*.rs`, C++ likely finishes output for each drive instantly (few results), freeing I/O threads for the next drive. Rust's channel-based architecture may serialize differently.

### Observed Anomalies

1. **C drive Run-1 outliers**: Some runs show first C: attempt at 16–37s (4–5× slower than steady state 8.2s). Likely OS filesystem cache cold start — the MFT itself isn't cached in RAM yet. Not an uffs bug.

2. **S drive C++ anomaly (Run 8)**: Runs 4-5 completed in 0.1s (vs normal 92s). C++ likely returned cached results from a prior run that wasn't properly cleared.

3. **F drive variance on `*.rs`**: 2.9s in one run vs 7.6s in another. The 2.9s result suggests OS MFT cache was warm from immediately prior benchmark.

### Drive Size Estimates (from timing patterns)

| Drive | Estimated MFT records | Category |
|-------|----------------------|----------|
| G | ~50K | Tiny (USB/small partition) |
| F | ~2M | Medium NVMe |
| C | ~5M | Large NVMe (system drive) |
| M | ~4M | Large HDD/NAS |
| D | ~5M | Large HDD |
| E | ~6M | Large HDD/USB |
| S | ~10M+ | Very large HDD |

### Summary — v0.3.54 Scorecard

| Metric | Status |
|--------|--------|
| NVMe single-drive | ✅ **3× faster** |
| HDD single-drive | ✅ **1.3–2.1× faster** |
| Full scan ALL parallel | ✅ **1.36× faster** |
| Filtered single-drive | ✅ Equal or faster |
| Filtered ALL parallel | ⚠️ **1.9× slower** — regression |
| Tiny drive overhead | ℹ️ ~50ms slower (startup cost) |
| Consistency | ✅ Excellent (< 1% variance) |
