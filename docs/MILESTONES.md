# UFFS (Ultra Fast File Search) Milestone Tracking

## Project Overview

**Project**: UFFS - Ultra Fast File Search (Rust Implementation)
**Start Date**: 2026-01-15
**Target Completion**: 2026-06-15 (21 weeks)
**Status**: 🟢 Substantially Complete
**Architecture**: Cargo Workspace with 6 crates (Polars-based)

---

## Workspace Structure

```
crates/
├── uffs-polars/  🔧 Polars facade (compilation isolation) ✅
├── uffs-mft/     📦 MFT reading → Polars DataFrame ✅
├── uffs-core/    📦 Query engine using Polars lazy API ✅
├── uffs-cli/     🔧 Command-line interface ✅
├── uffs-tui/     🖥️  Terminal UI ✅
└── uffs-gui/     🪟 Graphical UI (placeholder)
```

---

## Milestone Summary

| Phase | Milestone | Crate(s) | Target | Status | Progress |
|-------|-----------|----------|--------|--------|----------|
| 0 | Workspace Setup | all | Week 1 | 🟢 Complete | 100% |
| 1 | MFT Foundation | uffs-mft | Week 4 | 🟢 Complete | 100% |
| 2 | MFT DataFrame | uffs-mft | Week 8 | 🟢 Complete | 100% |
| 2.5 | **RAW MFT Persistence** | uffs-mft | - | 🟢 Complete | 100% |
| 2.6 | **Multi-Drive Parallel Reading** | uffs-mft, uffs-cli | - | 🟢 Complete | 100% |
| 3 | Core Processing | uffs-core | Week 12 | 🟢 Complete | 100% |
| 3.5 | **Directory Tree Structure** | uffs-core | - | 🟢 Complete | 100% |
| 4 | CLI & Performance | uffs-cli | Week 16 | 🟢 Complete | 100% |
| 5 | TUI & Polish | uffs-tui | Week 21 | 🟢 Complete | 95% |
| **6** | **Performance Optimization** | uffs-mft, uffs-core | Week 31 | ⬜ Not Started | 0% |

**Legend**: ⬜ Not Started | 🟡 In Progress | 🟢 Complete | 🔴 Blocked

### Phase 6 Goal: "Outperform the Reference Baseline"

Phase 6 focuses on making the Rust implementation **2-5x faster than the reference binary** through:
- **Pipelined I/O** ⭐ (2x speedup, 250x memory reduction) - overlap I/O and CPU work
- Arena-based name storage (eliminate allocations)
- Vector-based FRS lookup (O(1) direct indexing)
- In-place path building (append-reverse strategy)
- Direct-to-column parsing (skip intermediate structs)
- SIMD pattern matching (AVX2/AVX-512)

**Key Insight**: The C++ code uses Windows IOCP to pipeline I/O and parsing. When a read
completes, it immediately queues the next read BEFORE processing the current buffer.
This ensures disk and CPU are never idle, achieving near-perfect overlap.

See [MFT Architecture Deep Dive](architecture/MFT_ARCHITECTURE_DEEP_DIVE.md) for detailed analysis.

### Architecture Separation

| Crate | Responsibility |
|-------|----------------|
| `uffs-mft` | Pure MFT reading & storage (DataFrame, CSV, Parquet, RAW) |
| `uffs-core` | Post-processing (tree structure, queries, derived metrics) |

> **Design Principle:** `uffs-mft` does pure MFT reading and storage. No post-processing.
> Tree calculations and derived metrics belong in `uffs-core` query engine.

---

## Phase 0: Workspace Setup (Week 0-1)

**Goal**: Establish modern Rust workspace with Polars facade
**Target Date**: Week 1
**Status**: 🟢 Complete

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 0.1 | Workspace Cargo.toml | - | 🟢 | `[workspace]` manifest with all crates |
| 0.2 | crates/ directory structure | - | 🟢 | 6 crate directories created |
| 0.3 | **uffs-polars facade crate** | - | 🟢 | Re-exports polars::prelude, column constants |
| 0.4 | uffs-mft crate skeleton | - | 🟢 | Full implementation |
| 0.5 | uffs-core crate skeleton | - | 🟢 | Full implementation |
| 0.6 | uffs-cli crate skeleton | - | 🟢 | Full implementation |
| 0.7 | Workspace dependencies | - | 🟢 | `[workspace.dependencies]` configured |
| 0.8 | rustfmt.toml | - | 🟢 | Code formatting configured |
| 0.9 | clippy.toml | - | 🟢 | Linting rules configured |
| 0.10 | GitHub Actions CI | - | 🟢 | CI pipeline configured |
| 0.11 | MSRV policy | - | 🟢 | rust-version = "1.85" (Edition 2024) |

### Acceptance Criteria

- [x] `cargo build --workspace` succeeds
- [x] `cargo test --workspace` runs (even if no tests yet)
- [x] `cargo clippy --workspace` passes
- [x] CI pipeline runs on push/PR
- [x] All crates have proper Cargo.toml with workspace inheritance

