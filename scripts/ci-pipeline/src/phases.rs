// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
//! Non-resumable Phase 1 / Phase 2 driver functions.
//!
//! These variants skip the [`crate::workflow::WorkflowState`] wrapper
//! and run straight through from start to finish.  They back the
//! standalone `just go`, `just check-all`, and `just phase2-ship`
//! recipes — i.e. the lanes where "resume mid-flight" semantics are
//! not useful because the lane has no state to preserve.  The
//! resumable equivalents live in [`crate::ship`].
//!
//! * [`phase1_optimized`] — thin orchestrator: [`phase1_prime`] →
//!   [`phase1_tests`] → [`phase1_fanout_validation`].
//! * [`phase2_optimized`] — version bump + commit + push in a straight line
//!   (used by `just phase2-ship`, separate from the resumable
//!   `run_enhanced_phase2` that [`crate::ship`] drives).
//! * [`coverage_data_exists`] / [`coverage_report_command`] — the
//!   `coverage-report` subcommand primitives; referenced from both
//!   `phase1_tests` and the CLI dispatch.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::context::{PipelineContext, get_cargo_target_dir};
use crate::exec::{execute_command, execute_parallel_with_env};
use crate::git_ops::{git_commit, git_push};
use crate::version::{get_current_version, version_bump};
use crate::workflow::WorkflowState;

/// Return `true` if a previous `cargo llvm-cov` run left behind
/// coverage data under `target/llvm-cov-target/` — the signal
/// `coverage-report` uses to decide whether to regenerate HTML from
/// cached data or re-run the whole test suite with instrumentation.
pub(crate) fn coverage_data_exists() -> bool {
    let target_dir = get_cargo_target_dir();
    target_dir.join("llvm-cov").exists()
}

