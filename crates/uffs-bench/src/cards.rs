// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Gate [`Card`] and [`StepResult`] builders for the staged orchestrator.
//!
//! These pure presentation helpers turn a stage's plan and bundle paths into
//! the operator-facing card shown by [`crate::gate::confirm`] and the
//! [`StepResult`] echoed by [`crate::gate::done_panel`]. They are split out of
//! [`mod@crate::run`] so the orchestrator module stays focused on control flow;
//! the stage "vocabulary" they depend on (`STAGE0_ID`, `ASSEMBLY_ID`,
//! `stage_banner`, `stage_step_id`) still lives with the run loop.

use std::path::Path;

use crate::cli::Cli;
use crate::gate::{Card, StepResult};
use crate::report;
use crate::run::{ASSEMBLY_ID, STAGE0_ID, stage_banner, stage_step_id};
use crate::stages::StagePlan;

/// Build the Stage 0e plan-gate [`Card`].
pub(crate) fn plan_card(bundle_dir: &Path) -> Card {
    Card {
        id: STAGE0_ID.to_owned(),
        stage: "STAGE 0: PLAN".to_owned(),
        step_num: 1,
        step_total: 1,
        title: "Confirm environment, competitor preflight, and negotiated matrix".to_owned(),
        why: "Lock the apples-to-apples plan before any measurement runs.".to_owned(),
        commands: Vec::new(),
        resources: vec![bundle_dir.display().to_string()],
        backups: Vec::new(),
        est_time: "~5-20 s".to_owned(),
        recovery: "Read-only: nothing is mutated, so an abort restores nothing.".to_owned(),
        long_why: "The plan above is derived entirely from read-only probes; \
                    proceeding writes the Stage 0 artifacts into the bundle and \
                    unlocks the measurement stages."
            .to_owned(),
    }
}

/// Build a measurement-stage [`Card`] from the stage's [`StagePlan`].
///
/// The plan's commands/resources/backups are shown verbatim, upholding the
/// transparency guarantee (the card shows exactly what
/// [`crate::stages::run_stage`] will run for the same stage and config).
pub(crate) fn measurement_card(stage: u32, plan: &StagePlan) -> Card {
    let banner = stage_banner(stage);
    let title = format!("{banner} measurements");
    Card {
        id: stage_step_id(stage),
        stage: banner,
        step_num: 1,
        step_total: 1,
        title,
        why: "Time the negotiated cells for each participating tool.".to_owned(),
        commands: plan.commands.clone(),
        resources: plan.resources.clone(),
        backups: plan.backups.clone(),
        est_time: plan.est_time.clone(),
        recovery: "Snapshotted resources (daemon run-state, caches) are restored \
                    at teardown, in reverse order."
            .to_owned(),
        long_why: "Each measurement stage registers its snapshot restores on the \
                    run guard *before* mutating, shells out through the host seam, \
                    and writes its artifacts into the bundle; teardown (or an early \
                    return) undoes every snapshot in LIFO order."
            .to_owned(),
    }
}

/// The [`StepResult`] for a completed Stage 0.
pub(crate) fn stage0_result(bundle_dir: &Path) -> StepResult {
    StepResult {
        code: Some(0_i32),
        summary: "Plan locked; Stage 0 artifacts written.".to_owned(),
        output_path: Some(bundle_dir.join("matrix.json").display().to_string()),
    }
}

