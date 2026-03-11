# UFFS Architecture Snapshot

This document captures the post-Wave 2-4 architecture state for the live search,
index, cache, and observability flows. It is a runtime snapshot, not a roadmap.

## Workspace shape

| Layer | Crates | Role |
|------|--------|------|
| Data facade | `uffs-polars` | Single Polars entry point and column/schema home |
| NTFS/MFT engine | `uffs-mft` | Windows-only live MFT access, parsing, index building, cache/USN refresh |
| Query layer | `uffs-core` | Path resolution, pattern matching, tree metrics, search helpers |
| Frontends | `uffs-cli`, `uffs-tui`, `uffs-gui` | User-facing workflows |
| Diagnostics | `uffs-diag` | Diagnostic and analysis tooling |

The dependency flow remains intentionally layered:

`uffs-polars <- uffs-mft <- uffs-core <- frontends`

## Orchestration model

### Search path selection

- `uffs search` chooses among three source paths:
  - raw MFT file input via `--mft-file`
  - live `MftIndex` queries for the fast path
  - live `DataFrame` queries when parquet input or DataFrame-only behavior applies
- `QueryMode::Auto` prefers the index path unless a parquet index file is provided.
- `QueryMode::ForceDataFrame` bypasses the index fast path.
- `QueryMode::ForceIndex` preserves current CLI semantics and now emits structured
  tracing for the chosen drive scope instead of ad-hoc text diagnostics.

### Drive orchestration and wait boundaries

- Multi-drive live reads are intentionally bounded at the drive layer.
  The CLI search path and `uffs-mft::reader::MultiDriveMftReader` both cap
  drive-level fanout at 4 concurrent drives.
- Each drive can still use its own internal reader parallelism; the outer cap
  exists to avoid multiplying already-parallel per-drive work across too many
  volumes at once.
- Windows HANDLE-bound reads stay behind `spawn_blocking` boundaries.
  Structured tracing now records dispatch/wait/replenish decisions around those
  handoffs so orchestration can be reconstructed from logs without changing data
  output.

### Cache and refresh behavior

- Index cache usage remains enabled by default for live index reads.
- `--no-cache` preserves the existing CLI behavior by bypassing cached index use.
- Fresh cached indices are incrementally refreshed from the NTFS USN journal when
  possible.
- Stale or missing cache entries trigger a full rebuild.
- If the USN journal is unavailable, unreadable, wrapped past the cached
  checkpoint, or recreated with a new journal id, the system falls back to the
  existing safe behaviors: use the cached index as-is or rebuild, depending on
  the specific condition.
- The CLI's per-drive DataFrame search helper continues to use the existing TTL-
  backed cached DataFrame loader for live searches.

### Streaming and parity-safe observability

- Multi-drive DataFrame searches can stream results directly to console or file.
- Observability added in Waves 4A-4B is structured tracing, not data-plane text.
  Traces go through the configured tracing sink (stderr/file), so stdout-based
  CSV/NDJSON output and parity artifacts remain clean.
- `scripts/verify_parity.rs` remains the canonical parity gate for live behavior.

## Operational anchors

- Live NTFS reads remain Windows-only and require Administrator privileges.
- Non-Windows hosts remain valid for development, cross-compilation, offline
  index/query work, docs, and most unit tests.
- Full parity regeneration remains environment-sensitive because it depends on a
  Windows-accessible data root and live MFT/USN behavior.
- `uffs-mft` is still the structural center of gravity for the repository; file
  size and concentration are follow-up concerns, not part of the Wave 4 runtime
  hardening work.

## Related canonical docs

- [`docs/README.md`](README.md)
- [`docs/architecture/README.md`](architecture/README.md)
- [`docs/PERFORMANCE.md`](PERFORMANCE.md)
- [`docs/RISKS.md`](RISKS.md)
