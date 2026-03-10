# Performance Docs

Use this bucket for benchmark reports, optimization notes, and validation results.

## Current entry points

- [`../PHASE7_PERFORMANCE_ANALYSIS.md`](../PHASE7_PERFORMANCE_ANALYSIS.md)
- [`../UFFS_PERFORMANCE_OPTIMIZATION_PHASE2.md`](../UFFS_PERFORMANCE_OPTIMIZATION_PHASE2.md)
- [`../architecture/PHASE7_PERFORMANCE_VALIDATION.md`](../architecture/PHASE7_PERFORMANCE_VALIDATION.md)

This landing page gives the performance work a stable home while older documents are incrementally normalized.

## Focused cross-platform benchmark lane

- Hot-path query benchmark: `cargo bench -p uffs-core --bench query`
- Benchmark source: [`../../crates/uffs-core/benches/query.rs`](../../crates/uffs-core/benches/query.rs)

Use this lane for non-Windows performance checks on query/pattern hot paths when live MFT benchmarks are unavailable.


