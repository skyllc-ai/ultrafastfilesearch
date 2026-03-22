# Cross-Platform Refactoring Plan: Eliminating Unnecessary `#[cfg(windows)]`

## Problem Statement

The codebase has **~220+ `#[cfg(windows)]` / `#[cfg(not(windows))]`** gates spread
across the CLI, MFT reader, and core crates. Many of these guard code that is
genuinely cross-platform (index processing, filtering, streaming output) but
was cfg-gated because the only *callers* were Windows-only LIVE paths.

**The core insight:** The only truly Windows-specific operation is reading
the MFT from a live NTFS volume via IOCP. Everything after that — parsing,
indexing, tree metrics, filtering, pattern matching, streaming output — operates
on `MftIndex` which is a pure Rust data structure with no platform dependencies.

## Current Architecture (Before Refactoring)

```
                    WINDOWS ONLY                    CROSS-PLATFORM
                    ═══════════                     ══════════════
                    
    ┌──────────────────────┐
    │ IOCP Volume Read     │ ← Win32 API (CreateFile, DeviceIoControl)
    │ (reader/index_read)  │
    └──────────┬───────────┘
               │
               ▼
    ┌──────────────────────┐     ┌──────────────────────┐
    │ MFT Parser           │     │ --mft-file Reader     │
    │ (io/parser/*)        │────▶│ (reader/persistence)  │
    │ IOCP completion      │     │ Read from .mft file   │
    └──────────┬───────────┘     └──────────┬───────────┘
               │                            │
               ▼                            ▼
    ┌─────────────────────────────────────────────────┐
    │              MftIndex (pure data)                │ ← CROSS-PLATFORM
    │  records, names, streams, children, extensions   │
    └──────────────────────┬──────────────────────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
    ┌──────────────┐ ┌──────────┐ ┌──────────────────┐
    │ tree_metrics │ │ ext_index│ │ PathCache         │
    │ (compute)    │ │ (build)  │ │ (dir_cache)       │
    └──────┬───────┘ └────┬─────┘ └────────┬──────────┘
           │              │                │
           ▼              ▼                ▼
    ┌─────────────────────────────────────────────────┐
    │         Streaming Output Writer                  │ ← SHOULD BE CROSS-PLATFORM
    │  write_index_streaming_with_filter()             │    but currently has cfg gates
    │  pattern matching, attr filters, sort, etc.      │
    └─────────────────────────────────────────────────┘
```

## Audit: What Is Truly Windows-Only

### Layer 1: Platform I/O (GENUINELY Windows-only) — KEEP `#[cfg(windows)]`

These use Win32 API directly. No way to make cross-platform.

| File | What | Why Windows-only |
|---|---|---|
| `uffs-mft/src/platform/system.rs` | Volume handles, drive detection | `CreateFileW`, `DeviceIoControl` |
| `uffs-mft/src/platform/extents.rs` | NTFS extent mapping | `FSCTL_GET_RETRIEVAL_POINTERS` |
| `uffs-mft/src/io/iocp/*.rs` | IOCP sliding window | `CreateIoCompletionPort`, `GetQueuedCompletionStatus` |
| `uffs-mft/src/reader/read_mode.rs` | Volume open modes | Raw volume access |
| `uffs-mft/src/reader/benchmark.rs` | IOCP benchmark | IOCP timing |
| `uffs-mft/src/progress.rs` | Console progress bar | `GetConsoleMode` |
| `uffs-mft/src/usn.rs` | USN journal | Win32 USN API |

**Count: ~80 cfg gates — legitimate, keep all.**

### Layer 2: Reader Dispatch (PARTIALLY Windows-only) — REFACTOR

The `MftReader` dispatches between live volume read (Windows) and file read
(cross-platform), but has cfg gates scattered throughout.

| File | What | Current state | Target |
|---|---|---|---|
| `reader.rs` | `MftReader` struct | 15 cfg gates | Trait-based dispatch |
| `reader/index_read.rs` | Live index read | 12 cfg gates | Extract to `WindowsLiveReader` |
| `reader/dataframe_read.rs` | Live DataFrame read | 10 cfg gates | Extract to `WindowsLiveReader` |
| `reader/index_cache.rs` | Cache read/write | 3 cfg gates | Make fully cross-platform |
| `reader/persistence.rs` | .mft file read | 6 cfg gates | Make fully cross-platform |
| `reader/multi_drive/mod.rs` | Multi-drive scan | 10 cfg gates | Split live vs file |

**Count: ~56 cfg gates — most can be eliminated with trait-based reader.**

