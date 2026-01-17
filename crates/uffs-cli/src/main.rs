//! UFFS (Ultra Fast File Search) CLI
//!
//! Fast file search from the command line.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;

/// UFFS - Ultra Fast File Search using direct MFT reading
#[derive(Parser)]
#[command(name = "uffs")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
#[command(args_conflicts_with_subcommands = true)]
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Subcommand to execute (search, index, info, stats, save-raw, load-raw).
    #[command(subcommand)]
    command: Option<Commands>,

    /// Search pattern (glob, regex with `>`, or literal) - default action
    ///
    /// When no subcommand is specified, uffs performs a search.
    /// Examples:
    ///   `uffs *.txt`           - All .txt files
    ///   `uffs c:/pro*`         - Files starting with "pro" on C:
    ///   `uffs ">.*\.log$"`     - REGEX for .log files
    #[arg(value_name = "PATTERN")]
    pattern: Option<String>,

    /// Drive letter to search (e.g., C). Overrides drive in pattern.
    #[arg(short, long, conflicts_with = "drives")]
    drive: Option<char>,

    /// Multiple drive letters to search concurrently (e.g., C,D,E)
    #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
    drives: Option<Vec<char>>,

    /// Use pre-built index file instead of live MFT
    #[arg(short, long, conflicts_with_all = ["drive", "drives"])]
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

    /// Case-sensitive matching (default: off)
    #[arg(long, default_value = "false")]
    case: bool,

    /// Filter by file extension(s)
    #[arg(long)]
    ext: Option<String>,

    /// Output destination: console or filename
    #[arg(long, default_value = "console")]
    out: String,

    /// Columns to output (comma-separated or "all")
    #[arg(long, default_value = "all")]
    columns: String,

    /// Column separator (default: comma)
    #[arg(long, default_value = ",")]
    sep: String,

    /// Quote character for string values
    #[arg(long, default_value = "\"")]
    quotes: String,

    /// Include header row in output
    #[arg(long, default_value = "true")]
    header: bool,

    /// Representation for active/true boolean attributes
    #[arg(long, default_value = "1")]
    pos: String,

    /// Representation for inactive/false boolean attributes
    #[arg(long, default_value = "0")]
    neg: String,
}