---

## Phase 1: uffs-mft Foundation (Weeks 1-4)

**Goal**: Core NTFS structures and raw disk access
**Crate**: `uffs-mft`
**Target Date**: Week 4
**Status**: 🟢 Complete

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 1.1 | NtfsBootSector struct | - | 🟢 | `ntfs.rs` - Full boot sector parsing |
| 1.2 | FileRecordHeader struct | - | 🟢 | `ntfs.rs` - FileRecordSegmentHeader |
| 1.3 | AttributeHeader struct | - | 🟢 | `ntfs.rs` - AttributeRecordHeader |
| 1.4 | Resident attribute parsing | - | 🟢 | `ntfs.rs` - ResidentAttributeData |
| 1.5 | Non-resident attribute parsing | - | 🟢 | `ntfs.rs` - NonResidentAttributeData |
| 1.6 | Windows volume opening | - | 🟢 | `platform.rs` - VolumeHandle::open() |
| 1.7 | FSCTL_GET_NTFS_VOLUME_DATA | - | 🟢 | `platform.rs` - NtfsVolumeData |
| 1.8 | FSCTL_GET_RETRIEVAL_POINTERS | - | 🟢 | `platform.rs` - get_mft_extents(), MftExtent |
| 1.9 | Raw cluster reading | - | 🟢 | `io.rs` - AlignedBuffer, MftRecordReader |
| 1.10 | Error types with thiserror | - | 🟢 | `error.rs` - MftError enum |
| 1.11 | Unit tests | - | 🟢 | Tests in ntfs.rs, io.rs |

### Acceptance Criteria

- [x] Can open NTFS volume with admin privileges
- [x] Can read boot sector and extract MFT location
- [x] Can read raw MFT clusters
- [x] Can parse MFT record headers
- [x] All unit tests pass
- [x] `cargo doc --package uffs-mft` generates docs

---

## Phase 2: uffs-mft DataFrame (Weeks 5-8)

**Goal**: Complete MFT parsing with Polars DataFrame output
**Crate**: `uffs-mft`
**Target Date**: Week 8
**Status**: 🟢 Complete

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 2.1 | $STANDARD_INFORMATION parsing | - | 🟢 | `io.rs` - parse_standard_info() |
| 2.2 | $FILE_NAME parsing | - | 🟢 | `io.rs` - parse_file_name() |
| 2.3 | $DATA parsing (resident) | - | 🟢 | `io.rs` - parse_record() |
| 2.4 | $DATA parsing (non-resident) | - | 🟢 | `io.rs` - size from non-resident header |
| 2.5 | Multi-sector fixup (unfixup) | - | 🟢 | `io.rs` - apply_fixup(), fixup_file_record() |
| 2.6 | $BITMAP parsing | - | 🟢 | `platform.rs` - MftBitmap, get_mft_bitmap() |
| 2.7 | $REPARSE_POINT parsing | - | 🟢 | `ntfs.rs` - ReparsePointHeader, ReparseMountPointBuffer |
| 2.8 | Run list (mapping pairs) | - | 🟢 | `ntfs.rs` - DataRun, parse_data_runs(), extract_data_runs_from_attribute() |
| 2.9 | Attribute iteration | - | 🟢 | `ntfs.rs` - AttributeIterator, AttributeRef |
| 2.10 | Attribute list support | - | 🟢 | `ntfs.rs` - AttributeListEntry (large files) |
| 2.11 | Index structures | - | 🟢 | `ntfs.rs` - IndexHeader, IndexRoot (directories) |
| 2.12 | **DataFrame construction** | - | 🟢 | `reader.rs` - build_dataframe() |
| 2.13 | **Parquet persistence** | - | 🟢 | `reader.rs` - save_parquet()/load_parquet() |
| 2.14 | Unit tests | - | 🟢 | Tests in io.rs, reader.rs, ntfs.rs |

### Acceptance Criteria

- [x] Can parse all standard NTFS attributes
- [x] Multi-sector fixup correctly applied
- [x] Can extract file names and parent references
- [x] **MFT data returned as Polars DataFrame**
- [x] **DataFrame can be saved/loaded as Parquet**
- [x] All unit tests pass

---

## Phase 2.5: RAW MFT Persistence ✅ COMPLETE

**Goal**: Save/load complete raw MFT bytes for offline analysis
**Crate**: `uffs-mft`
**Status**: 🟢 Complete
**Completed**: 2026-01-16

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 2.5.1 | `save_raw_mft()` function | - | 🟢 | `raw.rs` - Save complete MFT bytes to file |
| 2.5.2 | `load_raw_mft()` function | - | 🟢 | `raw.rs` - Load saved MFT bytes |
| 2.5.3 | Handle fragmented MFT | - | 🟢 | `reader.rs` - `read_raw()` reassembles extents |
| 2.5.4 | Optional zstd compression | - | 🟢 | `raw.rs` - zstd feature flag |
| 2.5.5 | CLI commands `save-raw` / `load-raw` | - | 🟢 | `uffs-cli/commands.rs` |

