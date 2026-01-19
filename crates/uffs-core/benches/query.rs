//! Additional query benchmarks for UFFS (Windows only).
//!
//! These benchmarks complement `search_benchmarks.rs` with:
//! - Glob pattern matching performance
//! - Regex pattern matching performance
//! - Extension filter performance
//!
//! Run with: `cargo bench -p uffs-core --bench query`

// Suppress lints for benchmarks
#![allow(clippy::missing_docs_in_private_items)]
#![allow(clippy::single_call_fn)]
#![allow(clippy::integer_division_remainder_used)]
#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::min_ident_chars)]
#![allow(clippy::default_numeric_fallback)]
#![allow(clippy::std_instead_of_core)]
#![allow(clippy::semicolon_if_nothing_returned)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(unused_crate_dependencies)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::shadow_reuse)]
#![allow(clippy::redundant_closure_for_method_calls)]

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use uffs_core::MftQuery;
use uffs_core::extensions::ExtensionFilter;
use uffs_core::pattern::ParsedPattern;
use uffs_polars::{Column, DataFrame};

// ═══════════════════════════════════════════════════════════════════════════
// Test Data Generation
// ═══════════════════════════════════════════════════════════════════════════

/// Create a test DataFrame with realistic file names and extensions.
fn create_test_dataframe(num_rows: usize) -> DataFrame {
    let extensions = [
        "txt", "rs", "log", "dll", "exe", "pdf", "jpg", "png", "mp4", "doc",
    ];
    let prefixes = [
        "file", "document", "image", "video", "data", "report", "backup",
    ];

    let mut frs_values: Vec<u64> = Vec::with_capacity(num_rows);
    let mut parent_values: Vec<u64> = Vec::with_capacity(num_rows);
    let mut name_values: Vec<String> = Vec::with_capacity(num_rows);
    let mut is_dir_values: Vec<bool> = Vec::with_capacity(num_rows);
    let mut size_values: Vec<u64> = Vec::with_capacity(num_rows);

    // Root directory
    frs_values.push(5);
    parent_values.push(5);
    name_values.push(String::new());
    is_dir_values.push(true);
    size_values.push(0);

    for i in 1..num_rows {
        let frs = (i + 100) as u64;
        let is_dir = i % 10 == 0;
        let ext = extensions[i % extensions.len()];
        let prefix = prefixes[i % prefixes.len()];

        frs_values.push(frs);
        parent_values.push(5);
        name_values.push(if is_dir {
            format!("dir_{i}")
        } else {
            format!("{prefix}_{i}.{ext}")
        });
        is_dir_values.push(is_dir);
        size_values.push((i * 1024) as u64);
    }

    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), frs_values.as_slice()),
        Column::new("parent_frs".into(), parent_values.as_slice()),
        Column::new("name".into(), name_values.as_slice()),
        Column::new("is_directory".into(), is_dir_values.as_slice()),
        Column::new("size".into(), size_values.as_slice()),
    ])
    .expect("valid dataframe")
}

// ═══════════════════════════════════════════════════════════════════════════
// Glob Pattern Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_glob_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/glob");

    let patterns = [
        ("simple_ext", "*.txt"),
        ("prefix_ext", "file_*.rs"),
        ("any_char", "file_?.txt"),
        ("complex", "*report*2024*.pdf"),
    ];

    for size in [10_000, 100_000] {
        let df = create_test_dataframe(size);
        group.throughput(Throughput::Elements(size as u64));

        for (name, pattern) in &patterns {
            let bench_name = format!("{name}/{size}");
            group.bench_with_input(BenchmarkId::from_parameter(&bench_name), &df, |b, df| {
                b.iter_batched(
                    || MftQuery::new(df.clone()),
                    |query| query.glob(pattern).map(|q| q.collect()),
                    BatchSize::SmallInput,
                );
            });
        }
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension Filter Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_extension_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/extension_filter");

    let filters = [
        ("single", "txt"),
        ("multiple", "txt,rs,log"),
        ("collection", "pictures"),
        ("mixed", "pictures,mp4,doc"),
    ];

    for size in [10_000, 100_000] {
        let df = create_test_dataframe(size);
        group.throughput(Throughput::Elements(size as u64));

        for (name, filter_str) in &filters {
            let bench_name = format!("{name}/{size}");
            let filter = ExtensionFilter::parse(filter_str).expect("valid filter");

            group.bench_with_input(BenchmarkId::from_parameter(&bench_name), &df, |b, df| {
                b.iter_batched(
                    || MftQuery::new(df.clone()),
                    |query| query.extension_filter(&filter).collect(),
                    BatchSize::SmallInput,
                );
            });
        }
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Parsed Pattern Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_parsed_pattern(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/parsed_pattern");

    let patterns = [
        ("glob", "*.txt"),
        ("regex", "regex:file_[0-9]+\\.rs"),
        ("literal", "literal:exact_name.txt"),
    ];

    for size in [10_000, 100_000] {
        let df = create_test_dataframe(size);
        group.throughput(Throughput::Elements(size as u64));

        for (name, pattern_str) in &patterns {
            let bench_name = format!("{name}/{size}");
            let parsed = ParsedPattern::parse(pattern_str).expect("valid pattern");

            group.bench_with_input(BenchmarkId::from_parameter(&bench_name), &df, |b, df| {
                b.iter_batched(
                    || MftQuery::new(df.clone()),
                    |query| query.pattern(&parsed).map(|q| q.collect()),
                    BatchSize::SmallInput,
                );
            });
        }
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Criterion Groups
// ═══════════════════════════════════════════════════════════════════════════

criterion_group!(
    query_benches,
    bench_glob_patterns,
    bench_extension_filter,
    bench_parsed_pattern
);

criterion_main!(query_benches);