### Layer 3: CLI Search Command (SHOULD BE cross-platform) — REFACTOR

This is where the most unnecessary cfg gates are. The search command has
cfg gates around code that operates on `MftIndex` — pure data with no
platform dependency.

| File | Lines with cfg | What's cfg-gated | Should be cross-platform? |
|---|---|---|---|
| `search/mod.rs` | 18 | Helper functions (build_record_filter, try_get_extension_indices, etc.) | ✅ YES — pure index operations |
| `search/mod.rs` | — | Multi-drive live scan | ❌ No — uses IOCP live reader |
| `search/mod.rs` | — | `drives_to_search` detection | ❌ No — Win32 drive enumeration |
| `search/drive_search.rs` | module | Drive search helpers | ❌ No — live MFT reads |
| `search/multi_drive.rs` | module | Multi-drive DataFrame | ❌ No — live MFT reads |
| `search/streaming.rs` | module | Multi-drive streaming | ❌ No — live MFT reads |

**Count: ~18 cfg gates — ~12 can be eliminated.**

### Layer 4: CLI Output (SHOULD BE fully cross-platform) — REFACTOR

Output formatting operates on `MftIndex` and `SearchResult` — no platform
dependency at all. But currently has cfg gates because some types/functions
are only *called* from Windows paths.

| Code | Current cfg | Should be cross-platform? |
|---|---|---|
| `AttrRequirement`, `AttrKind` | `#[cfg(windows)]` | ✅ YES — pure enum + match |
| `SortColumn`, `SortKind` | `#[cfg(windows)]` | ✅ YES — pure enum + match |
| `compare_records()` | `#[cfg(windows)]` | ✅ YES — operates on MftIndex |
| `parse_sort_spec()` | `#[cfg(windows)]` | ✅ YES — pure string parsing |
| `parse_attr_filter()` | `#[cfg(windows)]` | ✅ YES — pure string parsing |
| `parse_age_filter()` | `#[cfg(windows)]` | ✅ YES — pure time math |
| `build_record_filter()` | `#[cfg(windows)]` | ✅ YES — constructs filter struct |
| `try_get_extension_indices()` | `#[cfg(windows)]` | ✅ YES — operates on MftIndex |
| `extract_trailing_extension()` | `#[cfg(windows)]` | ✅ YES — pure string parsing |
| `write_streaming_output_with_filter()` | `#[cfg(windows)]` | ✅ YES — writes from MftIndex |
| `StreamingRecordFilter.attr_filters` | `#[cfg(windows)]` field | ✅ YES — pure data |
| `StreamingRecordFilter.sort_spec` | `#[cfg(windows)]` field | ✅ YES — pure data |
| `StreamingRecordFilter.sort_desc` | `#[cfg(windows)]` field | ✅ YES — pure data |
| `write_native_header_pub()` | `#[cfg(windows)]` | ✅ YES — pure string output |
| `write_index_streaming_no_header()` | `#[cfg(windows)]` | ✅ YES — operates on MftIndex |
| `write_cpp_footer_pub()` | `#[cfg(windows)]` | ✅ YES — pure string output |
| `write_index_streaming_filtered()` | `#[cfg(windows)]` | ✅ YES — operates on MftIndex |
| `streaming.rs` module (StreamingWriter) | `#[cfg(windows)]` | ⚠️ MIXED — some Win32, mostly cross-platform |

**Count: ~24 cfg gates — ALL can be eliminated.**

## Refactoring Plan

### Phase 1: Make Output Fully Cross-Platform (LOW RISK)

**Goal:** Remove all `#[cfg(windows)]` from `output.rs` types, functions, and
struct fields. Make `--mft-file` path use ALL the new filters (attr, sort, date,
exclude).

**Steps:**
1. Remove `#[cfg(windows)]` from `AttrRequirement`, `AttrKind`, `SortColumn`, `SortKind`, `SortEntry`, `compare_records`, all parse functions
2. Remove `#[cfg(windows)]` from `StreamingRecordFilter` fields (`attr_filters`, `sort_spec`, `sort_desc`)
3. Remove `#[cfg(windows)]` from `write_native_header_pub`, `write_index_streaming_no_header`, `write_cpp_footer_pub`, `write_index_streaming_filtered`
4. Make `--mft-file` filtered path use `build_record_filter` + `write_streaming_output_with_filter` (cross-platform)
5. **Remove legacy code** that's now replaced:
   - `write_native_results()` → replaced by `write_streaming_output_with_filter()`
   - `write_native_results_to()` → replaced by unified streaming
   - `write_native_value()` → replaced by inline column formatting in streaming writer
   - `result_path()`, `path_only_from_path()` → replaced by `materialize_path_into()`
   - `native_file_type()` → replaced by inline extension lookup
   - `displayed_size()`, `displayed_allocated_size()` → replaced by inline tree_metrics
   - `native_tree_metrics()` → replaced by inline `record.tree_metrics()`
   - `append_display()` → replaced by `itoa`
   - `write_filtered_streaming_output()` → replaced by `write_streaming_output_with_filter()`
   - `write_index_streaming_filtered()` → replaced by unified `write_index_streaming_with_filter()`

