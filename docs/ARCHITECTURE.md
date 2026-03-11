# UFFS Architecture Snapshot

This document captures the current post-Wave-4 runtime architecture for live
search, indexing, caching, and observability. It is intentionally descriptive:
it explains what the codebase does today, not what an earlier plan expected.

## Workspace shape and dependency boundaries

| Layer | Crates | Role |
|------|--------|------|
| Data facade | `uffs-polars` | The only Polars dependency boundary; owns shared schema/column names and isolates Polars compile cost. |
| NTFS/MFT engine | `uffs-mft` | Windows-only volume access, MFT extent/bitmap discovery, parsing, index building, caching, and USN refresh. |
| Query layer | `uffs-core` | Query routing helpers, fast path resolution, extension-aware search, and tree metrics. |
| Frontends | `uffs-cli`, `uffs-tui`, `uffs-gui` | User-facing entrypoints and workflow orchestration. |
| Diagnostics | `uffs-diag` | Analysis and diagnostic tooling that is not part of the shared runtime core. |

The intentional dependency flow remains:

`uffs-polars <- uffs-mft <- uffs-core <- frontends`

Two architectural rules still matter:

- `polars` is intentionally isolated behind `uffs-polars`; runtime crates should
  not depend on Polars directly.
- `uffs-mft` remains the Windows/NTFS boundary. Everything above it should be
  able to operate on offline indices or data frames without raw volume access.

## End-to-end live read pipeline

### 1. Runtime entrypoints and shutdown boundaries

- Both `uffs` and `uffs_mft` start from Tokio entrypoints (`#[tokio::main]`).
- Each binary wraps its main async task in `run_until_shutdown()`, listens for
  `Ctrl+C`, aborts the spawned task, and maps that outcome onto the shared
  `Cancelled`/`WaitFailed` error taxonomy.
- The result is top-level graceful shutdown behavior at the binary boundary,
  even though some deeper blocking or Windows I/O operations are still coarse-
  grained from a cancellation perspective.

### 2. Volume opening and storage characterization

For live MFT reads, `uffs-mft` starts by opening `\\.\X:` through
`VolumeHandle::open()` and collecting the information needed to read `$MFT`
directly:

- drive type detection (`Nvme`, `Ssd`, `Hdd`, `Unknown`)
- NTFS volume data and file record size
- MFT extents via `FSCTL_GET_RETRIEVAL_POINTERS`
- the `$MFT::$BITMAP` occupancy map when available

The reader treats these as tuning and correctness inputs, not optional garnish:

- extents matter because the MFT can be fragmented
- bitmap data lets the readers skip unused records
- drive type feeds chunk-size and concurrency heuristics

If bitmap acquisition fails, the system degrades to an all-valid bitmap instead
of aborting the entire read.

### 3. Reader-mode resolution in the current codebase

The codebase still exposes several explicit read modes:

- `parallel`
- `streaming`
- `prefetch`
- `pipelined`
- `pipelined-parallel`
- `iocp-parallel`
- `bulk`
- `bulk-iocp`
- `sliding-iocp`
- `sliding-iocp-inline`

The important current-state detail is that `Auto` no longer means “pick a
different top-level reader per drive class” in the old SSD/HDD sense.

Today:

- the `DataFrame` path resolves `Auto` to `SlidingIocp`
- the lean-index path resolves `Auto` to `SlidingIocpInline`
- this is true for `Nvme`, `Ssd`, `Hdd`, and `Unknown`

Drive type still matters, but as a tuning input rather than a top-level mode
switch. The current heuristics are:

- `Nvme`: 4 MiB I/O chunks, concurrency 32
- `Ssd`: 2 MiB I/O chunks, concurrency 8
- `Hdd`: 1 MiB I/O chunks, concurrency 4 by default, with extent-aware HDD
  concurrency tuning in the sliding-window reader
- `Unknown`: conservative HDD-like defaults

### 4. Direct I/O and buffer strategy

UFFS does not rely on Windows directory enumeration for live reads. The MFT
pipeline instead uses direct volume/file access with Windows handles and
IOCP-capable reader variants when requested.

Key pieces of the current implementation:

- `VolumeHandle::open_overlapped_handle()` opens a second volume handle with
  `FILE_FLAG_OVERLAPPED` for IOCP-based readers
- bitmap reads use `FILE_FLAG_NO_BUFFERING`
- `AlignedBuffer` provides sector-aligned buffers so direct I/O requirements are
  satisfied
- the sliding-window reader breaks logical MFT chunks into drive-tuned I/O
  operations and keeps a bounded set of reads in flight

## Parsing and materialization model

### Parsed records and SoA staging

The parse layer has two important output shapes:

- `Vec<ParsedRecord>` for general parsed-record flows
- `ParsedColumns` for the Struct-of-Arrays (`SoA`) path used to build Polars
  data more efficiently

`ParsedColumns` exists specifically to avoid an expensive array-of-structs to
struct-of-arrays transpose. In the current code, it is explicitly documented as
the columnar staging format for direct `DataFrame` construction.

### Index-first vs DataFrame-first results

UFFS currently supports two principal in-memory representations:

- `MftIndex`: the lean search-oriented structure used by the fast path
- `DataFrame`: the richer Polars-based structure used for analytics/export-style
  workflows and Parquet interoperability

Important consequences:

