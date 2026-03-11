//! Benchmarks for MFT reading operations (Windows only).
//!
//! Run these benchmarks on Windows in elevated `PowerShell`:
//! ```powershell
//! cargo bench -p uffs-mft --bench mft_read
//! ```
//!
//! For real-world MFT benchmarks, use the CLI:
//! ```powershell
//! uffs_mft bench --drive C --runs 3
//! uffs_mft bench-all
//! ```

// Suppress unused crate warnings for dependencies used by the main crate.
use anyhow as _;
use bitflags as _;
use chrono as _;
use clap as _;
use criterion as _;
use dirs_next as _;
use hostname as _;
use indicatif as _;
use proptest as _;
use rayon as _;
use rustc_hash as _;
use smallvec as _;
use thiserror as _;
use tokio as _;
use tracing as _;
use tracing_appender as _;
use tracing_subscriber as _;
use uffs_mft as _;
use uffs_polars as _;

#[cfg(feature = "zstd")]
extern crate zstd as _;

// ═══════════════════════════════════════════════════════════════════════════
// Windows-only benchmarks (AlignedBuffer and ParsedColumns are Windows-only)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(windows)]
mod windows_benches {
    use criterion::{
        BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
    };
    use uffs_mft::{AlignedBuffer, ParsedColumns};

    /// Benchmark AlignedBuffer allocation at various sizes (4KB to 8MB).
    fn bench_aligned_buffer_alloc(c: &mut Criterion) {
        let mut group = c.benchmark_group("aligned_buffer/alloc");

        for size in [4096, 65536, 1_048_576, 8_388_608] {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
                b.iter(|| core::hint::black_box(AlignedBuffer::new(size)));
            });
        }

        group.finish();
    }

    /// Benchmark AlignedBuffer write throughput.
    fn bench_aligned_buffer_write(c: &mut Criterion) {
        let mut group = c.benchmark_group("aligned_buffer/write");

        for size in [4096, 65536, 1_048_576] {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
                b.iter_batched(
                    || AlignedBuffer::new(size),
                    |mut buffer: AlignedBuffer| {
                        let slice = buffer.as_mut_slice();
                        for byte in slice.iter_mut() {
                            *byte = 0x42;
                        }
                        core::hint::black_box(buffer)
                    },
                    BatchSize::SmallInput,
                );
            });
        }

        group.finish();
    }

    /// Create a ParsedColumns with synthetic MFT-like data.
    fn create_test_columns(count: usize) -> ParsedColumns {
        let mut cols = ParsedColumns::with_capacity(count);

        for i in 0..count {
            cols.frs.push(i as u64);
            cols.parent_frs
                .push(if i > 0 { (i / 10) as u64 } else { 5 });
            cols.name.push(format!("file_{i}.txt"));
            cols.size.push((i * 1024) as u64);
            cols.allocated_size
                .push(((i * 1024 + 4095) / 4096 * 4096) as u64);
            cols.created.push(1_700_000_000_000_i64 + i as i64);
            cols.modified.push(1_700_000_000_000_i64 + i as i64);
            cols.accessed.push(1_700_000_000_000_i64 + i as i64);
            cols.mft_changed.push(1_700_000_000_000_i64 + i as i64);
            cols.is_directory.push(i % 10 == 0);
            cols.name_count.push(1);
            cols.stream_count.push(1);
            cols.is_readonly.push(false);
            cols.is_hidden.push(i % 100 == 0);
            cols.is_system.push(false);
            cols.is_archive.push(true);
            cols.is_compressed.push(false);
            cols.is_encrypted.push(false);
            cols.is_sparse.push(false);
            cols.is_reparse.push(false);
            cols.is_offline.push(false);
            cols.is_not_indexed.push(false);
            cols.is_temporary.push(false);
            cols.is_integrity_stream.push(false);
            cols.is_no_scrub_data.push(false);
            cols.is_pinned.push(false);
            cols.is_unpinned.push(false);
            cols.is_virtual.push(false);
        }

        cols
    }

    /// Benchmark ParsedColumns allocation with pre-allocated capacity.
    fn bench_parsed_columns_alloc(c: &mut Criterion) {
        let mut group = c.benchmark_group("parsed_columns/alloc");

        for count in [1_000, 10_000, 100_000, 1_000_000] {
            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
                b.iter(|| core::hint::black_box(ParsedColumns::with_capacity(count)));
            });
        }

        group.finish();
    }

    /// Benchmark ParsedColumns extend (simulates Rayon reduce phase).
    fn bench_parsed_columns_extend(c: &mut Criterion) {
        let mut group = c.benchmark_group("parsed_columns/extend");

        for count in [1_000, 10_000, 100_000] {
            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
                b.iter_batched(
                    || (create_test_columns(count), create_test_columns(count)),
                    |(mut cols1, cols2): (ParsedColumns, ParsedColumns)| {
                        cols1.extend(cols2);
                        core::hint::black_box(cols1)
                    },
                    BatchSize::SmallInput,
                );
            });
        }

        group.finish();
    }

    /// Benchmark ParsedColumns to DataFrame conversion.
    fn bench_parsed_columns_to_dataframe(c: &mut Criterion) {
        let mut group = c.benchmark_group("parsed_columns/to_dataframe");
        group.sample_size(20); // Expensive operation

        for count in [1_000, 10_000, 100_000] {
            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
                b.iter_batched(
                    || create_test_columns(count),
                    |cols: ParsedColumns| core::hint::black_box(cols.into_dataframe()),
                    BatchSize::SmallInput,
                );
            });
        }

        group.finish();
    }

    criterion_group!(
        buffer_benches,
        bench_aligned_buffer_alloc,
        bench_aligned_buffer_write
    );

    criterion_group!(
        columns_benches,
        bench_parsed_columns_alloc,
        bench_parsed_columns_extend,
        bench_parsed_columns_to_dataframe
    );

    criterion_main!(buffer_benches, columns_benches);
}

// Non-Windows stub - benchmarks only run on Windows
#[cfg(not(windows))]
fn main() {
    // Benchmarks only run on Windows - this is a no-op stub
}
