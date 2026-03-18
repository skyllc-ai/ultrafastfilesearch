//! Command dispatch for the `uffs_mft` binary.

use anyhow::Result;

use crate::cli::Commands;

mod load;
#[cfg(windows)]
mod windows;

/// Dispatches parsed CLI commands to their handlers.
#[cfg(windows)]
pub async fn dispatch_command(command: Commands) -> Result<()> {
    match command {
        Commands::Read {
            drive,
            output,
            mode,
            full,
            unique,
            forensic,
        } => windows::cmd_read(drive, output, &mode, full, unique, forensic).await,
        Commands::Info {
            drive,
            deep,
            no_bitmap,
            unique,
        } => windows::cmd_info(drive, deep, no_bitmap, unique).await,
        Commands::Drives => windows::cmd_drives().await,
        Commands::Bench {
            drive,
            json,
            no_df,
            runs,
            mode,
            full,
        } => windows::cmd_bench(drive, json, no_df, runs, &mode, full).await,
        Commands::BenchAll {
            output,
            no_df,
            runs,
            full,
        } => windows::cmd_bench_all(output, no_df, runs, full).await,
        Commands::BitmapDiag { drive, samples } => windows::cmd_bitmap_diag(drive, samples).await,
        Commands::Save {
            drive,
            output,
            no_compress,
            compression_level,
            raw,
            iocp,
            iocp_concurrency,
        } => {
            windows::cmd_save(
                drive,
                &output,
                !no_compress,
                compression_level,
                raw,
                iocp,
                iocp_concurrency,
            )
            .await
        }
        Commands::Load {
            input,
            output,
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        } => load::cmd_load(
            &input,
            output.as_deref(),
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        ),
        Commands::BenchmarkMft { drive } => windows::cmd_benchmark_mft(drive).await,
        Commands::BenchmarkIndex { drive } => windows::cmd_benchmark_index(drive).await,
        Commands::BenchmarkIndexLean {
            drive,
            mode,
            no_bitmap,
            no_placeholders,
            concurrency,
            io_size_kb,
            parallel_parse,
            parse_workers,
        } => {
            windows::cmd_benchmark_index_lean(
                drive,
                &mode,
                no_bitmap,
                no_placeholders,
                concurrency,
                io_size_kb,
                parallel_parse,
                parse_workers,
            )
            .await
        }
        Commands::BenchmarkTree {
            drive,
            iterations,
            no_cache,
        } => windows::cmd_benchmark_tree(drive, iterations, no_cache).await,
        Commands::BenchmarkMultiVolume { drives } => {
            windows::cmd_benchmark_multi_volume(drives).await
        }
        Commands::UsnInfo { drive } => windows::cmd_usn_info(drive).await,
        Commands::UsnRead {
            drive,
            start_usn,
            limit,
        } => windows::cmd_usn_read(drive, start_usn, limit).await,
        Commands::IndexSave { drive, output } => windows::cmd_index_save(drive, &output).await,
        Commands::IndexLoad { input } => windows::cmd_index_load(&input).await,
        Commands::CacheStatus { clean, purge } => windows::cmd_cache_status(clean, purge).await,
        Commands::CacheGet { drive, force, ttl } => windows::cmd_cache_get(drive, force, ttl).await,
        Commands::CacheClear { drive, all } => windows::cmd_cache_clear(drive, all).await,
        Commands::IndexUpdate {
            drive,
            force_full,
            ttl,
        } => windows::cmd_index_update(drive, force_full, ttl).await,
        Commands::IndexAll {
            drives,
            no_cache,
            ttl,
        } => windows::cmd_index_all(drives, no_cache, ttl).await,
    }
}

/// Command dispatcher for non-Windows platforms (limited functionality).
///
/// Only the `load` command works on non-Windows platforms.
#[cfg(not(windows))]
#[expect(
    clippy::unused_async,
    reason = "async for api parity with windows implementation"
)]
#[expect(
    clippy::single_call_fn,
    reason = "logical separation of command dispatch"
)]
pub async fn dispatch_command(command: Commands) -> Result<()> {
    match command {
        Commands::Load {
            input,
            output,
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        } => load::cmd_load(
            &input,
            output.as_deref(),
            info_only,
            build_index,
            debug_tree,
            drive,
            forensic,
        ),
        Commands::Read { .. }
        | Commands::Info { .. }
        | Commands::Drives
        | Commands::Bench { .. }
        | Commands::BenchAll { .. }
        | Commands::BitmapDiag { .. }
        | Commands::Save { .. }
        | Commands::BenchmarkMft { .. }
        | Commands::BenchmarkIndex { .. }
        | Commands::BenchmarkIndexLean { .. }
        | Commands::BenchmarkTree { .. }
        | Commands::BenchmarkMultiVolume { .. }
        | Commands::UsnInfo { .. }
        | Commands::UsnRead { .. }
        | Commands::IndexSave { .. }
        | Commands::IndexLoad { .. }
        | Commands::CacheStatus { .. }
        | Commands::CacheGet { .. }
        | Commands::CacheClear { .. }
        | Commands::IndexUpdate { .. }
        | Commands::IndexAll { .. } => {
            anyhow::bail!(
                "This command requires Windows.
                 Only the 'load' command works on macOS/Linux for parsing saved MFT files."
            );
        }
    }
}