**Estimated impact:** Remove ~24 cfg gates, ~200 lines of dead code.
**Risk:** LOW — tests verify output parity. `--mft-file` path is well-tested.

### Phase 2: Make Search Helpers Cross-Platform (LOW RISK)

**Goal:** Remove `#[cfg(windows)]` from pure-logic helper functions in `search/mod.rs`.

**Steps:**
1. Remove `#[cfg(windows)]` from `try_get_extension_indices()`, `build_record_filter()`, `extract_trailing_extension()`, `write_streaming_output_with_filter()`
2. Remove the `#[cfg(not(windows))] { let _: ... }` suppression block
3. Wire `--mft-file` path to use these helpers (cross-platform callers make them reachable)

**Estimated impact:** Remove ~12 cfg gates, ~15 lines of suppression code.
**Risk:** LOW — these functions are pure data operations.

### Phase 3: Trait-Based Reader (MEDIUM RISK)

**Goal:** Replace the monolithic `MftReader` with a trait that abstracts
the MFT source (live volume vs file).

```rust
/// Cross-platform trait for reading MFT data.
pub trait MftSource {
    /// Read the MFT and build an MftIndex.
    async fn read_index(&self) -> Result<MftIndex>;
    
    /// Read the MFT and build a DataFrame.
    async fn read_dataframe(&self) -> Result<DataFrame>;
    
    /// Volume letter for this source.
    fn volume(&self) -> char;
}

/// Windows LIVE reader (IOCP from volume).
#[cfg(windows)]
pub struct LiveMftReader { ... }

/// Cross-platform file reader (from .mft file).
pub struct FileMftReader { ... }
```

**Steps:**
1. Define `MftSource` trait in `uffs-mft`
2. Extract `LiveMftReader` (Windows-only, keeps all IOCP code)
3. Extract `FileMftReader` (cross-platform, reads .mft files)
4. Make `MftReader` a thin dispatch wrapper
5. Move all post-read processing (tree_metrics, ext_index, PathCache) into shared code

**Estimated impact:** Remove ~56 cfg gates from reader modules.
**Risk:** MEDIUM — changes the reader abstraction layer.

### Phase 4: Remove Legacy DataFrame Output Path (HIGH VALUE)

**Goal:** The streaming path (`write_index_streaming_with_filter`) now handles
ALL patterns and filters. The old `IndexQuery → SearchResult → write_native_results`
path is redundant for native output.

**What to remove:**
- `IndexQuery::collect()` call path for native output (keep for DataFrame)
- `write_native_results()` and all its helper functions
- `RecordExpander::collect_results()` (output expansion now in streaming writer)
- `execute_index_query_native_pub()` in `raw_io.rs`

**What to keep:**
- `IndexQuery::collect()` for DataFrame/Polars output (non-native formats)
- `RecordFilter` for the Polars query path
- All the streaming output code

**Estimated impact:** Remove ~400 lines of legacy output code.
**Risk:** HIGH — need thorough parity testing before removal.

## Summary: cfg Gate Reduction

| Phase | cfg gates removed | Lines removed | Risk |
|---|---|---|---|
| Phase 1: Output cross-platform | 24 | ~200 (dead code) | LOW |
| Phase 2: Search helpers cross-platform | 12 | ~15 | LOW |
| Phase 3: Trait-based reader | 56 | ~100 (refactored) | MEDIUM |
| Phase 4: Remove legacy output | 0 (already removed) | ~400 | HIGH |
| **Total** | **~92** | **~715** | |

Remaining after all phases: **~80 cfg gates** — all in the genuinely
Windows-only IOCP/volume/platform layer where they belong.

## Execution Status

| Phase | Status | cfg removed | Lines removed |
|---|---|---|---|
| Phase 1: Output cross-platform | ✅ **DONE** | 24 | ~200 |
| Phase 2: Search helpers cross-platform | ✅ **DONE** | 12 | ~15 |
| Phase 4: Remove legacy output | ✅ **DONE** | 0 | ~500 |
| Phase 3: Trait-based reader | ❌ **NOT STARTED** | 0 of 56 | 0 |

