// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Search benchmarks for UFFS.
//!
//! This benchmark suite measures the performance of file search operations:
//! - Pattern parsing and matching
//! - Glob to regex conversion
//! - Query building and execution
//! - Path resolution
//! - Tree index building and metric computation
//!
//! Run with: `cargo bench --bench search_benchmarks`

// Benchmark-specific lint exceptions: benchmark code uses unwrap/expect on controlled data,
// discards results, uses std types directly, and doesn't need full documentation.
#![expect(
    clippy::single_call_fn,
    reason = "benchmark functions called once by criterion_group! macro"
)]
#![expect(clippy::let_underscore_untyped, reason = "benchmarks discard results")]
#![expect(
    clippy::let_underscore_must_use,
    reason = "benchmarks discard results to measure computation"
)]
#![expect(
    clippy::unwrap_used,
    reason = "benchmark code unwraps controlled test data"
)]
#![expect(
    clippy::expect_used,
    reason = "benchmark code expects on controlled test data"
)]
#![expect(clippy::missing_docs_in_private_items, reason = "benchmark code")]
#![expect(
    clippy::shadow_reuse,
    reason = "benchmarks reuse variable names in loops"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "benchmark code uses short loop variables"
)]
#![expect(clippy::semicolon_if_nothing_returned, reason = "benchmark code style")]
#![expect(
    clippy::std_instead_of_core,
    reason = "benchmark code uses std for simplicity"
)]
#![expect(
    unused_crate_dependencies,
    reason = "uffs-mft is a transitive dependency not directly used"
)]

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use uffs_core::pattern::ParsedPattern;
use uffs_core::tree::{TreeColumn, TreeIndex};
use uffs_core::{FastPathResolver, MftQuery, PathResolver};
use uffs_polars::{Column, DataFrame};

// ═══════════════════════════════════════════════════════════════════════════
// Test Data Generation
// ═══════════════════════════════════════════════════════════════════════════

/// Create a test `DataFrame` with the specified number of rows.
/// Simulates a realistic MFT structure with directories and files.
fn create_test_dataframe(num_rows: usize) -> DataFrame {
    let mut frs_values: Vec<u64> = Vec::with_capacity(num_rows);
    let mut parent_values: Vec<u64> = Vec::with_capacity(num_rows);
    let mut name_values: Vec<String> = Vec::with_capacity(num_rows);
    let mut is_dir_values: Vec<bool> = Vec::with_capacity(num_rows);
    let mut size_values: Vec<u64> = Vec::with_capacity(num_rows);
    let mut alloc_values: Vec<u64> = Vec::with_capacity(num_rows);

    // Root directory (FRS 5)
    frs_values.push(5);
    parent_values.push(5);
    name_values.push(String::new());
    is_dir_values.push(true);
    size_values.push(0);
    alloc_values.push(0);

    // Create a tree structure: ~10% directories, ~90% files
    let mut current_dir = 5_u64;
    let mut dir_count = 0_usize;

    for i in 1..num_rows {
        let frs = (i + 100) as u64;
        frs_values.push(frs);

        // Every 10th entry is a directory
        let is_dir = i % 10 == 0;
        is_dir_values.push(is_dir);

        if is_dir {
            // Directories are children of root or previous directories
            parent_values.push(if dir_count.is_multiple_of(3) {
                5
            } else {
                current_dir
            });
            name_values.push(format!("dir_{i}"));
            size_values.push(0);
            alloc_values.push(4096);
            current_dir = frs;
            dir_count += 1;
        } else {
            // Files are children of current directory
            parent_values.push(current_dir);
            name_values.push(format!("file_{i}.txt"));
            let size = ((i * 1024) % 1_000_000) as u64;
            size_values.push(size);
            // Allocated size is cluster-aligned (4KB)
            alloc_values.push(size.div_ceil(4096) * 4096);
        }
    }

    DataFrame::new_infer_height(vec![
        Column::new("frs".into(), frs_values),
        Column::new("parent_frs".into(), parent_values),
        Column::new("name".into(), name_values),
        Column::new("is_directory".into(), is_dir_values),
        Column::new("size".into(), size_values),
        Column::new("allocated_size".into(), alloc_values),
    ])
    .expect("Failed to create test DataFrame")
}

// ═══════════════════════════════════════════════════════════════════════════
// Pattern Parsing Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_pattern_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_parsing");

    // Simple glob pattern
    group.bench_function("simple_glob", |b| {
        b.iter(|| ParsedPattern::parse(std::hint::black_box("*.txt")))
    });

    // Complex glob pattern
    group.bench_function("complex_glob", |b| {
        b.iter(|| {
            ParsedPattern::parse(std::hint::black_box(
                "c:/users/**/documents/*.{doc,docx,pdf}",
            ))
        })
    });

    // Regex pattern
    group.bench_function("regex_pattern", |b| {
        b.iter(|| ParsedPattern::parse(std::hint::black_box(r">C:\\Temp.*\.txt$")))
    });

    // Literal pattern
    group.bench_function("literal_pattern", |b| {
        b.iter(|| ParsedPattern::parse(std::hint::black_box("main")))
    });

    // Pattern with drive prefix
    group.bench_function("drive_prefix", |b| {
        b.iter(|| ParsedPattern::parse(std::hint::black_box("d:/projects/*.rs")))
    });

    group.finish();
}

