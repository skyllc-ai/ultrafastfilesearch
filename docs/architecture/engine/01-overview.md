# UFFS Engine Architecture Overview

## Executive Summary

Ultra Fast File Search (UFFS) is a high-performance Windows file search utility written in Rust that achieves its speed by directly reading the NTFS Master File Table (MFT) rather than using standard Windows file enumeration APIs. On NVMe drives, UFFS indexes millions of files in under 6 seconds and delivers sub-second pattern matching across the entire file system.

This document series provides a comprehensive architectural reference for the **Rust engine** (v0.3.62, Edition 2024). A developer reading these documents should be able to understand, maintain, extend, or reimplement the core engine from scratch.

---

## System Context

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              User Interface                                │
│  ┌──────────────────────────┐  ┌─────────────────┐  ┌──────────────────┐  │
│  │  uffs-cli (clap v4)      │  │  uffs-tui       │  │  uffs-gui        │  │
│  │  Search, MFT dump, diag  │  │  (ratatui)      │  │  (future)        │  │
│  └────────────┬─────────────┘  └────────┬────────┘  └────────┬─────────┘  │
└───────────────┼─────────────────────────┼────────────────────┼─────────────┘
                │                         │                    │
                ▼                         ▼                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        uffs-core  (Query Engine)                            │
│  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────────────┐   │
│  │ Pattern Parsing  │  │ Index Search    │  │  Path Resolution         │   │
│  │ Glob/Regex/Lit   │  │ Extension Index │  │  Tree Metrics            │   │
│  └────────┬────────┘  └────────┬────────┘  └─────────────┬────────────┘   │
└───────────┼────────────────────┼─────────────────────────┼────────────────┘
            │                    │                         │
            ▼                    ▼                         ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        uffs-mft  (MFT Engine)                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────────────┐   │
│  │ MftReader        │  │ MftIndex        │  │  NTFS Structures         │   │
│  │ (Read Pipeline)  │  │ (In-memory DB)  │  │  (On-disk Layouts)       │   │
│  └────────┬────────┘  └────────┬────────┘  └─────────────┬────────────┘   │
│  ┌────────┴────────┐  ┌────────┴────────┐  ┌─────────────┴────────────┐   │
│  │ I/O Readers     │  │ Record Parsers  │  │  Platform Abstraction     │   │
│  │ IOCP, Pipelined │  │ Base, Extension │  │  Volume, Bitmap, Extents  │   │
│  └────────┬────────┘  └────────┬────────┘  └─────────────┬────────────┘   │
└───────────┼────────────────────┼─────────────────────────┼────────────────┘
            │                    │                         │
            ▼                    ▼                         ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                     uffs-polars  (DataFrame Facade)                         │
│  Compilation-isolation wrapper around Polars for analytics & export         │
└─────────────────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                      Windows Kernel / NTFS                                  │
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │  Direct Volume Access (CreateFile on \\.\C:)                            ││
│  │  I/O Completion Ports (IOCP) for async reads                            ││
│  │  FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_GET_RETRIEVAL_POINTERS              ││
│  │  DeviceIoControl for MFT bitmap and extent map                          ││
│  └─────────────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Workspace Crate Map

| Crate | Role | Key Types |
|-------|------|-----------|
| **`uffs-mft`** | MFT reading engine — I/O, parsing, indexing | `MftReader`, `MftIndex`, `FileRecord`, `VolumeHandle` |
| **`uffs-core`** | Query engine — pattern matching, search, output | `ParsedPattern`, `IndexQuery`, `PathResolver`, `TreeIndex` |
| **`uffs-cli`** | CLI binary — `uffs` command | clap-based argument parsing |
| **`uffs-tui`** | Terminal UI (ratatui) | Interactive file browser |
| **`uffs-gui`** | GUI (future) | Windows native UI |
| **`uffs-polars`** | Polars facade (compilation isolation) | `DataFrame`, `LazyFrame` re-exports |
| **`uffs-diag`** | Diagnostic tools (workspace-only, not shipped) | MFT analysis and validation |

