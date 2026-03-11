# Performance Docs

Use this bucket for benchmark reports, optimization notes, and validation results.

## Current entry points

- [`MFTINDEX_OPTIMIZATION_PLAN.md`](MFTINDEX_OPTIMIZATION_PLAN.md)
- [`UFFS_PERFORMANCE_OPTIMIZATION_PHASE2.md`](UFFS_PERFORMANCE_OPTIMIZATION_PHASE2.md)
- [`uffs-mft-optimization-plan.md`](uffs-mft-optimization-plan.md)
- [`uffs_mft_optimization_plan_review.md`](uffs_mft_optimization_plan_review.md)
- [`../architecture/PHASE7_PERFORMANCE_VALIDATION.md`](../architecture/PHASE7_PERFORMANCE_VALIDATION.md)

This landing page gives the retained performance work a stable home after the docs-root cleanup.

## Focused cross-platform benchmark lane

- Hot-path query benchmark: `cargo bench -p uffs-core --bench query`
- Benchmark source: [`../../crates/uffs-core/benches/query.rs`](../../crates/uffs-core/benches/query.rs)

Use this lane for non-Windows performance checks on query/pattern hot paths when live MFT benchmarks are unavailable.


