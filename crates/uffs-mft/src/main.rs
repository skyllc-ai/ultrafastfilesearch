//! # `uffs_mft`: MFT Command-Line Tool
//!
//! Low-level tool for reading and exporting NTFS Master File Table data.
//!
//! ## Usage
//!
//! ```bash
//! # Read MFT from C: drive and export to Parquet
//! uffs_mft read --drive C --output c_drive.parquet
//!
//! # Show MFT information for a drive
//! uffs_mft info --drive C
//!
//! # List all NTFS drives
//! uffs_mft drives
//! ```
//!
//! ## Logging
//!
//! Use `-v` / `--verbose` for debug-level terminal output:
//! ```bash
//! uffs_mft -v info --drive C
//! ```
//!
//! For finer control, use environment variables:
//! - `RUST_LOG`: Terminal log level (default: `info`, or `debug` with `-v`)
//! - `RUST_LOG_FILE`: File log level (default: `info`)
//! - `UFFS_LOG_DIR`: Log directory (default: `~/bin/uffs/logs`)
//!
//! **Note**: This tool requires Administrator privileges on Windows.

// ============================================================================
// Suppress unused crate warnings
// ============================================================================
// These dependencies are used by the uffs-mft library, not this binary.
// Cargo doesn't support per-binary dependencies, so we suppress the warnings
// here.

// CLI binaries have single-call functions by design (one per command)
#![expect(
    clippy::single_call_fn,
    reason = "CLI command functions are called once from dispatch"
)]

use anyhow::Result;
use bitflags as _;
use clap::Parser;
// Dev-dependencies (used in benchmarks and tests only)
#[cfg(test)]
use criterion as _;
// Pipelining dependencies (used in io.rs PipelinedMftReader on Windows)
#[cfg(windows)]
use crossbeam_channel as _;
// Platform-gated dependencies (used on Windows only)
#[cfg(not(windows))]
use indicatif as _;
#[cfg(test)]
use proptest as _;
use rayon as _;
use rustc_hash as _;
// SmallVec for path chain building (used in index.rs PathResolver)
use smallvec as _;
use thiserror as _;
#[cfg(not(windows))]
use tracing as _;
#[cfg(not(windows))]
use uffs_mft as _;
use uffs_polars as _;
// Optional dependencies
#[cfg(feature = "zstd")]
use zstd as _;
// Benchmark dependencies (used by bench/bench-all commands on Windows)
#[cfg(not(windows))]
use {chrono as _, hostname as _, num_cpus as _};

/// CLI definitions for the `uffs_mft` binary.
mod cli;
/// Command dispatch and handlers for the `uffs_mft` binary.
mod commands;
/// Formatting and display helpers shared by command handlers.
mod display;
/// Logging initialization support.
mod logging;
/// Progress display support for long-running commands.
mod progress;

use crate::cli::Cli;

#[tokio::main]
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing error output in main"
)]
async fn main() {
    let verbose = std::env::args().any(|arg| arg == "-v" || arg == "--verbose");
    let _guard = logging::init_logging(verbose);

    if let Err(err) = run().await {
        eprintln!("Error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("  Caused by: {cause}");
        }
        std::process::exit(1);
    }
}

/// Main application logic, separated from `main()` for clean error handling.
async fn run() -> Result<()> {
    let cli = Cli::parse();
    commands::dispatch_command(cli.command).await
}