### Dependency Graph

```
uffs-cli ──► uffs-core ──► uffs-mft ──► uffs-polars ──► polars (git)
                                    └──► windows (0.62)
                                    └──► tokio (async runtime)
                                    └──► rayon (parallel parsing)
                                    └──► zerocopy (zero-copy NTFS structs)
```

---

## Core Engine: `uffs-mft`

The `uffs-mft` crate is the heart of UFFS. It contains:

### Module Structure

```
uffs-mft/src/
├── lib.rs                  # Public API re-exports
├── reader.rs               # MftReader orchestration
│   ├── reader/
│   │   ├── index_read.rs       # Lean-index read pipeline (fast path)
│   │   ├── dataframe_read.rs   # DataFrame read pipeline
│   │   ├── multi_drive/        # Multi-drive parallel orchestration
│   │   ├── index_cache.rs      # Parquet/zstd cache layer
│   │   ├── persistence.rs      # Save/load MFT data
│   │   ├── read_mode.rs        # Drive-type-aware mode selection
│   │   ├── benchmark.rs        # Built-in benchmarking
│   │   └── stats.rs            # Progress tracking
│   │
├── io.rs                   # I/O layer
│   ├── io/
│   │   ├── readers/
│   │   │   ├── iocp/           # I/O Completion Port reader (production)
│   │   │   ├── pipelined.rs    # I/O + CPU overlap pipeline
│   │   │   ├── parallel/       # Rayon-based parallel reader
│   │   │   ├── streaming.rs    # Sequential streaming reader
│   │   │   └── prefetch.rs     # Double-buffered prefetch reader
│   │   ├── parser/
│   │   │   ├── index.rs            # Base record → MftIndex parser
│   │   │   ├── index_extension.rs  # Extension record → MftIndex parser
│   │   │   └── unified.rs         # Unified base+ext single-pass parser
│   │   ├── chunking.rs        # Extent-aware read chunk planning
│   │   ├── extent_map.rs      # MFT extent/fragment mapping
│   │   └── aligned_buffer.rs  # Sector-aligned I/O buffers
│   │
├── index.rs                # Lean in-memory index
│   ├── index/
│   │   ├── types.rs            # FileRecord, StandardInfo, LinkInfo, etc.
│   │   ├── model.rs            # MftIndex container + ChildInfo
│   │   ├── base.rs             # Constructors, lookup, stats
│   │   ├── builder.rs          # Index building from parsed records
│   │   ├── tree.rs             # Tree metrics (treesize, descendants)
│   │   ├── merge.rs            # Fragment merging for multi-chunk reads
│   │   ├── extensions.rs       # Extension interning table
│   │   ├── paths.rs            # Path resolution (FRS → full path)
│   │   ├── dataframe.rs        # MftIndex → Polars DataFrame conversion
│   │   └── storage/            # Parquet serialization
│   │
├── ntfs/                   # NTFS on-disk structures (cross-platform)
│   ├── boot_sector.rs      # NTFS boot sector parsing
│   ├── records.rs          # FILE record header, attribute iteration, USA fixup
│   ├── metadata.rs         # $STANDARD_INFORMATION, $FILE_NAME, etc.
│   └── data_runs.rs        # Non-resident data run decoding
│
├── parse/                  # MFT record parsing (cross-platform)
│   ├── full.rs             # Full record parsing to columnar format
│   ├── direct_index.rs     # Direct-to-index base record parser
│   ├── direct_index_extension.rs  # Direct-to-index extension parser
│   ├── fixup.rs            # USA fixup application
│   ├── index_helpers.rs    # Shared parsing utilities
│   └── attribute_helpers.rs # Attribute extraction helpers
│
├── platform/               # Platform abstraction
│   ├── volume.rs           # VolumeHandle, IOCP constants
│   ├── bitmap.rs           # MFT bitmap (record in-use tracking)
│   ├── extents.rs          # MFT extent list
│   └── system.rs           # Drive detection, elevation check
│
├── raw.rs                  # Raw MFT file loading (offline/cross-platform)
├── raw_iocp.rs             # IOCP capture format (save/replay)
├── cache.rs                # Index caching (Parquet + zstd)
├── tree_metrics.rs         # Tree metric computation engine
├── usn.rs                  # USN Journal reading
└── flags.rs                # FileFlags bitflags
```

