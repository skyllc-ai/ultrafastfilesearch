// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Command-line surface for the orchestrator binary.
//!
//! The [`Cli`] type is defined here in the library (rather than in `main.rs`)
//! so the flag surface and its derived defaults are unit-testable through the
//! same `MockHost` lane as the rest of the crate, and so `clap` is a genuine
//! library dependency. `main.rs` is a thin shim that parses [`Cli`] and calls
//! [`crate::run::run`].
//!
//! The three execution modes are mutually exclusive (a `clap` argument group),
//! defaulting to [`Mode::Guided`] when none is given; see implementation-guide
//! §4 for the mode semantics.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::gate::Mode;

/// Default tool ids when `--tools` is omitted (the full head-to-head set).
pub const DEFAULT_TOOLS: [&str; 3] = ["uffs", "uffs_cpp", "everything"];

/// Optional subcommands; an absent subcommand runs the full benchmark suite.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Download + SHA-256-verify the pinned competitor binary into the bundle.
    ///
    /// Reads `scripts/windows/competitors.toml`, fetches the pinned `es.exe`
    /// artifact into `<bundle>/tools/`, verifies its hash (failing closed on a
    /// mismatch), and records the result as a tracked acquisition in
    /// `state.json` with the `--keep-tools` disposition.
    #[command(name = "fetch-competitors")]
    FetchCompetitors,

    /// Replay a bundle's `restore-manifest.json` after a hard kill.
    ///
    /// Re-applies every restore closure serialized during the run to return
    /// the host to its as-found state. Fails closed (non-zero exit) when any
    /// undo cannot be replayed. On success the manifest is reset so a second
    /// replay is a no-op. Requires `--bundle <dir>`.
    Restore,

    /// Re-capture the host fingerprint and diff it against a bundle's baseline.
    ///
    /// Compares the current host state against `fingerprint-before.json` from
    /// a previous run. Fails closed (non-zero exit) when any difference is
    /// detected; writes `fingerprint-after.json` for forensics. Requires
    /// `--bundle <dir>`.
    Verify,
}

/// Robust, reproducible benchmark-suite orchestrator for UFFS.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a CLI flag surface is a flat bag of independent boolean switches: \
              the three mode flags are already collapsed into one mutually \
              exclusive clap group, and redo/force/drop_os_cache/keep_tools are \
              orthogonal operator toggles, not a state machine"
)]
#[derive(Parser, Debug, Clone)]
#[command(name = "uffs-bench", version, about, long_about = None)]
pub struct Cli {
    /// Teach each step in full the first time, then prompt tersely (default).
    #[arg(long, group = "mode")]
    pub guided: bool,
    /// Run every step with no prompts (snapshot/restore still run).
    #[arg(long, group = "mode")]
    pub auto: bool,
    /// Render every card but perform zero mutations.
    #[arg(long = "dry-run", group = "mode")]
    pub dry_run: bool,

    /// Drives to benchmark (comma-separated letters, for example `C,D`).
    #[arg(long, value_delimiter = ',')]
    pub drives: Vec<char>,
    /// Tool ids participating in the run (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub tools: Vec<String>,
    /// Number of measurement rounds per cell.
    #[arg(long, default_value_t = 10)]
    pub rounds: u32,

    /// Run only this single stage (1-based).
    #[arg(long = "only-stage")]
    pub only_stage: Option<u32>,
    /// Resume from this stage onward (1-based).
    #[arg(long = "from-stage")]
    pub from_stage: Option<u32>,

    /// Resume an existing bundle directory (loads its `state.json`).
    #[arg(long)]
    pub bundle: Option<PathBuf>,
    /// Root directory under which a new timestamped bundle is created.
    #[arg(long = "bundle-root", default_value = ".")]
    pub bundle_root: PathBuf,

    /// Re-run the next pending step even if it is cached.
    #[arg(long)]
    pub redo: bool,
    /// Invalidate the whole resume cache (re-run every step).
    #[arg(long)]
    pub force: bool,

    /// Drop OS file-cache before cold measurements.
    #[arg(long = "drop-os-cache")]
    pub drop_os_cache: bool,
    /// Keep any tools the suite acquired (default: remove at teardown).
    #[arg(long = "keep-tools")]
    pub keep_tools: bool,

    /// Optional subcommand (e.g. `fetch-competitors`); absent runs the suite.
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Resolve the execution [`Mode`] from the mutually-exclusive flags.
    ///
    /// Defaults to [`Mode::Guided`] when neither `--auto` nor `--dry-run` is
    /// given (the explicit `--guided` flag selects the same default).
    #[must_use]
    pub const fn mode(&self) -> Mode {
        if self.auto {
            Mode::AutoPilot
        } else if self.dry_run {
            Mode::DryRun
        } else {
            Mode::Guided
        }
    }

    /// Tool ids for the run, falling back to [`DEFAULT_TOOLS`] when unset.
    #[must_use]
    pub fn tools_or_default(&self) -> Vec<String> {
        if self.tools.is_empty() {
            DEFAULT_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect()
        } else {
            self.tools.clone()
        }
    }

    /// Candidate drives, uppercased; defaults to `C` when none are given.
    #[must_use]
    pub fn drives_or_default(&self) -> Vec<char> {
        if self.drives.is_empty() {
            vec!['C']
        } else {
            self.drives.iter().map(char::to_ascii_uppercase).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::Cli;
    use crate::gate::Mode;

    #[test]
    fn defaults_are_guided_with_ten_rounds() {
        let cli = Cli::parse_from(["uffs-bench"]);
        assert_eq!(cli.mode(), Mode::Guided);
        assert_eq!(cli.rounds, 10);
        assert_eq!(cli.drives_or_default(), vec!['C']);
        assert_eq!(cli.tools_or_default(), vec![
            "uffs",
            "uffs_cpp",
            "everything"
        ]);
    }

    #[test]
    fn auto_and_dry_run_are_mutually_exclusive() {
        let parsed = Cli::try_parse_from(["uffs-bench", "--auto", "--dry-run"]);
        parsed.expect_err("--auto and --dry-run must be mutually exclusive");
    }

    #[test]
    fn parses_drives_tools_and_modes() {
        let cli = Cli::parse_from(["uffs-bench", "--auto", "--drives", "c,d", "--tools", "uffs"]);
        assert_eq!(cli.mode(), Mode::AutoPilot);
        assert_eq!(cli.drives_or_default(), vec!['C', 'D']);
        assert_eq!(cli.tools_or_default(), vec!["uffs"]);
    }
}