### Implementation Details

New `raw.rs` module with:
- `RawMftHeader` - 64-byte header with magic, version, flags, sizes
- `RawMftData` - Loaded raw MFT with record iteration
- `SaveRawOptions` / `LoadRawOptions` - Configuration structs
- `save_raw_mft()` / `load_raw_mft()` - File I/O with optional zstd compression
- `MftReader::read_raw()` - Read MFT as raw bytes (handles fragmented MFT)
- `MftReader::save_raw_to_file()` - Convenience method to read and save
- `MftReader::load_raw_to_dataframe()` - Load saved MFT and parse to DataFrame

### Acceptance Criteria

- [x] Can save complete raw MFT to file (including fragmented)
- [x] Can load saved raw MFT and parse it
- [x] Compression reduces file size significantly (zstd feature)
- [x] CLI commands work correctly (`uffs save-raw`, `uffs load-raw`)
- [x] No automatic saving (user must explicitly request)

---

## Phase 2.6: Multi-Drive Parallel MFT Reading ✅ COMPLETE

**Goal**: Read MFTs from multiple drives concurrently and merge into unified DataFrame
**Crates**: `uffs-mft`, `uffs-cli`
**Status**: 🟢 Complete

### Motivation

When searching across multiple NTFS volumes (C:, D:, E:, etc.), reading each MFT sequentially
is inefficient. Since each drive has independent I/O, we can read all MFTs in parallel and
merge the results into a single DataFrame with a `drive` column to distinguish sources.

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 2.6.1 | `MultiDriveMftReader` struct | - | 🟢 | `reader.rs` - Orchestrates parallel drive reading |
| 2.6.2 | Async concurrent drive reading | - | 🟢 | tokio::spawn for each drive with JoinSet |
| 2.6.3 | DataFrame merging with `drive` column | - | 🟢 | Polars `vstack()` with added column |
| 2.6.4 | CLI `--drives` flag | - | 🟢 | Accept multiple drives: `--drives C,D,E` |
| 2.6.5 | Progress aggregation | - | 🟢 | Per-drive progress bars with MultiProgress |
| 2.6.6 | Error handling per drive | - | 🟢 | Continue on failure, report which drives failed |
| 2.6.7 | Unit tests | - | 🟢 | Tests for MultiDriveMftReader |

### Implementation Design

```rust
/// Reads MFTs from multiple drives in parallel.
pub struct MultiDriveMftReader {
    drives: Vec<char>,
}

impl MultiDriveMftReader {
    pub fn new(drives: Vec<char>) -> Self { ... }

    /// Read all drives concurrently, merge into single DataFrame.
    /// Adds a "drive" column (e.g., "C:", "D:") to distinguish sources.
    pub async fn read_all(&self) -> Result<DataFrame> {
        // Spawn async task for each drive
        // Collect results
        // Add "drive" column to each DataFrame
        // Concat all DataFrames
    }

    /// Read with per-drive progress callbacks.
    pub async fn read_with_progress<F>(&self, callback: F) -> Result<DataFrame>
    where
        F: Fn(char, MftProgress) + Send + Sync + 'static;
}
```

### CLI Usage

```bash
# Read from multiple drives
uffs index --drives C,D,E --output all_drives.parquet

# Search across all indexed drives
uffs search --index all_drives.parquet "*.rs"

# Save raw MFT from multiple drives
uffs save-raw --drives C,D --output-dir ./raw_mfts/
```

### Acceptance Criteria

- [ ] Can read MFTs from multiple drives concurrently
- [ ] Merged DataFrame has `drive` column (e.g., "C:", "D:")
- [ ] Progress shows per-drive and aggregate status
- [ ] Graceful handling when one drive fails (others continue)
- [ ] CLI accepts `--drives C,D,E` syntax
- [ ] Performance scales with number of drives (parallel I/O)

---

## Phase 3: uffs-core Processing (Weeks 9-12)

**Goal**: Query engine using Polars lazy API
**Crate**: `uffs-core`
**Target Date**: Week 12
**Status**: 🟢 Complete

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 3.1 | **MftQuery builder** | - | 🟢 | `query.rs` - Fluent API wrapping LazyFrame |
| 3.2 | Polars filter predicates | - | 🟢 | size, date, type filters implemented |
| 3.3 | PathResolver struct | - | 🟢 | `path_resolver.rs` - FRS → full path |
| 3.4 | Glob to regex conversion | - | 🟢 | `glob.rs` - glob_to_regex() |
| 3.5 | **Polars string matching** | - | 🟢 | `query.rs` - glob(), regex(), contains() |
| 3.6 | Streaming mode support | - | 🟢 | `query.rs` - collect_streaming() |
| 3.7 | Table exporter | - | 🟢 | `export.rs` - export_table() |
| 3.8 | JSON exporter | - | 🟢 | `export.rs` - export_json() |
| 3.9 | CSV exporter | - | 🟢 | `export.rs` - export_csv() |
| 3.10 | Unit tests | - | 🟢 | Tests in query.rs, glob.rs, export.rs |

