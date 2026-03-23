//! CLI argument definitions: `Cli` struct and `Commands` enum.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Parse a drive letter from common CLI input formats.
///
/// Accepts:
/// - Single letter: `C`, `c`
/// - With colon: `C:`, `c:`
///
/// Returns uppercase drive letter.
pub fn parse_drive_letter(input: &str) -> Result<char, String> {
    let trimmed = input.trim();
    // Strip trailing colon if present (`C:` -> `C`).
    let letter_str = trimmed.strip_suffix(':').unwrap_or(trimmed);

    if letter_str.len() != 1 {
        return Err(format!(
            "Invalid drive letter '{input}': expected single letter like 'C' or 'C:'"
        ));
    }

    let ch = letter_str
        .chars()
        .next()
        .ok_or_else(|| format!("Invalid drive letter '{input}'"))?;

    if !ch.is_ascii_alphabetic() {
        return Err(format!("Invalid drive letter '{input}': must be A-Z"));
    }

    Ok(ch.to_ascii_uppercase())
}

/// UFFS - Ultra Fast File Search using direct MFT reading
#[derive(Parser)]
#[command(name = "uffs")]
#[command(
    author,
    version,
    about = "Command-line interface for UFFS (Ultra Fast File Search)",
    long_about = "Fast NTFS search via direct Master File Table reads.\n\nSearch is the default action: pass a pattern with no subcommand to search a live volume, a saved index, or a raw MFT file. Use subcommands for index creation and offline inspection.",
    after_help = "Examples:\n  uffs '*.txt'\n  uffs '>.*\\.log$' --drive C\n  uffs '*' --mft-file G_mft.bin --drive G\n  uffs index -d C index.parquet"
)]
#[command(propagate_version = true)]
#[command(args_conflicts_with_subcommands = true)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI args struct mirrors many boolean flags from clap"
)]
pub struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Subcommand to execute (search, index, info, stats, save-raw, load-raw).
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Search pattern (glob, regex with `>`, or literal) - default action
    ///
    /// When no subcommand is specified, uffs performs a search.
    /// Examples:
    ///   `uffs *.txt`           - All .txt files
    ///   `uffs c:/pro*`         - Files starting with "pro" on C:
    ///   `uffs ">.*\.log$"`     - REGEX for .log files
    #[arg(value_name = "PATTERN", verbatim_doc_comment)]
    pub pattern: Option<String>,

    /// Drive letter to search (e.g., C or C:). Overrides drive in pattern.
    #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
    pub drive: Option<char>,

    /// Multiple drive letters to search concurrently (e.g., C,D,E or C:,D:,E:)
    #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
    pub drives: Option<Vec<char>>,

    /// Use pre-built index file instead of live MFT
    #[arg(short, long, conflicts_with_all = ["drive", "drives", "mft_file"])]
    pub index: Option<PathBuf>,

    /// Use raw MFT file(s) instead of live MFT (cross-platform)
    ///
    /// Load previously saved raw MFT files (from `uffs save-raw` or
    /// `uffs_mft save`). Drive letters are auto-inferred from filenames
    /// (e.g., `C.bin` → C:, `D_mft.bin` → D:). Use `--drive`/`--drives` to
    /// override if needed.
    ///   Single:  `uffs "*" --mft-file C.bin`
    ///   Multi:   `uffs "*" --mft-file C.bin,D.bin`
    #[arg(
        long,
        value_delimiter = ',',
        conflicts_with = "index",
        verbatim_doc_comment
    )]
    pub mft_file: Vec<PathBuf>,

    /// Show only files (exclude directories)
    #[arg(long)]
    pub files_only: bool,

    /// Show only directories
    #[arg(long)]
    pub dirs_only: bool,

    /// Hide system files (files starting with $)
    #[arg(long)]
    pub hide_system: bool,

    /// Show detailed timing breakdown for performance profiling
    #[arg(long)]
    pub profile: bool,

    /// Debug tree metrics computation (prints detailed hardlink handling info)
    #[arg(long, hide = true)]
    pub debug_tree: bool,

    /// Benchmark mode: skip output, only measure MFT reading and filtering
    /// Use this for profiling without stdout I/O overhead
    #[arg(long)]
    pub benchmark: bool,

    /// Disable MFT bitmap optimization (read ALL records)
    /// Use this for debugging if records appear to be missing
    #[arg(long)]
    pub no_bitmap: bool,

    /// Bypass cache and read MFT fresh (default: use cache)
    #[arg(long)]
    pub no_cache: bool,

    /// Minimum file size in bytes
    #[arg(long)]
    pub min_size: Option<u64>,

    /// Maximum file size in bytes
    #[arg(long)]
    pub max_size: Option<u64>,

    /// Maximum number of results (0 = unlimited)
    #[arg(short = 'n', long, default_value = "0")]
    pub limit: u32,

    /// Output format: table, json, csv, custom
    #[arg(short, long, default_value = "csv")]
    pub format: String,

    /// Case-sensitive matching (default: off)
    #[arg(long, default_value = "false")]
    pub case: bool,

    /// Smart case: auto case-sensitive if pattern contains uppercase (default:
    /// off)
    ///
    /// When enabled, patterns with ANY uppercase letter become case-sensitive
    /// automatically. Lowercase-only patterns stay case-insensitive.
    /// Like `fd --smart-case` / `ripgrep --smart-case`.
    #[arg(long, default_value = "false")]
    pub smart_case: bool,

    /// Filter by NTFS attributes (comma-separated, prefix ! to exclude)
    ///
    /// Examples: hidden, !hidden, compressed,encrypted, !system,!hidden
    /// Available: hidden, system, archive, readonly, compressed, encrypted,
    ///   sparse, reparse, offline, notindexed, temporary, virtual,
    ///   pinned, unpinned, integrity, noscrub, directory
    #[arg(long)]
    pub attr: Option<String>,

    /// Only files modified within this duration/after this date
    ///
    /// Examples: 7d (7 days), 24h (24 hours), 30m (30 minutes),
    ///   2026-01-15, 2026-01-15T10:30:00
    #[arg(long)]
    pub newer: Option<String>,

    /// Only files modified before this duration/date
    #[arg(long)]
    pub older: Option<String>,

    /// Only files created within this duration/after this date
    #[arg(long)]
    pub newer_created: Option<String>,

    /// Only files created before this duration/date
    #[arg(long)]
    pub older_created: Option<String>,

    /// Only files accessed within this duration/after this date
    #[arg(long)]
    pub newer_accessed: Option<String>,

    /// Only files accessed before this duration/date
    #[arg(long)]
    pub older_accessed: Option<String>,

    /// Exclude files matching this pattern (applied after main pattern)
    ///
    /// Example: uffs *.txt --exclude backup*
    #[arg(long)]
    pub exclude: Option<String>,

    /// Whole word matching (wraps pattern in \b...\b regex)
    ///
    /// Example: uffs --word nice  (finds "nice" but not "nicehouse")
    #[arg(long, default_value = "false")]
    pub word: bool,

    /// Sort results by column(s), comma-separated for multi-tier
    ///
    /// Examples: size, modified, name, size,name, modified,size,name
    /// Available: size, sizeondisk, modified, created, accessed, name,
    ///   ext, descendants, hidden, system, archive, readonly,
    ///   compressed, encrypted, directory
    #[arg(long)]
    pub sort: Option<String>,

    /// Reverse sort order (descending)
    #[arg(long, default_value = "false")]
    pub sort_desc: bool,

    /// Filter by file extension(s)
    #[arg(long)]
    pub ext: Option<String>,

    /// Output destination: console or filename
    #[arg(long, default_value = "console")]
    pub out: String,

    /// Columns to output (comma-separated or "all")
    /// Default: all columns.
    #[arg(long, default_value = "all")]
    pub columns: String,

    /// Column separator (default: comma)
    #[arg(long, default_value = ",")]
    pub sep: String,

    /// Quote character for string values (default: double quote)
    #[arg(long, default_value = "\"")]
    pub quotes: String,

    /// Include header row in output.
    #[arg(long, default_value = "true")]
    pub header: bool,

    /// Representation for active/true boolean attributes
    #[arg(long, default_value = "1")]
    pub pos: String,

    /// Representation for inactive/false boolean attributes
    #[arg(long, default_value = "0")]
    pub neg: String,

    /// Query execution mode: auto, index, dataframe
    ///
    /// - auto: Automatically choose best path (default)
    /// - index: Force fast `MftIndex` path (simple queries only)
    /// - dataframe: Force Polars `DataFrame` path (full features)
    #[arg(long, default_value = "auto", verbatim_doc_comment)]
    pub query_mode: String,

    /// Override timezone offset for timestamp display (hours from UTC).
    ///
    /// By default, timestamps are displayed in the current local timezone.
    /// Use this to force a specific offset, e.g. for reproducible parity
    /// testing when the reference was generated in a different DST period.
    ///
    /// Examples: -8 (PST), -7 (PDT), 0 (UTC), 1 (CET), 9 (JST)
    #[arg(long, allow_hyphen_values = true)]
    pub tz_offset: Option<i32>,

    /// Chaos mode seed for testing (randomizes chunk order).
    ///
    /// Only works with `--mft-file`. Reads MFT chunks in pseudo-random order
    /// to verify that directory index merging works correctly regardless of
    /// read order. Used for regression testing.
    #[arg(long, hide = true)]
    pub chaos_seed: Option<u64>,

    /// NTFS reserved cluster bytes to add to root directory's `Size on Disk`.
    ///
    /// C++ adds `(TotalReserved + MftZoneEnd - MftZoneStart) *
    /// BytesPerCluster` to the root. This flag lets parity verification pass
    /// the same value when reading from offline `.iocp` captures that don't
    /// embed volume metadata.
    #[arg(long, hide = true)]
    pub reserved_allocated: Option<u64>,
}

