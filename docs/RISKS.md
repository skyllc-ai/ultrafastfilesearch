# UFFS Risk Register

This document records the material operational risks that still matter after the
Wave 2-4 hardening work.

## Closed or materially reduced

- Hotspot diagnostics that previously risked contaminating CLI/parity output are
  now routed through structured tracing instead of ad-hoc stderr writes.
- Representative live search/index orchestration paths now emit structured
  context for drive selection, cache decisions, and async wait handoffs.
- Multi-drive drive-level orchestration remains explicitly bounded rather than
  unbounded fan-out.

## Active risks and accepted caveats

| Risk | Why it matters | Current disposition |
|------|----------------|---------------------|
| Windows-only live MFT access | Full end-to-end live validation still requires Windows with Administrator privileges. | Accepted platform constraint; keep documenting and testing around it. |
| Parity regeneration is environment-dependent | `scripts/verify_parity.rs` depends on a live data root plus Windows MFT/USN behavior, so not every host can run the full parity gate locally. | Accepted operational constraint; parity remains a required verifier in the appropriate environment. |
| Bounded multi-drive fanout is a tradeoff | Drive-level concurrency is intentionally capped at 4 to avoid oversubscribing hosts while each drive still performs internal parallel work. | Accepted performance/safety tradeoff; tune later only with profiling evidence. |
| Cache freshness depends on NTFS journal state | Fresh cache hits can be refreshed incrementally, but missing journal state, read failures, wraparound, or journal recreation can force a fallback to cached-as-is or full rebuild behavior. | Accepted filesystem caveat; current behavior favors correctness and operational safety over aggressive assumptions. |
| Host resource pressure | Workspace-wide builds, docs, parity runs, and release binaries still require meaningful disk, memory, and I/O headroom. | Active environment constraint. |
| Structural concentration in `uffs-mft` | `uffs-mft` remains the largest and most operationally dense crate, which raises maintenance and review cost. | Known follow-up area; unchanged by this observability/documentation wave. |
| CI tier split | Always-on CI still does not exercise every heavy Windows/parity path on every change. | Mitigated by wave-level verification gates and explicit parity verification. |

## Operational note

- Structured tracing is now the supported diagnostic channel for these flows.
  It is expected to go to stderr or a tracing sink, not stdout, so data output
  remains parity-safe.