/// Build the Stage 4 assembly [`Card`].
///
/// Assembly is read-only with respect to host state — it only reads the
/// bundle's existing artifacts and writes the draft back into the bundle — so
/// its recovery note reflects that no host resource is touched.
pub(crate) fn assembly_card(bundle_dir: &Path) -> Card {
    Card {
        id: ASSEMBLY_ID.to_owned(),
        stage: "STAGE 4: ASSEMBLY".to_owned(),
        step_num: 1,
        step_total: 1,
        title: format!("Assemble the bundle into {}", report::REPORT_DRAFT),
        why: "Scaffold a dated, reviewable report draft from the run's artifacts.".to_owned(),
        commands: Vec::new(),
        resources: vec![bundle_dir.join(report::REPORT_DRAFT).display().to_string()],
        backups: Vec::new(),
        est_time: "~1 s".to_owned(),
        recovery: "Bundle-only: no host resource is touched, so an abort restores nothing."
            .to_owned(),
        long_why: "Assembly reads the Stage 0/1/2/3 artifacts already in the bundle, \
                    renders the environment table, negotiated matrix, and raw-log \
                    citations, and writes a *draft* report into the bundle. The draft \
                    is never auto-committed; promotion into docs/benchmarks/ is manual."
            .to_owned(),
    }
}

