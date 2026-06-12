// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage identity + selection helpers for the staged run loop.
//!
//! Pure functions over stage numbers and CLI stage filters: which stages the
//! `--only-stage` / `--from-stage` / `--skip-stages` flags select, the stable
//! resume-engine step ids, the operator-facing banners, and the fixed Stage 0
//! artifact list.  Extracted from `run/mod.rs` so the orchestrator file stays
//! within the file-size policy; `cards.rs` consumes the ids/banners via the
//! `crate::run` re-exports.

use std::path::Path;

use crate::cli::Cli;

/// Whether `stage` is selected by the `--only-stage` / `--from-stage` /
/// `--skip-stages` filters.
pub(super) fn stage_selected(cli: &Cli, stage: u32) -> bool {
    if cli.skip_stages.contains(&stage) {
        return false;
    }
    match (cli.only_stage, cli.from_stage) {
        (Some(only), _) => stage == only,
        (None, Some(from)) => stage >= from,
        (None, None) => true,
    }
}

/// Resume-engine step id for a measurement stage.
pub(crate) fn stage_step_id(stage: u32) -> String {
    format!("stage{stage}/measure")
}

/// Operator-facing banner for a measurement stage.
pub(crate) fn stage_banner(stage: u32) -> String {
    let label = match stage {
        1 => "CROSS-TOOL",
        2 => "PARITY",
        _ => "FULL SUITE",
    };
    format!("STAGE {stage}: {label}")
}

/// Artifact paths Stage 0 writes into the bundle (for the state record).
pub(super) fn stage0_outputs(bundle_dir: &Path) -> Vec<String> {
    [
        "env.json",
        "env.md",
        "competitor-preflight.json",
        "matrix.json",
    ]
    .iter()
    .map(|name| bundle_dir.join(name).display().to_string())
    .collect()
}
