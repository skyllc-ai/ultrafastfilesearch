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

/// Coverage-scope label for the report draft name (participating drives, or
/// `"full"` when none were narrowed).
pub(crate) fn report_scope(cli: &Cli) -> String {
    let scope: String = cli
        .drives_or_default()
        .iter()
        .map(char::to_ascii_lowercase)
        .collect();
    if scope.is_empty() {
        "full".to_owned()
    } else {
        scope
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