- simple live search should prefer `MftIndex`
- `MftIndex::to_dataframe()` remains an on-demand conversion step rather than
  the default for every query
- tree metrics and path-oriented enrichments are derived layers, not raw MFT
  facts stored on disk

### Completeness toggles still present in the reader

The reader still exposes the completeness/performance toggles that shape live
materialization:

- extension-record merging
- bitmap use vs full-record scanning
- hard-link expansion vs one-row-per-FRS behavior
- placeholder parent synthesis for path resolution
- forensic inclusion flags for deleted/corrupt/extension records

These options materially affect speed, memory, and result semantics, so they are
part of the architecture rather than just CLI sugar.

## Query routing and search execution

### Search path selection

`uffs search` currently chooses among three source paths:

- raw MFT file input via `--mft-file`
- live/index-backed `MftIndex` queries for the fast path
- `DataFrame` queries when parquet input or DataFrame-only behavior applies

`QueryMode` behavior today is:

- `Auto`: prefer the index path unless a parquet index file forces the
  `DataFrame` path
- `ForceIndex`: keep the fast path even in multi-drive search, unless a parquet
  input makes that impossible
- `ForceDataFrame`: bypass the index path entirely

### Fast-path query architecture

The `uffs-core` fast path is built around `IndexQuery` and related helpers:

- simple pattern/size/type filters run directly over `MftIndex`
- suffix/extension queries can use the index's extension lookup to reduce a full
  scan to an `O(matches)` candidate walk
- path resolution is delegated to `FastPathResolver`, which uses dense `Vec`
  indexing plus a `NameArena` instead of a `HashMap`-heavy design

### DataFrame path and streaming orchestration

The `DataFrame` path remains the richer route for workflows that need features
the index path does not provide well, such as:

- parquet-backed queries
- SQL-like/aggregation-style workflows
- sorting and full tabular output shaping

For multi-drive live searches on Windows, the CLI can stream per-drive results
directly to console or file. That path uses bounded join-set orchestration and
can stop early when the output limit has been satisfied.

## Drive orchestration and wait boundaries

There are two concurrency layers in the current design:

- **cross-drive orchestration**: deliberately bounded
- **within-drive I/O/parse work**: allowed to exploit per-drive parallelism

Both the CLI search path and `uffs-mft::reader::MultiDriveMftReader` cap
drive-level fanout at 4 concurrent drives, further bounded by host
`available_parallelism()`.

This is deliberate: each drive may already use IOCP, buffering, Rayon parsing,
or cache refresh work internally, so unbounded multi-drive fanout would multiply
parallelism at the worst possible layer.

Blocking HANDLE-bound or cache-refresh work is intentionally pushed behind
`spawn_blocking` boundaries, and structured tracing records dispatch/wait/
replenish decisions so orchestration can be reconstructed without changing data
output.

## Cache and refresh architecture

### Index cache

Cached lean indices live under the system temp directory at:

`{TEMP}/uffs_index_cache/{DRIVE}_index.uffs`

The default TTL is 600 seconds. Cache decisions are explicit:

- `Fresh`: load cached index and attempt USN-based incremental refresh
- `Stale`: rebuild the index and rewrite cache
- `Missing`: build the index and write a new cache entry

### USN refresh behavior

When a fresh cache exists, the current code tries to advance it using the NTFS
USN journal.

Outcomes are intentionally conservative:

- journal unavailable or unreadable -> keep the cached index as-is
- journal ID changed -> full rebuild
- cached checkpoint wrapped out of journal history -> full rebuild
- no changes since last checkpoint -> reuse cached index unchanged
- valid USN delta -> apply changes, recompute tree metrics, and rewrite cache

This makes cache refresh correctness-first rather than aggressively optimistic.

### DataFrame cache usage

The CLI's per-drive DataFrame search helper still uses the existing TTL-backed
cached DataFrame loader for live searches. That path is separate from the lean
index cache and exists to avoid rebuilding the full tabular snapshot on every
DataFrame-based query.

## Observability and parity-safe diagnostics

- The supported diagnostics channel is structured tracing, not ad-hoc stdout
  text.
- Recent orchestration work records fields like `dispatch_reason`,
  `wait_strategy`, `cache_decision`, and `refresh_strategy`.
- This matters because CSV/NDJSON/parity outputs must stay data-clean.
- `scripts/verify_parity.rs` remains the canonical parity gate for live behavior.

## Operational anchors

- Live NTFS reads remain Windows-only and require Administrator privileges.
- Non-Windows hosts remain valid for development, cross-compilation, offline
  index/query work, docs, and most unit tests.
- Full parity regeneration remains environment-sensitive because it depends on a
  Windows-accessible data root and live MFT/USN behavior.
- `uffs-mft` remains the architectural center of gravity for the repository. Its
  concentration is a real maintenance concern, but not one changed by the Wave-4
  runtime hardening work itself.

## Related canonical docs

- [`docs/README.md`](README.md)
- [`docs/architecture/README.md`](architecture/README.md)
- [`docs/architecture/MFTINDEX_DEEP_DIVE.md`](architecture/MFTINDEX_DEEP_DIVE.md)
- [`docs/architecture/unsafe-surface-review.md`](architecture/unsafe-surface-review.md)
- [`docs/PERFORMANCE.md`](PERFORMANCE.md)
- [`docs/RISKS.md`](RISKS.md)
