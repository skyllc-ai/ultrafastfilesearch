# CRUCIBLE Performance Baseline

This document records the current performance model carried by the CRUCIBLE
audit artifacts. It is intentionally specific about where speed comes from,
which code paths are supposed to be fast, and which caveats still affect any
performance claim made against the current repo.

## What “fast” means in the current codebase

UFFS has two distinct performance stories:

| Workflow | Preferred representation | Why |
|----------|--------------------------|-----|
| Simple live search (`glob`, extension, size, basic filters) | `MftIndex` | Avoids `DataFrame` construction and uses search-specific structures. |
| Rich tabular workflows, parquet input, aggregations/sorting | `DataFrame` | Pays conversion/materialization cost to unlock the richer query model. |
| Full-drive live indexing | `MftReader` + drive-tuned MFT readers | Speed comes from direct NTFS MFT access, not Win32 file enumeration. |
| Multi-drive search | bounded cross-drive orchestration | Favors predictable host utilization over unbounded fanout. |

The fastest path in the repo is not “Polars everywhere”; it is “read the MFT
efficiently, keep simple search on `MftIndex`, and convert to tabular form only
when that extra expressiveness is actually needed.”

## Current hot-path model

### 1. Direct MFT access is still the primary macro-win

UFFS gets its biggest speed advantage by bypassing Windows file enumeration APIs
and reading NTFS metadata directly.

That still means:

- volume handle opening and MFT extent discovery in `uffs-mft`
- optional bitmap-driven skipping of unused records
- direct/aligned volume I/O for the MFT pipeline
- no dependency on recursive directory walking for the live path

### 2. Auto mode is now IOCP-first

One important current-state performance detail: `Auto` mode is effectively an
IOCP-sliding-window selector now.

Today:

- `DataFrame` reads map `Auto` -> `SlidingIocp`
- lean-index reads map `Auto` -> `SlidingIocpInline`

That is true across `Nvme`, `Ssd`, `Hdd`, and `Unknown` drive types. Drive type
still matters, but mainly as a tuning input for:

- I/O chunk size (`Nvme` 4 MiB, `Ssd` 2 MiB, `Hdd`/`Unknown` 1 MiB)
- read concurrency (`Nvme` 32, `Ssd` 8, `Hdd` 4 with extent-aware HDD tuning)
- whether certain parsing strategies are beneficial

So the current performance story is not “Auto picks different reader families by
device class”; it is “Auto picks the sliding-window IOCP family, then tunes it
per device.”

### 3. The index path is the main query-time optimization

`uffs-core::index_search` still documents the expected gap between the two query
representations for simple searches on large datasets:

- `MftIndex` path: roughly `~100-200ms` for 23M entries
- `DataFrame` path: roughly `~3-5s`, largely because of conversion/materialization overhead

That gap is not accidental. The fast path is built to avoid work:

- direct record iteration instead of `DataFrame` expression setup
- extension-aware candidate reduction through the extension index
- path resolution only when needed
- Rayon-powered filtering/expansion over already-compact in-memory structures

For simple search, any performance comparison that forces the `DataFrame` path is
measuring a different workload.

### 4. Path resolution is an explicit hot path

Path reconstruction remains a major cost center because NTFS stores parent FRS
links, not full paths.

The current optimization stack is:

- `FastPathResolver` uses `Vec`-indexed O(1) lookup rather than `HashMap`-based
  lookup
- file names are interned in a contiguous `NameArena`
- resolver entries are packed to 16 bytes each
- parallel path-column addition uses Rayon when the dataset is large enough

The code documents the expected win as:

- `FastPathResolver` is roughly `3-5x` faster than the legacy resolver
- memory use is roughly `~50%` lower due to `NameArena`

### 5. SoA staging reduces `DataFrame` build cost

`ParsedColumns` is the current struct-of-arrays staging format for the MFT parse
pipeline.

That matters because it removes the old array-of-structs -> struct-of-arrays
transpose during `DataFrame` build. The code documents this as reducing
`df_build` time by about `~20%`.