---

## Data Flow

### End-to-End Search Pipeline

```
1. VOLUME DISCOVERY
   └─► detect_ntfs_drives() → Vec<(char, DriveType)>
       Filter NTFS volumes, detect NVMe/SSD/HDD
       │
2. VOLUME ACCESS (per drive, parallel)
   └─► VolumeHandle::open('C') → direct \\.\C: access
       Requires Administrator privileges
       │
3. MFT METADATA
   ├─► FSCTL_GET_NTFS_VOLUME_DATA → cluster_size, record_size, mft_capacity
   ├─► FSCTL_GET_RETRIEVAL_POINTERS → $MFT extent map (fragments)
   └─► FSCTL_GET_RETRIEVAL_POINTERS → $MFT::$BITMAP extent map
       │
4. BITMAP-GUIDED CHUNK PLANNING
   ├─► Read $MFT::$BITMAP → which records are in-use
   ├─► generate_read_chunks() → Vec<ReadChunk>
   │   For each extent: scan bitmap to find skip_begin / skip_end
   └─► Eliminates 50-80% of I/O (deleted records skipped)
       │
5. IOCP-DRIVEN MFT READING
   ├─► IoCompletionPort associates volume handle
   ├─► Sliding window: 2+ concurrent async ReadFile operations
   ├─► Each completion triggers:
   │   a. parse_record_to_index() for each 1KB record in buffer
   │   b. Queue next read (maintains concurrency)
   └─► Result: populated MftIndex
       │
6. POST-PROCESSING
   ├─► compute_tree_metrics() → treesize, descendants, tree_allocated
   ├─► build_extension_index() → O(1) *.ext lookups
   └─► Path resolution cache built lazily
       │
7. PATTERN MATCHING
   ├─► ParsedPattern::parse("*.rs") → drive, pattern_type, is_path
   ├─► compile_index_pattern() → IndexPattern (optimized matcher)
   └─► Scan MftIndex records → matching results
       │
8. OUTPUT
   ├─► Path resolution: FRS → full path string
   ├─► Formatting: table, CSV, JSON
   └─► Streaming to stdout (output-as-ready for multi-drive)
```

---

## Key Design Decisions

### 1. Lean Index vs DataFrame

UFFS maintains **two paths** for MFT data:

- **Lean `MftIndex`** (fast path): Compact `Vec<FileRecord>` with O(1) FRS lookup. Used for interactive search. ~100-200 bytes per file. Built directly during I/O parsing — no intermediate allocation.

- **Polars `DataFrame`** (analytics path): Columnar format for complex queries, aggregation, export. Built on demand from `MftIndex` via `to_dataframe()`.

The lean index is **15-20× faster** to build than the DataFrame path because it avoids column construction overhead.

### 2. Cross-Platform Core, Windows-Only I/O

NTFS structure definitions (`ntfs/`) and record parsing (`parse/`) are **cross-platform** — they work on macOS and Linux for testing and offline MFT analysis. Live volume access (`io/readers/`, `platform/volume.rs`) is Windows-only behind `#[cfg(windows)]`.

This enables:
- Development and testing on macOS/Linux
- Offline MFT analysis from captured `.mft` files
- CI on GitHub Actions (Linux runners)

### 3. Drive-Type-Aware I/O

UFFS automatically detects the storage type and selects optimal I/O parameters:

| Drive Type | Concurrency | Chunk Size | Read Mode | Parallel Parse |
|------------|-------------|------------|-----------|----------------|
| **NVMe** | 32 | 4 MB | Sliding IOCP Inline | Yes |
| **SSD** | 8 | 2 MB | Sliding IOCP Inline | No |
| **HDD** | 2-6* | 1 MB | Sliding IOCP Inline | No |

\* HDD concurrency is extent-aware: fewer extents → higher concurrency (less seeking).

### 4. Extension Records and Hardlinks

NTFS files with many attributes (>~700 bytes of metadata) spill into **extension records**. UFFS handles two strategies:

- **Inline merge** (default): Extension attributes merged during parsing via `parse_record_to_index` + `parse_extension_to_index`. Fast, handles 99%+ of files.
- **Unified parser**: Single-pass processor (`process_record`) that handles base and extension records identically in a single attribute loop. Produces deterministic output regardless of record processing order.

### 5. mimalloc Global Allocator

UFFS uses `mimalloc` as the global allocator. For allocation-heavy workloads (building indexes with millions of records), mimalloc reduces fragmentation and improves throughput by ~10-15%.

---

## Memory Layout

The index uses a compact, cache-friendly memory layout:

```
┌───────────────────────────────────────────────────────────────┐
│ names: String                                                 │
│ ┌─────┬──────┬─────┬──────┬─────┬──────┬─────────────────┐   │
│ │file1│.txt  │file2│.rs   │dir1 │file3 │.h   ...         │   │
│ └─────┴──────┴─────┴──────┴─────┴──────┴─────────────────┘   │
│   ▲ offset                                                    │
└───┼───────────────────────────────────────────────────────────┘
    │
┌───┴───────────────────────────────────────────────────────────┐
│ records: Vec<FileRecord>  (224 bytes each)                    │
│ ┌────────────────────────────────────────────────────────────┐│
│ │ [0]: frs, stdinfo{created,modified,flags}, first_name{    ││
│ │       parent_frs, name→offset}, first_stream{size},       ││
│ │       name_count, stream_count, descendants, treesize     ││
│ ├────────────────────────────────────────────────────────────┤│
│ │ [1]: ...                                                   ││
│ └────────────────────────────────────────────────────────────┘│
└───────────────────────────────────────────────────────────────┘
    │
    │ frs_to_idx[frs] → record index (O(1) lookup)
    ▼
┌───────────────────────────────────────────────────────────────┐
│ frs_to_idx: Vec<u32>  (sparse array indexed by FRS)           │
│ ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐     │
│ │  0  │  1  │ MAX │  2  │  3  │ MAX │  4  │ ... │     │     │
│ └─────┴─────┴─────┴─────┴─────┴─────┴─────┴─────┴─────┘     │
│ (MAX = NO_ENTRY = u32::MAX, means FRS not present)            │
└───────────────────────────────────────────────────────────────┘

Overflow chains (for hardlinks and ADS):
┌────────────────────┐  ┌────────────────────┐
│ links: Vec<LinkInfo>│  │ streams: Vec<      │
│ (overflow hardlinks)│  │   IndexStreamInfo> │
│ next→next→NO_ENTRY  │  │ next→next→NO_ENTRY │
└────────────────────┘  └────────────────────┘

Directory tree:
┌──────────────────────────┐
│ children: Vec<ChildInfo>  │
│ (linked list per parent)  │
│ first_child→next→NO_ENTRY │
└──────────────────────────┘
```

### Per-Record Memory Budget

