# UFFS Risk Register

This document records the material operational and maintenance risks that still
matter after the post-Wave-4 hardening work. It focuses on risks that remain
true in the current codebase, not issues that were already retired by the recent
observability/concurrency work.

## Closed or materially reduced

- Hotspot diagnostics that previously risked contaminating CLI/parity output are
  now routed through structured tracing instead of ad-hoc stderr writes.
- Representative live search/index orchestration paths now emit structured
  context for drive selection, cache decisions, and async wait handoffs.
- Multi-drive drive-level orchestration remains explicitly bounded rather than
  unbounded fan-out.
- Top-level binary shutdown behavior is materially better than before because
  both `uffs` and `uffs_mft` now wrap their main task in explicit `Ctrl+C`
  handling and map shutdown onto the shared cancellation taxonomy.

## Active risks and accepted caveats

| Risk | Why it matters | Current disposition |
|------|----------------|---------------------|
| Windows-only live MFT access | Full end-to-end live validation still requires Windows with Administrator privileges. Non-Windows builds can validate offline/query logic but cannot exercise the real volume-reading path. | Accepted platform constraint; keep documenting and testing around it. |
| Parity regeneration is environment-dependent | `scripts/verify_parity.rs` depends on a live data root plus Windows MFT/USN behavior, so not every host can run the full parity gate locally. | Accepted operational constraint; parity remains a required verifier in the appropriate environment. |
| Polars/toolchain volatility | `uffs-polars` intentionally tracks Polars `branch = "main"` and enables Polars' `nightly` feature, while the workspace toolchain is pinned to nightly. That buys access to current performance features, but increases upgrade and breakage risk. | Accepted build/runtime tradeoff; keep this visible in architecture/performance docs and CI notes. |
| Unsafe surface remains substantial | The workspace denies unsafe by default, but the NTFS/Windows boundary still contains a meaningful reviewed exception surface. A simple current repo snapshot (`rg "unsafe\s*\{" crates`) finds 142 `unsafe` blocks, concentrated in `uffs-mft` volume I/O, parser, and NTFS layout code. | Accepted but important. Maintain review discipline and keep the unsafe-surface docs current. |
| Cache freshness depends on NTFS journal state | Fresh cache hits can be refreshed incrementally, but missing journal state, read failures, wraparound, or journal recreation can force a fallback to cached-as-is or full rebuild behavior. | Accepted filesystem caveat; current behavior favors correctness and operational safety over aggressive assumptions. |
| “Fast” vs “full” semantics are not perfectly harmonized across entrypoints | Public CLI help still describes the classic fast-path story where extension records are skipped unless `--full` is requested, while `MftReader::open()` currently initializes `merge_extensions: true` for baseline-compatible output. The capability is clear, but the default story is not yet perfectly uniform. | Active documentation caveat. Describe the feature truthfully and avoid assuming one universal default in docs or benchmarks. |
| Live forensic support is incomplete | The `uffs_mft` live read command still warns that `--forensic` is not yet supported for live reads and advises saving/loading an MFT file for forensic analysis instead. | Accepted functional limitation; document clearly to avoid operator surprise. |
| Bounded multi-drive fanout is a tradeoff | Drive-level concurrency is intentionally capped at 4 to avoid oversubscribing hosts while each drive still performs internal parallel work. This protects stability but can leave peak throughput on the table for some hosts. | Accepted performance/safety tradeoff; tune later only with profiling evidence. |
| Cancellation remains coarse-grained below the entrypoint layer | Top-level `Ctrl+C` handling now exists, and streaming search can stop early when output limits are reached, but many deeper operations are still blocking or phase-based. Cancellation is cooperative at orchestration boundaries, not fine-grained at every Windows I/O step. | Accepted runtime caveat; much better than before, but not a fully cancellable pipeline. |
| Host resource pressure | Workspace-wide builds, parity runs, large `DataFrame` materializations, and release artifacts still require meaningful disk, memory, and I/O headroom. | Active environment constraint. |
| Structural concentration in `uffs-mft` | `uffs-mft` remains the largest and most operationally dense crate, which raises maintenance, review, and unsafe-audit cost. | Known follow-up area; unchanged by this documentation wave. |
| CI tier split | Always-on CI still does not exercise every heavy Windows/parity path on every change. Some of the most important guarantees still rely on explicit Windows-only or operator-run validation. | Mitigated, not eliminated. Keep the heavier validation canon explicit. |

## Practical interpretation notes

### Cache fallbacks are intentionally conservative

The current cache-refresh rules are correctness-first:

- USN unavailable or unreadable -> return cached index as-is
- journal ID changed or checkpoint wrapped -> rebuild
- valid delta -> update cached index and recompute tree metrics

This reduces the risk of serving obviously incorrect data, but it does mean
latency and freshness can vary sharply depending on NTFS journal health.

### Documentation must describe current code, not legacy expectations

There are still places where older mode/default narratives survive in comments or
help text even though the effective code path has shifted. The current examples
are:

- `Auto` mode now routing to sliding-window IOCP readers for all drive types
- extension-merge defaults not being described consistently across entrypoints

Those are documentation-follow-through caveats, not a license to silently pick a
preferred story.

## Operational note

- Structured tracing is now the supported diagnostic channel for these flows.
  It is expected to go to stderr or a tracing sink, not stdout, so data output
  remains parity-safe.