/// Entry point for the `coverage-report` subcommand: regenerates the
/// HTML coverage report from existing instrumentation data, or runs
/// the full coverage test pass first if the data is missing.
///
/// # Errors
///
/// Propagates any failure from the wrapped `cargo llvm-cov` / `cargo
/// llvm-cov nextest` subprocesses.
pub(crate) async fn coverage_report_command(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📊 Coverage Report Generation".blue().bold());
    if coverage_data_exists() {
        println!(
            "{} Found existing coverage data, generating report...",
            "🔍".green()
        );
        execute_command(
            "Coverage report",
            "cargo",
            &["llvm-cov", "report", "--html"],
            ctx,
        )
        .await?;
    } else {
        println!(
            "{} No coverage data found, running tests first...",
            "⚠️".yellow()
        );
        execute_command(
            "Coverage tests",
            "cargo",
            &[
                "llvm-cov",
                "nextest",
                "--workspace",
                "--all-features",
                "--lib",
                "--bins",
                "--tests",
                "--profile",
                "ci",
                "--html",
            ],
            ctx,
        )
        .await?;
    }
    println!(
        "{} Coverage report: target/llvm-cov/html/index.html",
        "📁".green()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 1 (non-resumable)
// ─────────────────────────────────────────────────────────────────────────────

/// Prime the workspace before the heavy validation stages: ensure the
/// pinned nightly is installed and run the file-size policy (which
/// catches structural repo violations before any compilation cost).
async fn phase1_prime(ctx: &PipelineContext) -> Result<()> {
    execute_command("Toolchain ensure", "just", &["toolchain-ensure"], ctx).await?;
    execute_command(
        "File size policy",
        "bash",
        &["scripts/ci/check_file_size_policy.sh"],
        ctx,
    )
    .await
}

/// Run the workspace nextest test pass, then optionally regenerate the
/// HTML coverage report when `--coverage-report` is in effect.
async fn phase1_tests(ctx: &PipelineContext) -> Result<()> {
    execute_command(
        "Workspace tests",
        "cargo",
        &[
            "nextest",
            "run",
            "--workspace",
            "--all-features",
            "--lib",
            "--tests",
            "--profile",
            "ci",
        ],
        ctx,
    )
    .await?;

    if ctx.flags.coverage_report {
        coverage_report_command(ctx).await
    } else {
        if ctx.flags.verbose {
            println!(
                "{} Coverage report skipped (use --coverage-report to generate)",
                "⏭️".yellow()
            );
        }
        Ok(())
    }
}

/// Fan out the Phase 1 validation stack (doctests + production clippy +
/// test clippy + `cargo deny` + rustdoc link validation) with
/// `RUSTDOCFLAGS=-Dwarnings` so rustdoc failures are hard errors.
///
/// Kept as data + one call so that any future lint-profile churn lands
/// in exactly one place.
async fn phase1_fanout_validation(ctx: &PipelineContext) -> Result<()> {
    let parallel_commands = vec![
        ("Documentation tests", "cargo", vec![
            "test",
            "--doc",
            "--workspace",
            "--all-features",
        ]),
        // pedantic/nursery/cargo/multiple_crate_versions levels are set
        // in workspace Cargo.toml — only per-target overrides needed
        // here.
        ("Production linting", "cargo", vec![
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--no-deps",
            "--",
            "-D",
            "warnings",
            "-W",
            "clippy::panic",
            "-W",
            "clippy::todo",
            "-W",
            "clippy::unimplemented",
            "-W",
            "clippy::unwrap_used",
            "-W",
            "clippy::expect_used",
        ]),
        ("Test linting", "cargo", vec![
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--tests",
            "--no-deps",
            "--",
            "-D",
            "warnings",
            "-W",
            "clippy::panic",
            "-W",
            "clippy::todo",
            "-W",
            "clippy::unimplemented",
            "-A",
            "clippy::unwrap_used",
            "-A",
            "clippy::expect_used",
        ]),
        ("Dependency security", "cargo", vec!["deny", "check"]),
        ("Rustdoc link validation", "cargo", vec![
            "doc",
            "--workspace",
            "--all-features",
            "--no-deps",
        ]),
    ];
    execute_parallel_with_env(parallel_commands, &[("RUSTDOCFLAGS", "-Dwarnings")], ctx).await
}

/// Phase 1: Safe-by-default validation with maximum parallelism.
///
/// Thin orchestrator: [`phase1_prime`] (toolchain + file-size policy)
/// → [`phase1_tests`] (workspace nextest + optional coverage) →
/// [`phase1_fanout_validation`] (doctests + clippy trio + deny +
/// rustdoc).
///
/// # Errors
///
/// Propagates any failure from the three sub-stages.
pub(crate) async fn phase1_optimized(ctx: &PipelineContext) -> Result<()> {
    println!(
        "{}",
        "🧪 PHASE 1: Safe-by-Default Validation Pipeline"
            .blue()
            .bold()
    );
    println!("ℹ️  No version bump, deploy, commit, or push in this lane");

    phase1_prime(ctx).await?;
    phase1_tests(ctx).await?;
    phase1_fanout_validation(ctx).await?;

    println!(
        "{}",
        "✅ PHASE 1 COMPLETE: Validation passed without release-side effects!"
            .green()
            .bold()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2 (explicit ship lane — non-resumable counterpart)
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 2: Explicit ship lane (version bump → commit → push).  Used
/// by the standalone `just phase2-ship` recipe.  The resumable
/// equivalent lives in [`crate::ship::run_enhanced_phase2`] and is
/// the one `run_ship_pipeline` calls.
///
/// # Errors
///
/// Propagates any failure from [`version_bump`], [`git_commit`], or
/// [`git_push`].  The workflow-state mutation will also fail if the
/// state file cannot be written.
pub(crate) async fn phase2_optimized(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🚀 PHASE 2: Explicit Ship Lane".blue().bold());

    // Step 1: Version increment
    version_bump(ctx).await?;

    // Update workflow state with new version
    let mut state = WorkflowState::load().context("Failed to load workflow state")?;
    let new_version = get_current_version().context("Failed to get updated version")?;
    state.current_version = new_version;
    state.version_incremented = true;
    state.save().context("Failed to save workflow state")?;
    println!(
        "✅ Workflow state updated with new version: {}",
        state.current_version
    );

    // Step 2: Git commit (signed version-bump commit on the working
    // branch).
    git_commit(ctx).await?;

    // Step 3: Git push -- opens release/vX.Y.Z PR with auto-merge
    // queued.
    //
    // Binaries are NOT built here.  Once the PR merges to main,
    // `auto-tag-release.yml` tags the commit and invokes
    // `release.yml`, which produces the reproducible cross-platform
    // binaries on GitHub-hosted runners.
    git_push(ctx).await?;

    println!(
        "{}",
        "✅ PHASE 2 COMPLETE: Versioned, committed, and release PR opened!"
            .green()
            .bold()
    );
    Ok(())
}
