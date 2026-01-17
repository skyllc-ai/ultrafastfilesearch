//! Benchmarks for MFT reading operations.

// Suppress unused crate warnings for dependencies used by the main crate but
// not by benchmarks. Note: zstd is an optional dependency, so we conditionally
// suppress it.
use criterion::{Criterion, criterion_group, criterion_main};
use {
    anyhow as _, bitflags as _, clap as _, indicatif as _, rayon as _, thiserror as _, tokio as _,
    tracing as _, tracing_subscriber as _, uffs_mft as _, uffs_polars as _,
};

// zstd is optional - only suppress when the feature is enabled
#[cfg(feature = "zstd")]
extern crate zstd as _;

/// Placeholder benchmark for MFT reading operations.
#[allow(clippy::single_call_fn)] // Required by criterion_group! macro
fn bench_placeholder(criterion: &mut Criterion) {
    criterion.bench_function("placeholder", |bencher| {
        bencher.iter(|| {
            // TODO: Add actual MFT reading benchmarks
            core::hint::black_box(1_i32 + 1_i32)
        });
    });
}

criterion_group!(benches, bench_placeholder);
criterion_main!(benches);