| Field | Size | Notes |
|-------|------|-------|
| `frs` | 8 B | Primary key |
| `sequence_number` | 2 B | Forensic |
| `namespace` + `forensic_flags` | 2 B | Packed flags |
| `lsn` | 8 B | Log sequence number |
| `reparse_tag` | 4 B | Symlink/junction type |
| `base_frs` | 8 B | Extension record link |
| `stdinfo` | 48 B | Timestamps (4×i64) + flags + USN + security/owner |
| `name_count` + `stream_count` + `total_stream_count` | 6 B | Counts |
| `first_internal_stream` + `first_child` | 8 B | Linked list heads |
| `first_name` (LinkInfo) | 24 B | Inline primary name |
| `first_stream` (IndexStreamInfo) | 29 B | Inline primary stream |
| `fn_*` timestamps | 32 B | $FILE_NAME timestamps |
| `descendants` + `treesize` + `tree_allocated` | 20 B | Tree metrics |
| `internal_streams_*` | 16 B | Internal stream sizes |
| **Total** | **~224 B** | Per file/directory |

For 2M files: ~448 MB for records + ~46 MB for names ≈ **~500 MB total**.

---

## Performance Summary

### Benchmarks (v0.3.54, Cold Start)

| Drive | Type | Files | Time |
|-------|------|-------|------|
| C: | NVMe | 2.3M | 8.2s |
| D: | HDD | 1.5M | 30.7s |
| F: | NVMe | 1.1M | 5.1s |
| S: | HDD | 3.2M | 71.6s |

### Why UFFS is Fast

1. **Direct MFT reading**: Bypasses Windows file enumeration APIs entirely
2. **IOCP async I/O**: Overlaps disk reads with CPU parsing
3. **Bitmap skip**: Eliminates 50-80% of I/O by skipping deleted records
4. **Inline parsing**: Records parsed directly into final index (no intermediate copies)
5. **Cache-friendly layout**: Contiguous `Vec<FileRecord>` with bit-packed fields
6. **mimalloc**: Reduces allocation overhead for millions of records
7. **Drive-type tuning**: NVMe gets 32× concurrency, HDD gets 2-6×

---

## Glossary

| Term | Definition |
|------|------------|
| **MFT** | Master File Table — NTFS database containing all file metadata |
| **FRS** | File Record Segment — unique record number in the MFT (0-based) |
| **VCN** | Virtual Cluster Number — logical cluster offset within a file |
| **LCN** | Logical Cluster Number — physical cluster position on disk |
| **USA** | Update Sequence Array — NTFS sector integrity protection |
| **ADS** | Alternate Data Stream — additional named data streams on NTFS files |
| **IOCP** | I/O Completion Port — Windows async I/O mechanism |
| **$I30** | Directory index attribute name (combines `$INDEX_ROOT` + `$INDEX_ALLOCATION`) |
| **Extent** | Contiguous range of clusters on disk belonging to a file |

---

## Related Documents

| # | Document | Description |
|---|----------|-------------|
| 01 | [Overview](01-overview.md) | This document — architecture, crate map, data flow |
| 02 | [MFT Reading Pipeline](02-mft-reading.md) | IOCP, bitmap skip, extent mapping, async I/O |
| 03 | [NTFS Structures & Parsing](03-ntfs-parsing.md) | On-disk layouts, USA fixup, attribute extraction |
| 04 | [In-Memory Index](04-indexing.md) | FileRecord, names buffer, tree metrics, path resolution |
| 05 | [Concurrency Model](05-concurrency.md) | Tokio, Rayon, IOCP threads, multi-drive parallelism |
| 06 | [Pattern Matching & Search](06-pattern-search.md) | Glob, regex, extension index, smart-case |
| 07 | [Output & Streaming](07-output-streaming.md) | Formats, filters, multi-drive streaming writer |
| 08 | [CLI Reference](08-cli.md) | All flags, examples, output modes |
| 09 | [Performance & Benchmarking](09-performance.md) | Optimization techniques, profiling |
| 10 | [Build System & CI](10-build-ci.md) | Cargo profiles, cross-compilation, GitHub Actions |
| 11 | [Performance Deep Dive](11-performance-deep-dive.md) | Every optimization with measured impact, real benchmark data |

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
*UFFS Version: 0.3.62 (Rust 1.85+ / Edition 2024)*