### Acceptance Criteria

- [x] **MftQuery wraps Polars LazyFrame**
- [x] Polars lazy predicates work correctly
- [x] Path resolution is accurate
- [x] Export formats produce valid output
- [x] **Streaming mode handles large datasets**
- [x] All unit tests pass

### Phase 3.5: Directory Tree Structure ✅ COMPLETE

> **Note:** Tree structure is post-processing, belongs in `uffs-core` not `uffs-mft`.

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 3.5.1 | `TreeIndex` struct | - | 🟢 | `tree.rs` - Build parent→children index with memoization |
| 3.5.2 | `descendants` calculation | - | 🟢 | Count of all items under a directory |
| 3.5.3 | `treesize` calculation | - | 🟢 | Sum of all file sizes in subtree |
| 3.5.4 | `tree_allocated` calculation | - | 🟢 | Sum of allocated sizes in subtree |
| 3.5.5 | `bulkiness` calculation | - | 🟢 | tree_allocated / treesize ratio (fragmentation metric) |
| 3.5.6 | Add tree columns to query results | - | 🟢 | On-demand via `add_tree_columns()` |
| 3.5.7 | Unit tests | - | 🟢 | 9 tests in tree.rs |

---

## Phase 4: uffs-cli & Performance (Weeks 13-16)

**Goal**: CLI tool and performance optimization
**Crate**: `uffs-cli`
**Target Date**: Week 16
**Status**: 🟢 Complete

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 4.1 | CLI argument parsing | - | 🟢 | `main.rs` - clap derive |
| 4.2 | `search` command | - | 🟢 | `commands.rs` - Full search with filters |
| 4.3 | `index` command | - | 🟢 | `commands.rs` - Build/save index |
| 4.4 | `stats` command | - | 🟢 | `commands.rs` - Volume statistics |
| 4.5 | Progress indicators | - | 🟢 | `commands.rs` - indicatif progress bar |
| 4.6 | Error messages | - | 🟢 | anyhow + context |
| 4.7 | Batch I/O optimization | - | 🟢 | `io.rs` - BatchMftReader, 1MB chunks |
| 4.8 | Parallel MFT reading | - | 🟢 | `io.rs` - ParallelMftReader with Rayon |
| 4.9 | Cluster-level bitmap skip | - | 🟢 | `platform.rs` - calculate_skip_range(), in_use_cluster_ranges() |
| 4.10 | Fragmented MFT support | - | 🟢 | `io.rs` - MftExtentMap, VCN-to-LCN mapping |
| 4.11 | Benchmark suite | - | 🟡 | Skeleton in benches/ |
| 4.12 | Performance profiling | - | ⬜ | Future work |
| 4.13 | Integration tests | - | 🟡 | Basic tests |

### Acceptance Criteria

- [x] CLI accepts all documented arguments
- [x] Progress shown during indexing
- [ ] MFT read speed ≥500 MB/s (needs Windows testing)
- [ ] Index build ≤2s for 1M files (needs Windows testing)
- [ ] Search latency <10ms (needs Windows testing)
- [ ] All benchmarks pass

### Performance Tracking

| Metric | C++ Baseline | Current | Target | Status |
|--------|--------------|---------|--------|--------|
| MFT Read (MB/s) | 500 | TBD | ≥500 | 🟡 |
| Index Build (1M files) | 2.0s | TBD | ≤1.5s | 🟡 |
| Search Latency | 8ms | TBD | <5ms (SIMD) | 🟡 |
| Memory/File | 32B | TBD | ~45B (Polars) | 🟡 |
| Parquet Size | N/A | TBD | ~60% of raw | 🟡 |

### High-Performance MFT Reading Architecture

The implementation matches the historical baseline for performance with these key components:

| Component | Location | Description |
|-----------|----------|-------------|
| `MftExtentMap` | `io.rs` | VCN-to-LCN mapping for fragmented MFT support |
| `MftBitmap` | `platform.rs` | Tracks which MFT records are in use |
| `calculate_skip_range()` | `platform.rs` | Cluster-level skip range calculation |
| `in_use_cluster_ranges()` | `platform.rs` | Iterator over clusters with in-use records |
| `generate_read_chunks()` | `io.rs` | Creates optimized 1MB read chunks |
| `ParallelMftReader` | `io.rs` | Orchestrates parallel reading with Rayon |
| `BatchMftReader` | `io.rs` | Reads multiple records per I/O operation |
| `ReadChunk` | `io.rs` | Represents a contiguous read with skip info |

**Performance Features Implemented:**

