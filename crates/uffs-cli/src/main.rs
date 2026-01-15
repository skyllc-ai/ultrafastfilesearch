//! `UltraFastFileSearch` CLI
//!
//! Fast file search from the command line.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tracing_subscriber::EnvFilter;

mod commands;

/// `UltraFastFileSearch` - Lightning-fast file search using direct MFT reading
#[derive(Parser)]
#[command(name = "uffs")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search for files matching a pattern
    Search {
        /// Search pattern (glob syntax: *.rs, **/*.txt)
        pattern: String,

        /// Drive letter to search (e.g., C)
        #[arg(short, long)]
        drive: Option<char>,

        /// Use pre-built index file instead of live MFT
        #[arg(short, long)]
        index: Option<PathBuf>,

        /// Show only files (exclude directories)
        #[arg(long)]
        files_only: bool,

        /// Show only directories
        #[arg(long)]
        dirs_only: bool,

        /// Minimum file size in bytes
        #[arg(long)]
        min_size: Option<u64>,

        /// Maximum file size in bytes
        #[arg(long)]
        max_size: Option<u64>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value = "100")]
        limit: u32,

        /// Output format: table, json, csv
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// Build an index from a drive's MFT
    Index {
        /// Drive letter to index (e.g., C)
        #[arg(short, long)]
        drive: char,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Show information about an index file
    Info {
        /// Index file path
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Search {
            pattern,
            drive,
            index,
            files_only,
            dirs_only,
            min_size,
            max_size,
            limit,
            format,
        } => {
            commands::search(
                &pattern, drive, index, files_only, dirs_only, min_size, max_size, limit, &format,
            )
            .await?;
        }
        Commands::Index { drive, output } => {
            commands::index(drive, &output).await?;
        }
        Commands::Info { path } => {
            commands::info(&path)?;
        }
    }

    Ok(())
}

/// Create a progress bar with a nice style.
#[allow(dead_code)]
fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}