/// Available CLI subcommands.
///
/// Note: Search is NOT a subcommand - it's the default action.
/// This matches ripgrep/fd/Everything patterns where the tool name IS the
/// search.
#[derive(Subcommand)]
pub enum Commands {
    /// Build an index from drive MFT(s)
    ///
    /// By default, indexes ALL available NTFS drives. Use --drive or --drives
    /// to limit to specific drives.
    ///
    /// If no extension is provided, defaults to `.parquet`.
    ///
    /// Examples:
    ///   uffs index index.parquet           # Index ALL drives
    ///   uffs index -d C index.parquet      # Index only C: drive
    ///   uffs index --drives C,D,E out.parquet  # Index C:, D:, E:
    ///   uffs index myindex                 # Creates myindex.parquet
    Index {
        /// Output file path (extension defaults to .parquet)
        output: PathBuf,

        /// Drive letter to index (limits to single drive)
        #[arg(short, long, conflicts_with = "drives", value_parser = parse_drive_letter)]
        drive: Option<char>,

        /// Multiple drive letters to index (e.g., C,D,E)
        #[arg(long, value_delimiter = ',', conflicts_with = "drive", value_parser = parse_drive_letter)]
        drives: Option<Vec<char>>,
    },

    /// Show information about an index file
    Info {
        /// Index file path
        path: PathBuf,
    },

    /// Show statistics about files in an index
    Stats {
        /// Index file path
        path: PathBuf,

        /// Show top N largest files
        #[arg(long, default_value = "10")]
        top: u32,
    },
}