/// Available CLI subcommands.
#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Search for files matching a pattern
    ///
    /// Supports multiple pattern syntaxes:
    /// - Glob: `*.txt`, `**/*.rs`, `file?.doc`
    /// - Drive prefix: `c:/pro*`, `D:\Users\*`
    /// - REGEX: `">C:\\Temp.*"` (patterns starting with `>`)
    /// - Literal: `readme` (no wildcards = substring match)
    Search {
        /// Search pattern (glob, regex with `>`, or literal)
        ///
        /// Examples:
        ///   `*.txt`           - All .txt files
        ///   `c:/pro*`         - Files starting with "pro" on C:
        ///   `**/*.rs`         - All .rs files recursively
        ///   `">.*\.log$"`     - REGEX for .log files
        pattern: String,

        /// Drive letter to search (e.g., C). Overrides drive in pattern.
        #[arg(short, long, conflicts_with = "drives")]
        drive: Option<char>,

        /// Multiple drive letters to search concurrently (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
        drives: Option<Vec<char>>,

        /// Use pre-built index file instead of live MFT
        #[arg(short, long, conflicts_with_all = ["drive", "drives"])]
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

        /// Case-sensitive matching (default: off)
        #[arg(long, default_value = "false")]
        case: bool,

        /// Filter by file extension(s)
        ///
        /// Accepts comma-separated extensions or collection aliases:
        /// - Extensions: `jpg,png,gif`
        /// - Collections: `pictures`, `documents`, `videos`, `music`,
        ///   `archives`, `code`
        /// - Mixed: `pictures,mp4,pdf`
        #[arg(long)]
        ext: Option<String>,

        /// Output destination: console or filename
        ///
        /// Special values: console, con, term, terminal (all output to stdout)
        /// Otherwise treated as a file path to write results to.
        #[arg(long, default_value = "console")]
        out: String,

        /// Columns to output (comma-separated or "all")
        ///
        /// Available: path, name, pathonly, size, sizeondisk, created,
        /// modified, accessed, type, attributes, attributevalue,
        /// hidden, system, archive, readonly, compressed, encrypted,
        /// sparse, reparse, offline, notindexed, temporary, virtual,
        /// pinned, unpinned, descendants
        #[arg(long, default_value = "all")]
        columns: String,

        /// Column separator (default: comma)
        ///
        /// Special values: TAB, NEWLINE
        #[arg(long, default_value = ",")]
        sep: String,

        /// Quote character for string values
        #[arg(long, default_value = "\"")]
        quotes: String,

        /// Include header row in output
        #[arg(long, default_value = "true")]
        header: bool,

        /// Representation for active/true boolean attributes
        #[arg(long, default_value = "1")]
        pos: String,

        /// Representation for inactive/false boolean attributes
        #[arg(long, default_value = "0")]
        neg: String,
    },

    /// Build an index from a drive's MFT
    Index {
        /// Drive letter to index (e.g., C). Use --drives for multiple drives.
        #[arg(short, long, conflicts_with = "drives")]
        drive: Option<char>,

        /// Multiple drive letters to index concurrently (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive")]
        drives: Option<Vec<char>>,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Show information about an index file
    Info {
        /// Index file path
        path: PathBuf,
    },

    /// Show statistics about files in an index
    Stats {
        /// Index file path
        #[arg(short, long)]
        index: PathBuf,

        /// Show top N largest files
        #[arg(long, default_value = "10")]
        top: u32,
    },

    /// Save raw MFT bytes to a file for offline analysis
    SaveRaw {
        /// Drive letter to read MFT from (e.g., C)
        #[arg(short, long)]
        drive: char,

        /// Output file path for raw MFT data
        #[arg(short, long)]
        output: PathBuf,

        /// Compress the output using zstd
        #[arg(short, long, default_value = "true")]
        compress: bool,

        /// Compression level (1-22, default 3)
        #[arg(long, default_value = "3")]
        compression_level: i32,
    },

    /// Load raw MFT from a saved file and export to parquet/csv
    LoadRaw {
        /// Input raw MFT file path
        input: PathBuf,

        /// Output file path (parquet or csv based on extension)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Show info about the raw MFT file only (don't parse)
        #[arg(long)]
        info_only: bool,
    },
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    // Handle default search (no subcommand) or explicit subcommand
    match cli.command {
        Some(Commands::Search {
            pattern,
            drive,
            drives,
            index,
            files_only,
            dirs_only,
            min_size,
            max_size,
            limit,
            format,
            case,
            ext,
            out,
            columns,
            sep,
            quotes,
            header,
            pos,
            neg,
        }) => {
            commands::search(
                &pattern,
                drive,
                drives,
                index,
                files_only,
                dirs_only,
                min_size,
                max_size,
                limit,
                &format,
                case,
                ext.as_deref(),
                &out,
                &columns,
                &sep,
                &quotes,
                header,
                &pos,
                &neg,
            )
            .await?;
        }
        Some(Commands::Index {
            drive,
            drives,
            output,
        }) => {
            commands::index(drive, drives, &output).await?;
        }
        Some(Commands::Info { path }) => {
            commands::info(&path)?;
        }
        Some(Commands::Stats { index, top }) => {
            commands::stats(&index, top)?;
        }
        Some(Commands::SaveRaw {
            drive,
            output,
            compress,
            compression_level,
        }) => {
            commands::save_raw(drive, &output, compress, compression_level).await?;
        }
        Some(Commands::LoadRaw {
            input,
            output,
            info_only,
        }) => {
            commands::load_raw(&input, output.as_deref(), info_only)?;
        }
        None => {
            // Default action: search with top-level arguments
            if let Some(pattern) = cli.pattern {
                commands::search(
                    &pattern,
                    cli.drive,
                    cli.drives,
                    cli.index,
                    cli.files_only,
                    cli.dirs_only,
                    cli.min_size,
                    cli.max_size,
                    cli.limit,
                    &cli.format,
                    cli.case,
                    cli.ext.as_deref(),
                    &cli.out,
                    &cli.columns,
                    &cli.sep,
                    &cli.quotes,
                    cli.header,
                    &cli.pos,
                    &cli.neg,
                )
                .await?;
            } else {
                // No pattern provided - show help
                use clap::CommandFactory;
                Cli::command().print_help()?;
            }
        }
    }

    Ok(())
}