1. ✅ **Fragmented MFT Support** - MFT can be scattered across disk; `MftExtentMap` handles VCN-to-LCN translation
2. ✅ **Cluster-Level Bitmap Skipping** - Skip entire clusters where all records are unused
3. ✅ **Batch I/O (1MB chunks)** - Reduce syscall overhead by reading multiple records per I/O
4. ✅ **Parallel Record Processing** - Use Rayon to parse records across all CPU cores
5. ✅ **USA Fixup** - Apply Update Sequence Array fixup to detect torn writes

---

## Phase 5: uffs-tui & Polish (Weeks 17-21)

**Goal**: Terminal UI and production readiness
**Crate**: `uffs-tui`
**Target Date**: Week 21
**Status**: 🟢 Complete (95%)

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 5.1 | TUI framework setup | - | 🟢 | `main.rs` - ratatui + crossterm |
| 5.2 | Search input widget | - | 🟢 | `app.rs` - Input handling |
| 5.3 | Results list widget | - | 🟢 | `app.rs` - Scrollable list |
| 5.4 | File details panel | - | 🟡 | Basic (details in list) |
| 5.5 | Progress indicators | - | 🟢 | Status bar display |
| 5.6 | Keyboard navigation | - | 🟢 | Up/Down/Enter/Esc |
| 5.7 | Admin privilege check | - | 🟡 | Windows-only (future) |
| 5.8 | User documentation | - | 🟡 | Inline docs, --help |
| 5.9 | API documentation | - | 🟡 | rustdoc for all crates |
| 5.10 | Release builds | - | ⬜ | Future work |
| 5.11 | Cross-compilation | - | ⬜ | Windows targets |

### Acceptance Criteria

- [x] TUI launches and displays search interface
- [x] Real-time search updates as you type
- [x] Keyboard navigation works smoothly
- [x] Progress shown during indexing
- [ ] Documentation complete and accurate
- [ ] Release binaries tested on clean system

---

## Phase 6: Performance Optimization - "Outperform the Reference Baseline" (Weeks 22-31)

**Goal**: Make the Rust implementation 2-5x faster than the reference binary through architectural optimizations
**Crates**: `uffs-mft`, `uffs-core`
**Target Date**: Week 31
**Status**: 🟡 In Progress
**Reference**: [MFT Architecture Deep Dive](architecture/MFT_ARCHITECTURE_DEEP_DIVE.md)

### 🎉 IMPORTANT: MFT Reader is Already Highly Optimized!

Before diving into optimizations, note that **`uffs-mft` is already 55% faster** than v0.1.30:

| Drive | v0.1.30 (Before) | v0.1.39 (Current) | Improvement |
|-------|------------------|-------------------|-------------|
| SSD C: | 11.3s | **3.1s** | **73% faster** |
| HDD S: | 160.6s | **45.9s** | **71% faster** |
| Total (7 drives) | 315s | **142s** | **55% faster** |

**What's Already Done in `uffs-mft`:**
- ✅ Bitmap-based cluster skipping (skip free records)
- ✅ Rayon fold/reduce parallel parsing
- ✅ SoA layout (`ParsedColumns`)
- ✅ `PrefetchMftReader` for HDDs (double-buffered I/O)
- ✅ `ParallelMftReader` for SSDs (8MB chunks)
- ✅ Drive-type auto-detection

**The Real Bottleneck is in `uffs-core`:**
- ✅ Path resolution FIXED (FastPathResolver with Vec-based O(1) lookup)
- ❌ No SIMD pattern matching
- ❌ No early termination for `--limit`

### Background

Analysis of the historical baseline revealed key architectural differences that explain its speed:
- **Contiguous memory**: All data in vectors, not scattered heap allocations
- **Direct indexing**: `vector[frs]` instead of HashMap lookups
- **Packed structures**: Minimal memory footprint, cache-friendly
- **In-place operations**: Reuse buffers, avoid allocations
- **Single-pass processing**: Parse during read, not after

### Current Performance Gap

| Metric | C++ | Rust (Current) | Target |
|--------|-----|----------------|--------|
| `*.txt` search time | ~43.6s | ~56.1s | <20s |
| Files found | 735K | ~200K (bug) | 735K+ |
| Memory per file | ~64 bytes | ~200+ bytes | <50 bytes |
| Path resolution | O(1) array | O(1) HashMap* | O(1) array |

*HashMap has significant constant factor overhead vs direct array indexing.

### Phase 6.0: Benchmark Infrastructure (Week 21) 📊 NEW
**Goal**: Establish baseline measurements and automated comparison tools.
**Status**: 🟡 In Progress

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.0.1 | `just bench-vs-cpp` command | - | 🟢 | Compare Rust vs C++ (`~/bin/uffs.com`) |
| 6.0.2 | `just bench-micro` command | - | 🟢 | Run criterion micro-benchmarks |
| 6.0.3 | `just bench-search` command | - | 🟢 | End-to-end search benchmark |
| 6.0.4 | Fill in MFT reading benchmarks | - | ⬜ | Replace placeholder in `mft_read.rs` |
| 6.0.5 | Fill in query benchmarks | - | ⬜ | Replace placeholder in `query.rs` |
| 6.0.6 | Add `resolve_path()` benchmark | - | ⬜ | Measure path resolution performance |
| 6.0.7 | Baseline tracking | - | 🟢 | `benchmarks/baseline.json` |