/// Human-readable coverage-scope label for the report header — the requested
/// drives as `"C:, D:, …"`, or `"full"` when none were narrowed.
///
/// The promotion *filename* slugifies this back to a compact `cd…` form (see
/// `report::promotion_name`), so the readable display and the filename stay in
/// sync from one source.
pub(crate) fn report_scope(cli: &Cli) -> String {
    let drives = cli.drives_or_default();
    if drives.is_empty() {
        return "full".to_owned();
    }
    drives
        .iter()
        .map(|letter| format!("{}:", letter.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Map a probe id to a human-readable product name.
///
/// `everything` and `everything_gui` are two probes of the same product.
fn tool_display_name(id: &str) -> &str {
    match id {
        "uffs" => "UFFS",
        "uffs_cpp" => "UFFS (C++ ref)",
        "everything" | "everything_gui" => "Everything",
        _ => id,
    }
}

/// Deduplicate a list of probe ids into unique, ordered product display names.
fn unique_product_names(ids: &[&str]) -> Vec<String> {
    let mut seen = alloc::collections::BTreeSet::new();
    ids.iter()
        .map(|id| tool_display_name(id).to_owned())
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

/// Build the tool-selection gate [`Card`] shown after the env table.
///
/// Always fires — even when all tools are present — so the operator can
/// confirm (or in the future, deselect) which products will be benchmarked.
/// When some tools are missing, the card notes them and points to the table.
pub(crate) fn tool_selection_card(available: &[&str], missing: &[&str], step_total: u32) -> Card {
    let avail_products = unique_product_names(available);
    let avail_names = avail_products.join(" and ");
    let avail_count = avail_products.len();
    let (title, why, long_why) = if missing.is_empty() {
        (
            format!("Benchmark {avail_names} — confirm tool selection"),
            format!("All {avail_count} product(s) found. Confirm to continue."),
            format!(
                "Proceeding will benchmark: {avail_names}.\n\
                 Press [q] to abort and adjust the tool list."
            ),
        )
    } else {
        let missing_products = unique_product_names(missing).join(", ");
        (
            format!("Benchmark {avail_names} — proceed or quit to install missing tools first?"),
            format!(
                "Not found: {missing_products} (see install links in table above). \
                 Proceeding benchmarks only the {avail_count} available product(s)."
            ),
            format!(
                "Missing: {missing_products}.\n\
                 The table above shows install URLs for each missing tool.\n\
                 Install the binaries and re-run for a full comparison, or\n\
                 proceed now with: {avail_names}."
            ),
        )
    };
    Card {
        id: "tool-selection".to_owned(),
        stage: "STAGE 0: PREFLIGHT".to_owned(),
        step_num: 1,
        step_total,
        title,
        why,
        commands: Vec::new(),
        resources: avail_products
            .iter()
            .map(|name| format!("✓  {name}"))
            .collect(),
        backups: Vec::new(),
        est_time: "0 s".to_owned(),
        recovery: "Read-only — aborting changes nothing.".to_owned(),
        long_why,
    }
}

/// Build the UFFS daemon restart confirmation [`Card`].
///
/// Shown after the negotiated matrix when the daemon is currently loaded with
/// more drives than the negotiated set.  The bench must kill the running daemon
/// and restart it restricted to the capable drives so measurements are
/// not polluted by index load on unused drives.
pub(crate) fn uffs_restart_card(capable_drives: &[char], step_num: u32, step_total: u32) -> Card {
    let drive_list: String = capable_drives
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let drive_args: String = capable_drives
        .iter()
        .map(|ch| format!("--drive {ch}"))
        .collect::<Vec<_>>()
        .join(" ");
    let title = format!("Restart UFFS daemon restricted to negotiated drives: {drive_list}");
    let why = format!(
        "The daemon is currently loaded with more drives than the negotiated set ({drive_list}). \
         The bench will kill it and restart it with only those drives so index RAM, \
         warmup time, and query routing are confined to the drives under test."
    );
    let cmd_kill = "uffs daemon kill".to_owned();
    let cmd_start = format!("uffs daemon start {drive_args}");
    Card {
        id: "uffs-daemon-restart".to_owned(),
        stage: "STAGE 0: PREFLIGHT".to_owned(),
        step_num,
        step_total,
        title,
        why: why.clone(),
        commands: vec![cmd_kill, cmd_start],
        resources: capable_drives
            .iter()
            .map(|ch| format!("{ch}: (UFFS index)"))
            .collect(),
        backups: vec!["uffs daemon run-state: restored to as-found state on teardown".to_owned()],
        est_time: "~10-60 s (index load)".to_owned(),
        recovery: "Aborting here keeps the daemon in its current state — all drives remain loaded."
            .to_owned(),
        long_why: format!(
            "{why}\n\nThe daemon is stopped with `uffs daemon kill` (hard stop) rather than \
             `restart` because `restart` reloads with the previous drive set. \
             On teardown the bench will stop the restricted instance so your normal \
             daemon session can reload with all drives on next use."
        ),
    }
}

/// Build the ES-instance launch confirmation [`Card`].
///
/// Shown after the negotiated matrix is displayed, before the bench tool
/// actually spawns `Everything.exe`.  Gives the operator a chance to cancel
/// if they don't want the bench to touch the Everything process.
pub(crate) fn es_launch_card(
    capable_drives: &[char],
    admin: bool,
    step_num: u32,
    step_total: u32,
) -> Card {
    let drive_list: String = capable_drives
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let admin_note = if admin {
        " (as Administrator — `Everything.exe -admin`)"
    } else {
        ""
    };
    let title = format!("Launch isolated Everything.exe instance for drives: {drive_list}");
    let why = format!(
        "The bench will stop any running Everything instance, then start a private \
         instance{admin_note} restricted to the RAM-budget-capable drives ({drive_list}) \
         and shut it down when the run completes. Your permanent Everything.ini is not modified."
    );
    let cmd = if admin {
        "Everything.exe -config <temp.ini> -instance uffs-bench -admin -startup".to_owned()
    } else {
        "Everything.exe -config <temp.ini> -instance uffs-bench -startup".to_owned()
    };
    Card {
        id: "es-instance-launch".to_owned(),
        stage: "STAGE 0: PREFLIGHT".to_owned(),
        step_num,
        step_total,
        title,
        why: why.clone(),
        commands: vec![cmd],
        resources: capable_drives
            .iter()
            .map(|ch| format!("{ch}: (Everything index)"))
            .collect(),
        backups: Vec::new(),
        est_time: "~1-5 min (indexing)".to_owned(),
        recovery: "Aborting here skips ES cells entirely — UFFS-only run.".to_owned(),
        long_why: format!(
            "{why}\n\nAny running Everything instance (default or stale bench) is stopped \
             first so the bench starts from a clean slate.  The instance is named `uffs-bench` \
             to distinguish it from a regular session.\n\
             Pass `--es-admin` on the command line to spawn it elevated."
        ),
    }
}

/// The [`StepResult`] used when a step is dry-run (rendered, not executed).
pub(crate) fn dry_run_result() -> StepResult {
    StepResult {
        code: None,
        summary: "Dry-run: rendered only, nothing mutated.".to_owned(),
        output_path: None,
    }
}
