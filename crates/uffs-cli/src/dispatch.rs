// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! First-token command dispatch for `uffs` (design:
//! `docs/architecture/cli-grammar.md`).
//!
//! UFFS is **search-first**: `uffs <anything>` searches for `<anything>`.
//! The first token decides the mode — a known `--command` runs that command;
//! anything else (bare word, glob, single dash, or a search flag) is a
//! search. The command set is deliberately **disjoint** from every
//! search-flag long name, so `uffs --ext pdf` is a (pattern-less) search
//! while `uffs --update` is the updater.

use anyhow::Result;

use crate::commands;

/// A management command — the ONLY first tokens that switch `uffs` out of
/// search mode. Every variant's token is `--<command>` and is disjoint from
/// every search-flag long name (`--sort`, `--ext`, `--drive`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Explicit search (the bare positional default also routes here).
    Search,
    /// `--stats [path]`.
    Stats,
    /// `--agg <preset>`.
    Agg,
    /// `--daemon <action>`.
    Daemon,
    /// `--mcp <action>`.
    Mcp,
    /// `--update [action]`.
    Update,
    /// `--status`.
    Status,
}

impl Command {
    /// Map a first token to its [`Command`], or `None` when it is not a
    /// management command (→ the token is treated as a search pattern/flag).
    pub(crate) fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "--search" => Self::Search,
            "--stats" => Self::Stats,
            "--agg" | "--aggregate" => Self::Agg,
            "--daemon" => Self::Daemon,
            "--mcp" => Self::Mcp,
            "--update" => Self::Update,
            "--status" => Self::Status,
            _ => return None,
        })
    }
}

/// Dispatch a resolved [`Command`] to its handler. `args` is everything
/// after the `--<command>` token.
///
/// # Errors
///
/// Propagates the underlying command's failure.
pub(crate) fn dispatch_command(command: Command, args: &[String]) -> Result<()> {
    match command {
        Command::Search => crate::run_search(args),
        Command::Stats => crate::run_stats(args),
        Command::Agg => crate::run_aggregate(args),
        Command::Daemon => crate::run_daemon(args),
        Command::Mcp => commands::mcp_mgmt::mcp_from_args(args),
        Command::Update => commands::update::run_update(args),
        Command::Status => {
            run_status(args);
            Ok(())
        }
    }
}

/// `--status` — combined daemon + broker + MCP status (never fails).
fn run_status(args: &[String]) {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        crate::args::print_status_help();
    } else {
        commands::system_status::system_status();
    }
}

#[cfg(test)]
mod tests {
    use super::Command;

    #[test]
    fn command_tokens_resolve() {
        assert_eq!(Command::from_token("--update"), Some(Command::Update));
        assert_eq!(Command::from_token("--daemon"), Some(Command::Daemon));
        assert_eq!(Command::from_token("--mcp"), Some(Command::Mcp));
        assert_eq!(Command::from_token("--stats"), Some(Command::Stats));
        assert_eq!(Command::from_token("--agg"), Some(Command::Agg));
        assert_eq!(Command::from_token("--aggregate"), Some(Command::Agg));
        assert_eq!(Command::from_token("--status"), Some(Command::Status));
        assert_eq!(Command::from_token("--search"), Some(Command::Search));
    }

    #[test]
    fn bare_and_single_dash_tokens_are_patterns_not_commands() {
        // The whole point of the grammar: the old reserved words are
        // searchable again, and single-dash tokens stay patterns.
        for pattern in [
            "update", "status", "daemon", "mcp", "stats", "agg", "report", "help", "version",
            "-update", "*.pdf", "-x",
        ] {
            assert_eq!(
                Command::from_token(pattern),
                None,
                "`{pattern}` must be a search pattern, not a command"
            );
        }
    }

    #[test]
    fn search_flags_are_never_commands() {
        // The disjointness invariant: a search flag as the FIRST token stays
        // in search mode (e.g. `uffs --ext pdf` is a pattern-less search), so
        // it must never resolve to a command.
        for flag in [
            "--ext",
            "--sort",
            "--drive",
            "--limit",
            "--format",
            "--tz-offset",
            "--out",
            "--no-output",
            "--profile",
            "--agg-format",
        ] {
            assert_eq!(
                Command::from_token(flag),
                None,
                "search flag `{flag}` must NOT be a management command"
            );
        }
    }
}
