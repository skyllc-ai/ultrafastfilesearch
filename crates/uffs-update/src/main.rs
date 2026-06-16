// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs-update` — the self-update **acquire** helper binary.
//!
//! A separate binary from `uffs` so the HTTP/TLS stack (reqwest + rustls)
//! never bloats the lean CLI. `uffs update` spawns this for the
//! download/verify step only; detect + snapshot stay in `uffs-cli`.
//!
//! ```text
//! uffs-update acquire --snapshot <path> --stage <dir>
//!                     [--repo <owner/name>] [--version <tag>] [--sums <asset>]
//! ```

mod acquire;
mod apply;
mod github;
mod journal;
mod orchestrate;
mod plan;
mod proc;
mod quiesce;
mod recover;
mod restore;
mod verify;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::acquire::AcquirePlan;

/// Entry point. Returns a non-zero exit on any failure.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("acquire") => run_acquire(args.get(1..).unwrap_or_default()),
        Some("apply") => run_apply(args.get(1..).unwrap_or_default()),
        Some("recover") => run_recover(args.get(1..).unwrap_or_default()),
        Some("--help" | "-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            bail!("unknown subcommand `{other}` (try `acquire` / `apply` / `recover`)")
        }
    }
}

/// Parse the `apply` flags and run the full journal-driven flow:
/// quiesce → backup/swap/smoke → commit → restore → prune. On any
/// pre-commit failure the binaries are rolled back **and** the stopped
/// services are restarted (INV-1: never leave a service down).
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn run_apply(args: &[String]) -> Result<()> {
    let snapshot_path = PathBuf::from(required(flag(args, "--snapshot"), "--snapshot <path>")?);
    let stage = PathBuf::from(required(flag(args, "--stage"), "--stage <dir>")?);
    let update_dir = stage
        .parent()
        .map_or_else(|| stage.clone(), Path::to_path_buf);

    let snapshot = plan::Snapshot::load(&snapshot_path)?;
    let backup_dir = update_dir.join(format!("backup-{}", std::process::id()));
    let journal_path = update_dir.join("journal.json");
    let mut journal = orchestrate::journal_from_snapshot(journal_path, &snapshot, backup_dir);
    // Record the snapshot so Phase H can restore service state on recovery.
    journal.snapshot_ref = Some(snapshot_path.display().to_string());
    journal.transition(journal::UpdateState::Acquired, "apply.acquired")?;

    // Pre-flight BEFORE touching any service: prove every staged binary
    // is present and every target dir is writable. A failure here is
    // zero-downtime — nothing is quiesced or swapped yet (§19, Phase D).
    if let Err(err) = orchestrate::preflight(&journal, &stage) {
        journal.transition(journal::UpdateState::Aborted, "preflight.failed")?;
        journal.archive();
        return Err(err);
    }
    journal.transition(journal::UpdateState::PreflightOk, "preflight.ok")?;

    // Stop the resident services so their files unlock.
    quiesce::quiesce(&mut journal, &snapshot)?;

    // Swap + smoke + commit; on failure the binaries are already rolled
    // back — now restart the services we stopped (INV-1) and abort.
    if let Err(err) = orchestrate::apply_all(&mut journal, &stage, |target| {
        apply::smoke_ok(target, orchestrate::SMOKE_ARG)
    }) {
        let failed = restore::restore(&snapshot);
        journal.transition(
            journal::UpdateState::Aborted,
            &format!("apply.aborted; restart_failed=[{}]", failed.join(", ")),
        )?;
        journal.archive();
        return Err(err);
    }

    // Committed: relaunch services into the new binaries.
    let failed = restore::restore(&snapshot);
    journal.transition(journal::UpdateState::Restored, "restore.done")?;
    if !failed.is_empty() {
        #[expect(clippy::print_stderr, reason = "CLI user-facing warning")]
        {
            eprintln!(
                "warning: components failed to restart: [{}]",
                failed.join(", ")
            );
        }
    }

    orchestrate::prune_all(&journal);
    journal.transition(journal::UpdateState::Done, "apply.done")?;
    journal.archive();
    println!("Applied + committed → {}", journal.to_version);
    Ok(())
}

/// Parse the `recover` flags and run Phase H against a journal.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn run_recover(args: &[String]) -> Result<()> {
    let journal_path = PathBuf::from(required(flag(args, "--journal"), "--journal <path>")?);
    let message = match recover::recover(&journal_path)? {
        recover::Recovery::NothingToDo => "nothing to do",
        recover::Recovery::InProgress => "another update is in progress (owner alive)",
        recover::Recovery::RolledForward => "interrupted update resumed → completed",
        recover::Recovery::RolledBack => "interrupted update rolled back to the previous version",
    };
    println!("recover: {message}");
    Ok(())
}

/// Default upstream repository for self-update artifacts.
const DEFAULT_REPO: &str = "skyllc-ai/UltraFastFileSearch";

/// Parse the `acquire` flags and download + verify the installed-subset
/// binaries from the snapshot.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn run_acquire(args: &[String]) -> Result<()> {
    let snapshot_path = PathBuf::from(required(flag(args, "--snapshot"), "--snapshot <path>")?);
    let stage = PathBuf::from(required(flag(args, "--stage"), "--stage <dir>")?);
    let snapshot = plan::Snapshot::load(&snapshot_path)?;

    // The installed subset across every unmanaged root, deduplicated.
    let mut binaries: Vec<String> = snapshot
        .unmanaged_targets()
        .flat_map(|target| target.binaries.iter().map(|binary| binary.name.clone()))
        .collect();
    binaries.sort();
    binaries.dedup();
    if binaries.is_empty() {
        bail!("no unmanaged binaries to acquire (WinGet roots are delegated to winget)");
    }

    let plan = AcquirePlan {
        repo: flag(args, "--repo").unwrap_or_else(|| DEFAULT_REPO.to_owned()),
        tag: flag(args, "--version"),
        stage: stage.clone(),
        sums: flag(args, "--sums").unwrap_or_else(|| "SHA256SUMS".to_owned()),
        binaries,
    };
    let staged = acquire::run(&plan)?;
    println!(
        "Acquired + verified {} binaries into {}",
        staged.len(),
        stage.display()
    );
    Ok(())
}

/// Return the value following `name` in `args` (`--name value`).
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

/// Turn a missing required flag into a clear error.
fn required(value: Option<String>, what: &str) -> Result<String> {
    value.ok_or_else(|| anyhow::anyhow!("missing required {what}"))
}

/// Print usage to stdout.
#[expect(clippy::print_stdout, reason = "intentional help output")]
fn print_usage() {
    println!(
        "uffs-update — self-update acquire + apply + recover helper\n\n\
         USAGE:\n\
         \x20 uffs-update acquire --snapshot <path> --stage <dir> \\\n\
         \x20                     [--repo <owner/name>] [--version <tag>] [--sums <asset>]\n\
         \x20 uffs-update apply   --snapshot <path> --stage <dir>\n\
         \x20 uffs-update recover --journal <path>\n\n\
         acquire: per the snapshot's installed subset, downloads each binary\n\
         as an individual release asset + SHA256SUMS, SHA-256-verifies each,\n\
         and leaves them staged. It does not replace anything (apply phase).\n"
    );
}