fn bench_pattern_to_regex(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_to_regex");

    let patterns = [
        ("simple_glob", "*.txt"),
        ("complex_glob", "**/src/**/*.rs"),
        ("literal", "main"),
    ];

    for (name, pattern_str) in patterns {
        let parsed = ParsedPattern::parse(pattern_str).expect("valid pattern");
        group.bench_function(name, |b| b.iter(|| parsed.to_regex()));
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Query Building Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_query_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_building");

    // Test with different DataFrame sizes
    for size in [1_000, 10_000, 100_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("new", size), &df, |b, df| {
            b.iter(|| MftQuery::new(std::hint::black_box(df.clone())))
        });

        group.bench_with_input(BenchmarkId::new("files_only", size), &df, |b, df| {
            b.iter(|| MftQuery::new(std::hint::black_box(df.clone())).files_only())
        });

        group.bench_with_input(BenchmarkId::new("chained_filters", size), &df, |b, df| {
            b.iter(|| {
                MftQuery::new(std::hint::black_box(df.clone()))
                    .files_only()
                    .min_size(1024)
                    .limit(100)
            })
        });
    }

    group.finish();
}

fn bench_query_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_execution");
    group.sample_size(50); // Reduce sample size for slower benchmarks

    for size in [1_000, 10_000, 100_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("collect_all", size), &df, |b, df| {
            b.iter_batched(
                || MftQuery::new(df.clone()),
                |query: MftQuery| query.collect(),
                BatchSize::SmallInput,
            )
        });

        group.bench_with_input(
            BenchmarkId::new("files_only_collect", size),
            &df,
            |b, df| {
                b.iter_batched(
                    || MftQuery::new(df.clone()).files_only(),
                    |query: MftQuery| query.collect(),
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_with_input(BenchmarkId::new("filtered_collect", size), &df, |b, df| {
            b.iter_batched(
                || {
                    MftQuery::new(df.clone())
                        .files_only()
                        .min_size(10_000)
                        .limit(100)
                },
                |query: MftQuery| query.collect(),
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Tree Index Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_tree_index_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("tree_index_building");
    group.sample_size(30); // Reduce sample size for slower benchmarks

    for size in [1_000, 10_000, 100_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("from_dataframe", size), &df, |b, df| {
            b.iter(|| TreeIndex::from_dataframe(std::hint::black_box(df)))
        });
    }

    group.finish();
}

fn bench_tree_column_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("tree_column_computation");
    group.sample_size(20); // Reduce sample size for slower benchmarks

    for size in [1_000, 10_000, 50_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        // Benchmark adding all tree columns
        group.bench_with_input(BenchmarkId::new("all_columns", size), &df, |b, df| {
            b.iter_batched(
                || TreeIndex::from_dataframe(df).expect("valid tree"),
                |mut tree| {
                    tree.add_columns(df, &[
                        TreeColumn::Descendants,
                        TreeColumn::TreeSize,
                        TreeColumn::TreeAllocated,
                        TreeColumn::Bulkiness,
                    ])
                },
                BatchSize::SmallInput,
            )
        });

        // Benchmark adding just descendants
        group.bench_with_input(BenchmarkId::new("descendants_only", size), &df, |b, df| {
            b.iter_batched(
                || TreeIndex::from_dataframe(df).expect("valid tree"),
                |mut tree| tree.add_columns(df, &[TreeColumn::Descendants]),
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Path Resolution Benchmarks
// ═══════════════════════════════════════════════════════════════════════════

fn bench_path_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("path_resolution");

    for size in [1_000, 10_000, 100_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        // Benchmark building the resolver
        group.bench_with_input(BenchmarkId::new("build", size), &df, |b, df| {
            b.iter(|| {
                PathResolver::build(std::hint::black_box(df), uffs_mft::platform::DriveLetter::C)
            })
        });

        // Benchmark resolving paths (with caching)
        group.bench_with_input(BenchmarkId::new("resolve_cached", size), &df, |b, df| {
            b.iter_batched(
                || {
                    let resolver = PathResolver::build(df, uffs_mft::platform::DriveLetter::C)
                        .expect("valid resolver");
                    // Get some FRS values to resolve
                    let frs_col = df.column("frs").unwrap().u64().unwrap();
                    let frs_values: Vec<u64> = (0..std::cmp::min(100, df.height()))
                        .filter_map(|i| frs_col.get(i))
                        .collect();
                    (resolver, frs_values)
                },
                |(mut resolver, frs_values): (PathResolver, Vec<u64>)| {
                    for frs in frs_values {
                        let _ = resolver.resolve(frs);
                    }
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// FastPathResolver Benchmarks (Vec-based O(1) lookup)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_fast_path_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("fast_path_resolution");

    for size in [1_000, 10_000, 100_000] {
        let df = create_test_dataframe(size);

        group.throughput(Throughput::Elements(size as u64));

        // Benchmark building the fast resolver
        group.bench_with_input(BenchmarkId::new("build", size), &df, |b, df| {
            b.iter(|| {
                FastPathResolver::build(
                    std::hint::black_box(df),
                    uffs_mft::platform::DriveLetter::C,
                )
            })
        });

        // Benchmark resolving paths (with caching)
        group.bench_with_input(BenchmarkId::new("resolve_cached", size), &df, |b, df| {
            b.iter_batched(
                || {
                    let resolver = FastPathResolver::build(df, uffs_mft::platform::DriveLetter::C)
                        .expect("valid resolver");
                    // Get some FRS values to resolve
                    let frs_col = df.column("frs").unwrap().u64().unwrap();
                    let frs_values: Vec<u64> = (0..std::cmp::min(100, df.height()))
                        .filter_map(|i| frs_col.get(i))
                        .collect();
                    (resolver, frs_values)
                },
                |(resolver, frs_values): (FastPathResolver, Vec<u64>)| {
                    for frs in frs_values {
                        let _ = resolver.resolve(frs);
                    }
                },
                BatchSize::SmallInput,
            )
        });

        // Benchmark adding path column to filtered results
        group.bench_with_input(BenchmarkId::new("add_path_column", size), &df, |b, df| {
            // Simulate filtered results (10% of original)
            let filtered_size = size / 10;
            let filtered_df = create_test_dataframe(filtered_size);

            b.iter_batched(
                || {
                    let resolver = FastPathResolver::build(df, uffs_mft::platform::DriveLetter::C)
                        .expect("valid resolver");
                    (resolver, filtered_df.clone())
                },
                |(mut resolver, filtered): (FastPathResolver, DataFrame)| {
                    let _ = resolver.add_path_column(&filtered);
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// HashMap vs Vec Resolver Comparison
// ═══════════════════════════════════════════════════════════════════════════

fn bench_resolver_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolver_comparison");
    group.sample_size(50);

    // Use 100K rows for meaningful comparison
    let size = 100_000;
    let df = create_test_dataframe(size);

    group.throughput(Throughput::Elements(size as u64));

    // Compare build times
    group.bench_function("hashmap_build", |b| {
        b.iter(|| {
            PathResolver::build(
                std::hint::black_box(&df),
                uffs_mft::platform::DriveLetter::C,
            )
        })
    });

    group.bench_function("vec_build", |b| {
        b.iter(|| {
            FastPathResolver::build(
                std::hint::black_box(&df),
                uffs_mft::platform::DriveLetter::C,
            )
        })
    });

    // Compare resolve times (100 paths each)
    let frs_col = df.column("frs").unwrap().u64().unwrap();
    let frs_values: Vec<u64> = (0..100).filter_map(|i| frs_col.get(i)).collect();

    group.bench_function("hashmap_resolve_100", |b| {
        b.iter_batched(
            || {
                PathResolver::build(&df, uffs_mft::platform::DriveLetter::C)
                    .expect("valid resolver")
            },
            |mut resolver| {
                for &frs in &frs_values {
                    let _ = resolver.resolve(frs);
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("vec_resolve_100", |b| {
        b.iter_batched(
            || {
                FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)
                    .expect("valid resolver")
            },
            |resolver| {
                for &frs in &frs_values {
                    let _ = resolver.resolve(frs);
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// Benchmark parallel vs sequential path resolution.
fn bench_parallel_path_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_path_resolution");
    group.sample_size(20);

    // Use 50K rows - above the parallel threshold
    let size = 50_000;
    let df = create_test_dataframe(size);

    group.throughput(Throughput::Elements(size as u64));

    // Sequential resolution
    group.bench_function("sequential_50k", |b| {
        b.iter_batched(
            || {
                let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)
                    .expect("valid resolver");
                (resolver, df.clone())
            },
            |(mut resolver, df)| resolver.add_path_column(&df),
            BatchSize::LargeInput,
        )
    });

    // Parallel resolution
    group.bench_function("parallel_50k", |b| {
        b.iter_batched(
            || {
                let resolver = FastPathResolver::build(&df, uffs_mft::platform::DriveLetter::C)
                    .expect("valid resolver");
                (resolver, df.clone())
            },
            |(resolver, df)| resolver.add_path_column_parallel(&df),
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════════════
// Criterion Groups
// ═══════════════════════════════════════════════════════════════════════════

criterion_group!(
    pattern_benches,
    bench_pattern_parsing,
    bench_pattern_to_regex
);

criterion_group!(query_benches, bench_query_building, bench_query_execution);

criterion_group!(
    tree_benches,
    bench_tree_index_building,
    bench_tree_column_computation
);

criterion_group!(
    path_benches,
    bench_path_resolution,
    bench_fast_path_resolution,
    bench_resolver_comparison,
    bench_parallel_path_resolution
);

criterion_main!(pattern_benches, query_benches, tree_benches, path_benches);