This optimization only helps the tabular path, but it is still important because
that path is used for parquet/export/analytics-style workflows.

## Where the current costs still go

### Live full-scan costs

Even with the current optimizations, a live full-drive read still spends real
time in:

- volume setup and metadata collection
- reading the MFT extents themselves
- fixup/parsing work on each record
- optional extension-record merging
- optional placeholder parent synthesis
- `DataFrame` construction when the tabular path is requested

### Query-time costs

For search, the main remaining variable costs are:

- path resolution when output needs full paths
- row expansion for hard links and streams
- `DataFrame` materialization when the query mode requires it
- drive fanout and output serialization for multi-drive streaming search

### Cache-sensitive costs

The best-case live-query performance is often “use a fresh cached index and do a
small or no-op USN refresh.”

The worst case is still “cache miss or stale cache -> full rebuild.”

In between, current behavior intentionally trades peak freshness for safety:

- journal unavailable/read failure -> use cached index as-is
- journal wrapped/journal ID changed -> rebuild
- valid delta -> apply USN changes and recompute tree metrics

That is often a major latency difference, so performance discussions should
always state whether the measured path was a cold full scan, a warm cache hit,
or a USN refresh.

## Performance-sensitive options and their tradeoffs

The current codebase still exposes a few switches that materially change runtime
costs:

- extension-record merging: more complete, slower
- bitmap usage: usually faster, but can be disabled for debugging/experiments
- placeholder creation: code comments document roughly `~15%` CPU savings when
  disabled, at the cost of weaker path-resolution behavior on some records
- hard-link expansion: more user-visible rows, more work

One caveat worth stating explicitly: the public “fast vs `--full`” story still
exists, but internal defaults around extension merging are not fully harmonized
across every entrypoint. Benchmark explicit flags/settings when making claims
instead of assuming one universal default.

## Benchmark inventory in the repo

### Cross-platform benchmark lane

When live Windows MFT validation is unavailable, the repo still has useful,
smaller-scope benchmark coverage in `uffs-core`:

- `cargo bench -p uffs-core --bench query`
- `cargo bench -p uffs-core --bench search_benchmarks`

Those benches cover:

- pattern parsing and glob/regex conversion
- `MftQuery` building and execution
- extension filter performance
- tree metric/index operations
- `PathResolver` vs `FastPathResolver`
- sequential vs parallel path-column addition

### Windows-specific MFT benchmark lane

The repo also keeps a Windows-oriented benchmark lane for the MFT path:

- `cargo bench -p uffs-mft --bench mft_read`

That bench focuses on lower-level components such as:

- aligned buffer allocation/write cost
- `ParsedColumns` allocation/merge cost
- `ParsedColumns` -> `DataFrame` conversion

For more realistic end-to-end Windows measurements, the `uffs_mft` binary also
retains benchmark commands such as `benchmark-index` and `benchmark-index-lean`.

## Validation canon anchors

The approved validation canon for this baseline remains:

1. `cargo build --release -p uffs-cli --bin uffs`
2. `cargo xwin check -p uffs-mft --lib --bin uffs_mft`
3. `cargo test -p uffs-mft --bin uffs_mft required_output_path`
4. `rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate`

These are not all “performance benchmarks” in the narrow sense. They are the
approved correctness-and-parity anchors that protect the performance baseline
from silently regressing via behavior drift.

## Current carried status

- Validation canon alignment is verified.
- Wave 1C parity artifact resolution is verified.
- The `required_output_path` regression check is still considered mandatory, but
  the current rerun is blocked by external disk pressure on the host (`No space
  left on device`, `os error 28`). This is carried forward as an environment
  blocker, not a performance or correctness regression.

## How to interpret performance claims safely

When discussing UFFS performance, always specify at least:

- live MFT vs cached index vs cached `DataFrame`
- `MftIndex` vs `DataFrame` query path
- single-drive vs multi-drive
- extension merge / placeholder settings if relevant
- Windows elevated live run vs offline/non-Windows benchmark lane

Without those qualifiers, two “UFFS is fast” statements may be describing very
different parts of the system.
