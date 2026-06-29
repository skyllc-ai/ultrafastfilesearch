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
mod inventory;
mod plan;
mod render;
mod resolve_order;

use anyhow::{Result, bail};
use args::UninstallArgs;

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

    // M1 analysis: reuse the self-update Phase-A detection for the binary
    // resolution table, then inventory the non-binary artifacts.
    let report = crate::commands::update::detect();
    let candidates = analyze::build_candidates(&report);
    let resolved = resolve_order::group_and_resolve(&candidates, &analyze::search_dirs());
    let inventory = inventory::collect();
    // M2: turn the analysis into an ordered removal plan (read-only).
    let removal_plan = plan::build_plan(&report, &inventory, &parsed);

    if parsed.json {
        render::print_json(&resolved, &inventory, &removal_plan);
        return Ok(());
    }

    render::print_resolution_table(&resolved);
    render::print_inventory(&inventory);
    render::print_plan(&removal_plan);

    if parsed.dry_run {
        print_dry_run_footer();
        return Ok(());
    }

    // M3 elevation gate (U-30): refuse before any effect when the plan needs
    // Administrator the current process does not have.
    if removal_plan.requires_elevation() && !uffs_winsvc::is_elevated() {
        render::print_elevation_refusal(&removal_plan);
        bail!("uninstall needs Administrator for the items listed above; re-run elevated");
    }

    // M4+ : interactive consent + the removal engine land here.
    print_pending_removal_notice();
    Ok(())
}

/// Footer printed after a `--dry-run` plan.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_dry_run_footer() {
    println!("\nDry run: nothing was removed.");
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

/// Notice printed on the would-remove path until the removal engine (M4+)
/// lands.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_pending_removal_notice() {
    println!(
        "\nThe removal engine is not implemented yet (M4+); no changes were made.\n\
         Use `uffs --uninstall --dry-run` to review the plan."
    );
}
