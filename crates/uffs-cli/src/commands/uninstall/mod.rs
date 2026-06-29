// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs --uninstall` — full removal of the UFFS family from the machine.
//!
//! Design + plan:
//! - `docs/dev/architecture/UFFS-Uninstall-Feasibility-and-Design.md`
//! - `docs/dev/architecture/UFFS-Uninstall-Implementation-Plan.md`
//!
//! This is the command entry point. M1 implements the read-only **analysis**
//! (the binary resolution table); the plan, consent, and removal phases land in
//! sibling modules as the later milestones progress.

mod analyze;
mod args;
mod effects;
mod inventory;
mod journal;
mod plan;
mod remove;
mod render;
mod resolve_order;
mod sweep;
mod verify;

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use args::UninstallArgs;
use plan::{PlanTarget, RemovalPlan};

/// Entry point for `uffs --uninstall`. `args` is every token after the
/// `--uninstall` command token.
///
/// # Errors
///
/// Propagates argument-parse failures (and, in later milestones, analysis and
/// removal failures).
pub(crate) fn run_uninstall(args: &[String]) -> Result<()> {
    let parsed = UninstallArgs::parse(args)?;
    if parsed.help {
        print_help();
        return Ok(());
    }

    // M9 crash-awareness: if a prior uninstall was interrupted, say so. Because
    // removal is idempotent, this (re-)run simply completes it.
    if journal::was_interrupted() {
        render::print_resumed_note();
    }

    // M1 analysis: reuse the self-update Phase-A detection for the binary
    // resolution table, then inventory the non-binary artifacts.
    let report = crate::commands::update::detect();
    let candidates = analyze::build_candidates(&report);
    let resolved = resolve_order::group_and_resolve(&candidates, &analyze::search_dirs());
    let inventory = inventory::collect();
    // M2: turn the analysis into an ordered removal plan (read-only).
    let removal_plan = plan::build_plan(&report, &inventory, &parsed, &analyze::path_entries());

    if parsed.json {
        render::print_json(&resolved, &inventory, &removal_plan);
        return Ok(());
    }

    render::print_resolution_table(&resolved);
    render::print_inventory(&inventory);
    render::print_plan(&removal_plan);

    // M7 deep sweep: while the daemon is still up, ask UFFS itself for stray
    // family files elsewhere on the indexed drives. Read-only; reported only.
    if !parsed.no_deep_sweep {
        let known = plan_dirs(&removal_plan);
        let mut search = sweep::DaemonSearch;
        if let Ok(strays) = sweep::find_strays(&mut search, &known) {
            render::print_strays(&strays);
        }
    }

    if parsed.dry_run {
        print_dry_run_footer();
        return Ok(());
    }

    // M3 elevation gate (U-30): refuse before any effect when the plan needs
    // privilege the current process lacks. `uffs_mft::platform::is_elevated` is
    // cross-platform (Windows token check; Unix effective-uid 0), unlike the
    // Windows-only `uffs_winsvc::is_elevated`.
    if removal_plan.requires_elevation() && !uffs_mft::platform::is_elevated() {
        render::print_elevation_refusal(&removal_plan);
        bail!("uninstall needs Administrator for the items listed above; re-run elevated");
    }

    if removal_plan.is_empty() {
        return Ok(());
    }

    // M4 consent (U-21): unless --yes, require explicit confirmation (default No)
    // before any destructive effect.
    if !parsed.assume_yes && !confirm_removal()? {
        print_aborted();
        return Ok(());
    }

    // M9: mark the run in progress (survives the lifecycle-dir deletion) so an
    // interruption is detectable next launch. Best-effort: a failed marker write
    // must not block the uninstall, but we surface it honestly.
    if let Err(err) = journal::begin() {
        render::print_journal_warning(&err);
    }

    // M4 execute (U-40..42): run the ordered plan against the live effects sink,
    // best-effort. The outcome reports what was removed and what failed.
    let mut effects = effects::SystemEffects::new();
    let outcome = remove::execute(&removal_plan, &mut effects);
    render::print_outcome(&outcome);

    // M8 self-delete (U-80): the running uffs.exe (+ uffs-update.exe) cannot
    // delete themselves in place; schedule a deferred delete. If even scheduling
    // fails, say so rather than hiding it.
    let self_paths = self_binaries();
    if let Err(err) = effects::schedule_self_delete(&self_paths) {
        render::print_self_delete_warning(&err);
    }

    // M8 verify (U-81): confirm the targeted locations are gone, excluding the
    // reboot-deferred self-binaries handled above.
    let to_check: Vec<PathBuf> = plan_dirs(&removal_plan)
        .into_iter()
        .filter(|dir| {
            !self_paths
                .iter()
                .any(|self_path| self_path.starts_with(dir))
        })
        .collect();
    render::print_verification(&verify::still_present(&to_check));

    // M9: clear the in-progress marker now the run finished.
    if let Err(err) = journal::finish() {
        render::print_journal_warning(&err);
    }
    Ok(())
}

/// The running self-binaries that cannot be deleted in place: the current
/// `uffs` executable and its sibling `uffs-update`.
fn self_binaries() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let mut paths = vec![exe.clone()];
    if let Some(dir) = exe.parent() {
        let updater = if cfg!(windows) {
            "uffs-update.exe"
        } else {
            "uffs-update"
        };
        paths.push(dir.join(updater));
    }
    paths
}

/// The directories the plan acts on, used to dedup deep-sweep hits (a stray
/// already inside a planned dir is not a separate finding).
fn plan_dirs(plan: &RemovalPlan) -> Vec<PathBuf> {
    plan.items()
        .filter_map(|item| match &item.target {
            PlanTarget::DeleteBinaries { dir, .. }
            | PlanTarget::DelegateWinget { dir, .. }
            | PlanTarget::RemovePathEntry { dir } => Some(dir.clone()),
            PlanTarget::DeleteDir { path, .. } => Some(path.clone()),
            PlanTarget::StopProcess { .. } | PlanTarget::RemoveService { .. } => None,
        })
        .collect()
}

/// Prompt for confirmation before any removal. Default (empty / anything but
/// `y`/`yes`) is **No**.
#[expect(clippy::print_stdout, reason = "interactive CLI prompt")]
fn confirm_removal() -> Result<bool> {
    use std::io::Write as _;

    print!("\nProceed with removal? [y/N] ");
    std::io::stdout()
        .flush()
        .context("flushing the confirmation prompt")?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading confirmation")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Footer printed after a `--dry-run` plan.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_dry_run_footer() {
    println!("\nDry run: nothing was removed.");
}

/// Message printed when the user declines the confirmation.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_aborted() {
    println!("Aborted. Nothing was removed.");
}

/// Print `uffs --uninstall` usage.
#[expect(clippy::print_stdout, reason = "intentional help output")]
fn print_help() {
    println!(
        "uffs --uninstall — remove UFFS and all of its data from this machine\n\
         \n\
         USAGE:\n\
         \x20 uffs --uninstall [flags]\n\
         \n\
         FLAGS:\n\
         \x20 --dry-run         Show the analysis + removal plan, change nothing\n\
         \x20 --yes, -y         Skip the confirmation prompt\n\
         \x20 --keep-config     Remove binaries + caches but keep settings/config\n\
         \x20 --no-deep-sweep   Skip the cross-drive search for stray UFFS files\n\
         \x20 --no-path         Do not edit PATH (print a manual hint instead)\n\
         \x20 --scope <s>       Restrict to user | machine | all (default: all)\n\
         \x20 --json            Emit the analysis + plan as JSON\n\
         \x20 --help, -h        Show this help"
    );
}