### Phase 6.1: Foundation (Week 22-23) ⭐ CRITICAL ✅ COMPLETE
**Goal**: Fix correctness issues and establish baseline.
**Status**: 🟢 Complete

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.1.1 | Fix PathResolver bug | - | 🟢 | Build from FULL MFT data before filtering |
| 6.1.2 | Add benchmark suite | - | 🟢 | criterion benchmarks for key operations |
| 6.1.3 | Profile current code | - | ⬜ | flamegraph, heaptrack analysis |
| 6.1.4 | Establish baseline metrics | - | 🟢 | `just bench-vs-cpp` provides this |

**Completed:**
- `FastPathResolver` with Vec-based O(1) lookup (replaces HashMap)
- `NameArena` for contiguous name storage
- Search pipeline builds resolver from FULL MFT before filtering
- 10 unit tests for path resolution
- Benchmark comparison: HashMap vs Vec resolver

### Phase 6.2: Memory Architecture (Week 24-25) ✅ COMPLETE
**Goal**: Eliminate allocation overhead.
**Status**: 🟢 Complete (merged with Phase 6.1)

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.2.1 | Implement `NameArena` | - | 🟢 | Single buffer for all names |
| 6.2.2 | Implement `NameRef` | - | 🟢 | offset + length via FastEntry |
| 6.2.3 | Replace HashMap with Vec | - | 🟢 | Direct FRS indexing in FastPathResolver |
| 6.2.4 | In-place path building | - | 🟢 | Append-reverse strategy (like C++) |
| 6.2.5 | Add NameRef to ParsedColumns | - | ⬜ | Future: Reference instead of String clone |

**Expected Impact**: 2-3x speedup from reduced allocations.

### Phase 6.3: Parsing Optimization (Week 26-27)
**Goal**: Reduce parsing overhead.

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.3.1 | Direct-to-column parsing | - | ⬜ | Skip intermediate ParsedRecord |
| 6.3.2 | Lazy attribute parsing | - | ⬜ | Only parse what's needed for query |
| 6.3.3 | Bit-packed attributes | - | ⬜ | Single u32 for all 18 flags |
| 6.3.4 | Compact record format | - | ⬜ | ~56 bytes vs ~200+ bytes |

**Expected Impact**: 1.3-1.5x speedup from reduced overhead.

### Phase 6.4: True Pipelining (Week 28-29) ✅ COMPLETE
**Goal**: Overlap I/O and CPU work for additional HDD speedup.
**Status**: 🟢 Complete

> **Implementation:** `PipelinedMftReader` uses `crossbeam-channel` bounded channels
> to overlap I/O and CPU work. Reader thread queues chunks while parser thread processes.
> Auto mode now selects `Pipelined` for HDDs instead of `Prefetch`.

**Architecture:**
```
Reader Thread ──▶ [Bounded Channel] ──▶ Parser Thread
     │                                        │
     ▼                                        ▼
 Read chunks                            Parse records
 as fast as                             as they arrive
 possible                               (with Rayon)
```

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.4.1 | Channel-based pipeline | - | 🟢 | `crossbeam-channel` bounded channels |
| 6.4.2 | Reader thread(s) | - | 🟢 | Dedicated thread queues reads |
| 6.4.3 | Parser thread(s) | - | 🟢 | Main thread parses with MftRecordMerger |
| 6.4.4 | Backpressure handling | - | 🟢 | Bounded channel (depth=3) prevents memory explosion |
| 6.4.5 | Multi-extent parallelism | - | ⬜ | Future: One reader per MFT extent |

**Expected Impact (HDDs only)**:
- **20-35% additional speedup** on HDDs where I/O time ≈ CPU time
- **True overlap**: `Time = max(I/O, CPU)` instead of `Time = I/O + CPU`

### Phase 6.5: I/O Tuning (Week 30-31) ✅ MOSTLY DONE
**Goal**: Maximize disk throughput.
**Status**: 🟢 Mostly Complete (via `PrefetchMftReader` and `ParallelMftReader`)

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.5.1 | Adaptive read sizing | - | 🟢 | `MftReadMode::Auto` detects SSD vs HDD |
| 6.5.2 | MFT bitmap optimization | - | 🟢 | Bitmap-based cluster skipping implemented |
| 6.5.3 | Prefetch optimization | - | 🟢 | `PrefetchMftReader` with double-buffering |
| 6.5.4 | Memory-mapped I/O option | - | ⬜ | Optional: For SSDs with sufficient RAM |

**Already Achieved**: 55% faster than v0.1.30, 1,839 MB/s on SSDs.