### Remaining cfg gates in CLI crate: 38

- `search/mod.rs` (13) — Windows LIVE streaming (IOCP multi-drive, drive
  detection). **Genuinely Windows-only** — calls `MftReader::open(drive)`.
- `output.rs` (7) — 3 Windows multi-drive helpers (callers are Windows-only),
  2 cfg-gated imports, 2 test gates. **Blocked by Phase 3.**
- `raw_io.rs` (7), `index.rs` (6), `commands.rs` (5) — Windows LIVE MFT
  reader calls. **Genuinely Windows-only.**

### Remaining cfg gates in uffs-mft crate: ~130

All in `reader/` submodules. Blocked by Phase 3 (trait-based reader).

## Phase 3: Detailed Implementation Plan

### Current Architecture

```
MftReader {
    volume: char,
    #[cfg(windows)] handle: VolumeHandle,  ← ONLY platform-specific field
    mode: MftReadMode,
    merge_extensions: bool,
    use_bitmap: bool,
    expand_links: bool,
    add_placeholders: bool,
    concurrency: Option<usize>,
    io_size: Option<usize>,
    parallel_parse: Option<bool>,
    parse_workers: Option<usize>,
    forensic: bool,
}
```

The `handle` field is the root cause of ALL 56 cfg gates in the reader layer.
Every method that touches `self.handle` needs `#[cfg(windows)]`.

### Proposed Architecture

```rust
/// Cross-platform MFT source abstraction.
enum MftSource {
    /// Live NTFS volume (Windows only, IOCP pipeline).
    #[cfg(windows)]
    LiveVolume(VolumeHandle),
    /// Pre-captured .mft file (cross-platform).
    File(std::path::PathBuf),
}

/// MFT Reader — cross-platform struct.
pub struct MftReader {
    volume: char,
    source: MftSource,
    // ... all config fields unchanged ...
}
```

### Key Changes

1. **`MftSource` enum** — abstracts the data source. `LiveVolume` variant
   is `#[cfg(windows)]` (contains `VolumeHandle`), `File` variant is
   cross-platform.

2. **`MftReader::open(drive)`** — stays Windows-only, creates
   `MftSource::LiveVolume`.

3. **`MftReader::from_file(path, volume)`** — NEW cross-platform constructor,
   creates `MftSource::File`.

4. **Read methods** — dispatch on `self.source`:
   ```rust
   pub async fn read_all_index(&self) -> Result<MftIndex> {
       match &self.source {
           #[cfg(windows)]
           MftSource::LiveVolume(handle) => self.read_index_live(handle).await,
           MftSource::File(path) => self.read_index_from_file(path).await,
       }
   }
   ```

5. **Multi-drive** — `MultiDriveMftReader` dispatches per-drive to either
   live or file sources, enabling cross-platform multi-drive with
   `--mft-file` paths.

### Files to Change

| File | cfg gates | Change |
|---|---|---|
| `reader.rs` | 15 | Add `MftSource`, `from_file()`, make struct cross-platform |
| `reader/index_read.rs` | 12 | Split into `read_index_live()` + `read_index_from_file()` |
| `reader/dataframe_read.rs` | 10 | Same split pattern |
| `reader/multi_drive/mod.rs` | 10 | Support `MftSource::File` per-drive |
| `reader/persistence.rs` | 6 | Wire into `MftSource::File` path |
| `reader/index_cache.rs` | 3 | Make cache work with file sources |
| `reader/index_timing.rs` | — | Mostly follows index_read changes |
| `reader/dataframe_timing.rs` | — | Mostly follows dataframe_read changes |
| `reader/read_mode.rs` | — | Add file-based mode variant |
| `reader/benchmark.rs` | — | Windows-only, keep cfg gates |

### Risk Mitigation

- **Test with existing Windows benchmarks** before and after to verify no
  performance regression on LIVE path.
- **Keep `--mft-file` path working** throughout — it's the cross-platform
  regression test.
- **Atomic commits** per submodule (reader.rs first, then index_read, etc.).

### Estimated Effort

- ~4-6 hours of focused refactoring
- ~56 cfg gates removed
- ~100 lines refactored (not removed — moved into dispatch methods)
- Risk: MEDIUM (touches core parser, needs careful testing)

## Key Principle

> The boundary between Windows-only and cross-platform code should be
> **the MftIndex construction**. Everything before it (IOCP, volume handles,
> extent mapping) is Windows-only. Everything after it (tree metrics,
> filtering, pattern matching, streaming output) is cross-platform.
