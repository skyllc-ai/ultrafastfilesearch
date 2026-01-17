//! Benchmarks for query operations.

// Suppress lints for benchmarks
#![allow(clippy::missing_docs_in_private_items)]
#![allow(clippy::single_call_fn)]
#![allow(clippy::integer_division_remainder_used)]
#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::min_ident_chars)]
#![allow(clippy::default_numeric_fallback)]
#![allow(clippy::std_instead_of_core)]
#![allow(clippy::semicolon_if_nothing_returned)]
#![allow(unused_crate_dependencies)]

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_placeholder(criterion: &mut Criterion) {
    criterion.bench_function("query_placeholder", |bencher| {
        bencher.iter(|| {
            // TODO: Add actual query benchmarks
            core::hint::black_box(1_i32 + 1_i32)
        });
    });
}

criterion_group!(benches, bench_placeholder);
criterion_main!(benches);