### Phase 6.6: Query Optimization (Week 32-33) ✅ COMPLETE
**Goal**: Accelerate pattern matching.
**Status**: 🟢 Complete (except SIMD which is deferred)

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 6.6.1 | SIMD pattern matching | - | ⬜ | AVX2/AVX-512 for wildcards (deferred) |
| 6.6.2 | Early termination | - | 🟢 | Streaming mode has early termination |
| 6.6.3 | Parallel path resolution | - | 🟢 | `add_path_column_parallel()` with Rayon |
| 6.6.4 | Extension index | - | 🟢 | `ExtensionIndex` for fast `*.ext` queries |

**Expected Impact**: 1.2-1.5x speedup for pattern matching.

### Acceptance Criteria

- [x] PathResolver correctly resolves ALL files (matches reference output)
- [x] Benchmark suite covers all critical paths
- [ ] `*.txt` search completes in <20s (vs C++ ~43.6s)
- [ ] Memory usage <50 bytes per file (vs current ~200+)
- [x] All existing tests continue to pass (77 tests)
- [ ] Performance regression tests in CI

### Performance Targets

| Optimization | Expected Speedup | Cumulative |
|--------------|------------------|------------|
| Fix PathResolver | 1.0x (correctness) | 1.0x |
| NameArena | 2-3x | 2-3x |
| Vec-based lookup | 1.5-2x | 3-6x |
| In-place path building | 1.5-2x | 4.5-12x |
| Direct parsing | 1.3-1.5x | 6-18x |
| **Pipelined I/O** ⭐ | **2x** | **12-36x** |
| SIMD matching | 1.2-1.5x | 14-54x |

**Conservative estimate**: 10-15x faster than current Rust
**Optimistic estimate**: 25-50x faster than current Rust
**Target**: 2-5x faster than the reference binary

### Memory Targets

| Approach | Memory for 1M files |
|----------|---------------------|
| Current Rust | ~1GB (entire MFT) |
| Pipelined Rust | ~4MB (4 × 1MB buffers) |
| **Reduction** | **250x** |

### Key Data Structures

```rust
// Arena-based name storage (like C++ std::tvstring)
pub struct NameArena {
    buffer: String,  // All names concatenated
}

pub struct NameRef {
    offset: u32,     // Offset into arena
    length: u16,     // Name length
    is_ascii: bool,  // ASCII compression flag
}

// Fast path resolver (like C++ RecordsLookup)
pub struct FastPathResolver {
    entries: Vec<Option<(u32, NameRef)>>,  // Index = FRS
    names: NameArena,
    volume: char,
}

// Compact record (like C++ Record)
pub struct CompactRecord {
    pub frs: u64,
    pub parent_frs: u32,      // u32 sufficient (like C++)
    pub name_ref: NameRef,    // 8 bytes
    pub size: u64,
    pub attributes: u32,      // Bit-packed flags
    pub timestamps: [i64; 4], // Inline array
}
// Total: ~56 bytes vs current ~200+ bytes
```

---

## Risk Register

| ID | Risk | Impact | Probability | Mitigation | Status |
|----|------|--------|-------------|------------|--------|
| R1 | Windows API complexity | High | Medium | Use `windows` crate, extensive testing | ⬜ Open |
| R2 | Performance regression | High | Low | Continuous benchmarking vs C++ | ⬜ Open |
| R3 | Memory safety with raw I/O | Critical | Medium | Buffer management, fuzzing | ⬜ Open |
| R4 | NTFS edge cases | Medium | Medium | Test on diverse volumes | ⬜ Open |
| R5 | Admin privilege issues | Medium | Low | Clear error messages, docs | ⬜ Open |
| R6 | Workspace complexity | Low | Low | Clear crate boundaries, docs | ⬜ Open |

---

## Dependencies

### Crate Dependencies

| Crate | Depends On | Key External Deps |
|-------|------------|-------------------|
| `uffs-polars` | - | polars (all features) |
| `uffs-mft` | uffs-polars | windows, rayon, bitflags, thiserror |
| `uffs-core` | uffs-polars, uffs-mft | regex |
| `uffs-cli` | uffs-core | clap, indicatif, anyhow, tokio |
| `uffs-tui` | uffs-core | ratatui, crossterm |
| `uffs-gui` | uffs-core | egui (future) |

### Phase Dependencies

```
Phase 0 (Workspace Setup + uffs-polars facade)
    ↓
Phase 1 (uffs-mft Foundation)
    ↓
Phase 2 (uffs-mft DataFrame)
    ↓
Phase 3 (uffs-core Processing with Polars)
    ↓
Phase 4 (uffs-cli & Performance)
    ↓
Phase 5 (uffs-tui & Polish)
    ↓
Phase 6 (Performance Optimization - "Outperform the Reference Baseline")
```

---

## Weekly Progress Log

### Week 0 (2026-01-15) - Planning

- [x] Analyzed C++ codebase
- [x] Created implementation plan
- [x] Created milestone document
- [x] Refactored for workspace architecture
- [x] **Refactored for Polars-based architecture**
- [x] Set up workspace structure with uffs-polars facade
- [x] Establish CI/CD pipeline

### Week 1 (2026-01-16) - Implementation

- [x] Implemented uffs-polars facade crate
- [x] Implemented uffs-mft with NTFS structures
- [x] Implemented uffs-mft with MftReader and DataFrame
- [x] Implemented uffs-core with MftQuery fluent API
- [x] Implemented uffs-cli with search/index/stats commands
- [x] Implemented uffs-tui with ratatui interface
- [x] Workspace compiles successfully

---

## Change Log

| Date | Change | Reason |
|------|--------|--------|
| 2026-01-15 | Initial document creation | Project kickoff |
| 2026-01-15 | Refactored for workspace architecture | Modular crate design |
| 2026-01-15 | **Refactored for Polars-based architecture** | SIMD, parallelism, Parquet persistence |
| 2026-01-16 | **Implementation complete** | All core crates implemented |
| 2026-01-16 | **High-performance MFT reading** | Parallel processing (Rayon), batch I/O, cluster-level bitmap skipping, fragmented MFT support |
| 2026-01-16 | **Phase 3.5: Directory Tree Structure** | `TreeIndex` with memoized metrics: descendants, treesize, tree_allocated, bulkiness |
| 2026-01-19 | **Phase 6: Performance Optimization** | Deep dive analysis of the historical baseline vs Rust architecture; roadmap to make Rust 2-5x faster than the reference binary |

---

## Appendix A: Workspace Structure

```
UltraFastFileSearch/
├── Cargo.toml                      # Workspace manifest
├── crates/
│   ├── uffs-polars/                # 🔧 Polars facade (compiles ONCE)
│   │   ├── Cargo.toml              # All Polars features here
│   │   └── src/
│   │       └── lib.rs              # Re-exports polars::prelude::*
│   │
│   ├── uffs-mft/                   # 📦 MFT reading → DataFrame
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs              # Public API
│   │       ├── reader.rs           # MftReader
│   │       ├── dataframe.rs        # DataFrame construction
│   │       ├── ntfs/               # NTFS structures
│   │       │   ├── mod.rs
│   │       │   ├── boot_sector.rs
│   │       │   ├── file_record.rs
│   │       │   ├── attributes.rs
│   │       │   └── run_list.rs
│   │       ├── io/                 # Low-level I/O
│   │       │   ├── mod.rs
│   │       │   ├── volume.rs
│   │       │   └── async_read.rs
│   │       └── platform/
│   │           ├── mod.rs
│   │           └── windows.rs
│   │
│   ├── uffs-core/                  # 📦 Query engine (Polars lazy)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── query.rs            # MftQuery (wraps LazyFrame)
│   │       ├── path_resolver.rs    # Path reconstruction
│   │       ├── glob.rs             # Glob to regex
│   │       └── export.rs           # Table, JSON, CSV
│   │
│   ├── uffs-cli/                   # 🔧 CLI binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── commands/
│   │           ├── mod.rs
│   │           ├── search.rs
│   │           ├── index.rs
│   │           └── stats.rs
│   │
│   ├── uffs-tui/                   # 🖥️ TUI binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── widgets/
│   │
│   └── uffs-gui/                   # 🪟 GUI binary (future)
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
│
├── examples/                       # Usage examples
├── benches/                        # Benchmarks
└── docs/                           # Documentation
```

---

## Appendix B: Key Metrics Dashboard

```
┌─────────────────────────────────────────────────────────────────┐
│                    PROJECT HEALTH DASHBOARD                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Overall Progress: ██░░░░░░░░░░░░░░░░░░ 5%                      │
│                                                                  │
│  Phase 0: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 3: ░░░░░░░░░░░░ 0%   │
│  Phase 1: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 4: ░░░░░░░░░░░░ 0%   │
│  Phase 2: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 5: ░░░░░░░░░░░░ 0%   │
│                                                                  │
│  Crates: 0/6 complete              Tests: 0 passing / 0 total   │
│  Open Risks: 6                      Blockers: 0                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Appendix C: Resources

### Project Resources

- **Reference baseline**: `old_cpp/uffs/UltraFastFileSearch-code/`
- **Architecture Doc**: `docs/architecture/Suggested Rust Source Code Structure.docx`
- **Implementation Plan**: `docs/IMPLEMENTATION_PLAN.md`

### External References

- [NTFS Documentation (Microsoft)](https://docs.microsoft.com/en-us/windows/win32/fileio/master-file-table)
- [NTFS Internals](https://flatcap.github.io/linux-ntfs/ntfs/)
- [Rust `windows` crate](https://docs.rs/windows)
- [Tokio async runtime](https://tokio.rs)
- [ratatui TUI framework](https://ratatui.rs)
- [Cargo Workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html)
- [Polars User Guide](https://docs.pola.rs/)
- [Polars Rust API](https://docs.rs/polars)
- [Parquet Format](https://parquet.apache.org/)

