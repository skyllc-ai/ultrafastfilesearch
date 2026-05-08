// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
//! Resumable ship pipeline driver.
//!
//! Everything in this module wraps [`crate::exec::execute_step_with_tracking`]
//! around a pipeline step so the on-disk
//! [`crate::workflow::WorkflowState`] records each transition — a
//! re-run after a failure picks up at the first non-completed step.
//!
//! Layout:
//! * `tracked_*_step` — one helper per resumable pipeline step (toolchain,
//!   clean, coverage, parallel validation, format check).
//! * `run_enhanced_phase1` / `run_enhanced_phase2` — thin orchestrators over
//!   those tracked helpers.
//! * `load_or_reset_ship_state` / `print_ship_*` — resume / summary /
//!   next-steps helpers.
//! * `run_ship_pipeline` — the entry point `just ship` ultimately calls.

use std::time::Duration;

use anyhow::{Context, Result};
use colored::Colorize;
use tokio::process::Command;

use crate::context::{
    PipelineContext, bytes_to_gib, dir_size_bytes, disk_free_bytes, get_cargo_target_dir,
};
use crate::exec::{execute_command, execute_parallel_with_env, execute_step_with_tracking};
use crate::git_ops::{count_unpushed_commits, git_commit, git_push};
use crate::version::{get_current_version, update_polars_git};
use crate::workflow::{
    ALL_STEPS, STEP_CLEAN_ARTIFACTS, STEP_COVERAGE_TESTS, STEP_FORMAT_CHECK, STEP_FORMAT_CODE,
    STEP_GIT_COMMIT, STEP_GIT_PUSH, STEP_PARALLEL_VALIDATION, STEP_TOOLCHAIN_SYNC,
    STEP_UPDATE_POLARS, WorkflowPhase, WorkflowState,
};

// ─────────────────────────────────────────────────────────────────────────────
// Tracked step helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Tracked toolchain step: `--fresh` runs bump to today's nightly via
/// `just toolchain-sync` (unless `--skip-toolchain-sync`); resumable
/// runs only ensure the currently pinned nightly is installed so
/// repeat ship invocations don't churn `rust-toolchain.toml`.
async fn tracked_toolchain_step(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    let (step_label, step_recipe) = if ctx.flags.fresh && !ctx.flags.skip_toolchain_sync {
        (
            "Toolchain sync (fresh bump to latest nightly)",
            "toolchain-sync",
        )
    } else {
        ("Toolchain ensure (pinned nightly)", "toolchain-ensure")
    };
    execute_step_with_tracking(state, STEP_TOOLCHAIN_SYNC, || async {
        execute_command(step_label, "just", &[step_recipe], ctx).await
    })
    .await
}

/// Tracked clean step: applies the disk-pressure auto-clean policy,
/// running `cargo clean` only when explicitly requested (`--clean`) or
/// when one of the thresholds (`--min-free-gb`, `--max-target-gb`) is
/// tripped.  Inert when `--no-clean` is set.
async fn tracked_clean_step(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    execute_step_with_tracking(state, STEP_CLEAN_ARTIFACTS, || async {
        let target_dir = get_cargo_target_dir();
        let free_gb = disk_free_bytes(&target_dir).await.map(bytes_to_gib);
        let target_gb = dir_size_bytes(&target_dir, Duration::from_secs(30))
            .await
            .map(bytes_to_gib);

        if ctx.flags.verbose {
            if let Some(free) = free_gb {
                println!("  💾 Free disk space: {free} GiB");
            }
            if let Some(size) = target_gb {
                println!("  📁 Target directory size: {size} GiB");
            }
        }

        let should_clean = ctx.flags.force_clean
            || (!ctx.flags.force_no_clean
                && (free_gb.is_some_and(|g| g < ctx.min_free_gb)
                    || target_gb.is_some_and(|g| g > ctx.max_target_gb)));

        if should_clean {
            if ctx.flags.force_clean {
                println!("  🧹 Forced clean (--clean flag)");
            } else {
                println!("  🧹 Auto-clean triggered (disk space low or target too large)");
            }
            execute_command("Clean build artifacts", "cargo", &["clean"], ctx).await
        } else {
            println!("  ⏭️  Skipping clean (disk space OK, target size OK)");
            Ok(())
        }
    })
    .await
}

/// Tracked coverage-tests step: `cargo llvm-cov nextest` across the
/// workspace.  Uses `--lib --bins --tests` rather than `--all-targets`
/// to exclude benchmarks whose init-time `DataFrame` allocations would
/// SIGKILL `nextest` during test enumeration.
async fn tracked_coverage_tests_step(
    state: &mut WorkflowState,
    ctx: &PipelineContext,
) -> Result<()> {
    execute_step_with_tracking(state, STEP_COVERAGE_TESTS, || async {
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
                "--no-report",
            ],
            ctx,
        )
        .await
    })
    .await
}

/// Tracked parallel-validation step: doctests + production clippy +
/// test clippy + `cargo deny` + rustdoc link validation, all fanned
/// out with `RUSTDOCFLAGS=-Dwarnings` so rustdoc failures are hard.
async fn tracked_parallel_validation_step(
    state: &mut WorkflowState,
    ctx: &PipelineContext,
) -> Result<()> {
    execute_step_with_tracking(state, STEP_PARALLEL_VALIDATION, || async {
        let parallel_commands = vec![
            ("Documentation tests", "cargo", vec![
                "test",
                "--doc",
                "--workspace",
                "--all-features",
            ]),
            // pedantic/nursery/cargo/multiple_crate_versions levels are
            // set in workspace Cargo.toml — only per-target overrides
            // needed here.
            ("Production linting", "cargo", vec![
                "clippy",
                "--workspace",
                "--lib",
                "--bins",
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
                "-A",
                "unused-crate-dependencies",
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
    })
    .await
}

/// Tracked format-check step: `cargo fmt --all -- --check` on the
/// *post-format* tree — catches any files rustfmt can't stabilise in
/// one pass (extremely rare, but worth gating before shipping).
async fn tracked_format_check_step(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    execute_step_with_tracking(state, STEP_FORMAT_CHECK, || async {
        execute_command(
            "Format check",
            "cargo",
            &["fmt", "--all", "--", "--check"],
            ctx,
        )
        .await
    })
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// Resumable Phase 1 / Phase 2
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 1 of the ship pipeline: the full validation sweep (toolchain +
/// polars + clean + format + coverage tests + parallel validation +
/// format check).  Mutates `state` so every step's started / completed
/// / failed transition is persisted — a re-run after a failure picks
/// up at the first non-completed step.
///
/// Thin orchestrator: each `STEP_*` gets a named `tracked_*_step`
/// helper, so the control flow is readable and a backtrace on failure
/// points at the specific tracked step that failed.
///
/// # Errors
///
/// Propagates any failure from the wrapped tracked steps.
pub(crate) async fn run_enhanced_phase1(
    state: &mut WorkflowState,
    ctx: &PipelineContext,
) -> Result<()> {
    println!(
        "{}",
        "🧪 PHASE 1: Optimized Testing & Validation Pipeline"
            .blue()
            .bold()
    );
    println!(
        "ℹ️  Running all validation with CURRENT version - version increment happens AFTER validation passes"
    );
    println!(
        "🚀 Using safe parallel optimization: format → compile → parallel(doc tests + linting)"
    );

    tracked_toolchain_step(state, ctx).await?;
    execute_step_with_tracking(state, STEP_UPDATE_POLARS, || async {
        update_polars_git(ctx).await
    })
    .await?;

    println!("{}", "📋 Stage 1: Sequential Prerequisites".yellow().bold());

    tracked_clean_step(state, ctx).await?;
    execute_step_with_tracking(state, STEP_FORMAT_CODE, || async {
        execute_command("Format code", "cargo", &["fmt", "--all"], ctx).await
    })
    .await?;

    // File size policy runs inline (not resumable) because it's
    // ~100 ms and a stale "completed" state could mask a new
    // violation.
    execute_command(
        "File size policy",
        "bash",
        &["scripts/ci/check_file_size_policy.sh"],
        ctx,
    )
    .await?;

    tracked_coverage_tests_step(state, ctx).await?;
    tracked_parallel_validation_step(state, ctx).await?;
    tracked_format_check_step(state, ctx).await?;

    println!(
        "{}",
        "✅ PHASE 1 COMPLETE - All testing and validation passed!"
            .green()
            .bold()
    );
    Ok(())
}

/// Phase 2 of the ship pipeline: version bump + commit + push + open
/// release PR.  Same resumable-state semantics as
/// [`run_enhanced_phase1`].  The push step participates in the Phase 6
/// `invalidate_step` fix so a follow-up commit after a cached
/// "push completed" state re-runs the push rather than silently
/// skipping.
///
/// # Errors
///
/// Propagates any failure from the wrapped tracked steps or from the
/// workflow-state writes surrounding them.
pub(crate) async fn run_enhanced_phase2(
    state: &mut WorkflowState,
    ctx: &PipelineContext,
) -> Result<()> {
    println!("{}", "📦 PHASE 2: Commit + Push".blue().bold());

    // R5 (2026-05-08): Phase 2 no longer bumps the workspace version.
    // release-plz drives version bumps on `main` via the release-PR
    // flow (see `release-automation-plan.md` §R5).  The local ship
    // pipeline just commits whatever the dev staged and pushes the
    // working branch.  WorkflowState's `current_version` is read
    // from the unchanged `Cargo.toml` so the resume banner still
    // reports the right number.
    let current_version =
        get_current_version().context("Failed to read workspace version from Cargo.toml")?;
    if state.current_version != current_version {
        state.current_version = current_version;
        state.save()?;
    }

    // Step 10: Git commit (signed commit on the working branch).
    execute_step_with_tracking(state, STEP_GIT_COMMIT, || async { git_commit(ctx).await }).await?;

    // Step 11: Git push -- opens / updates the working-branch PR.
    //
    // Binaries are NOT built here.  When the PR merges to `main` and
    // a release-PR (opened by release-plz) is subsequently merged,
    // release-plz creates the `vX.Y.Z` tag and dispatches
    // `release.yml` to produce the reproducible cross-platform
    // binaries on GitHub-hosted runners (R5 bridge in
    // `.github/workflows/release-plz.yml`).
    //
    // Phase 6 resumable-push fix (docs/architecture/dev-flow.md §
    // 5.1 / dev-flow-implementation-plan.md § 6.3): if the developer
    // committed locally since the previous ship run (e.g. to fix a
    // CI-detected audit failure), HEAD will be ahead of the already-
    // pushed release branch on origin.  The cached "completed" state
    // from the prior run would otherwise silently skip this step and
    // the new commits would never land.  Detect that condition and
    // invalidate the cached state so the step re-runs.
    let release_branch_peek = format!("release/v{}", get_current_version()?);
    let unpushed = count_unpushed_commits(&release_branch_peek).await?;
    if unpushed > 0 && state.is_step_completed(STEP_GIT_PUSH) {
        println!(
            "↻ {} unpushed commit(s) on HEAD vs origin/{} — re-running step {}",
            unpushed.to_string().yellow(),
            release_branch_peek.cyan(),
            STEP_GIT_PUSH.cyan(),
        );
        state.invalidate_step(STEP_GIT_PUSH)?;
    }
    execute_step_with_tracking(state, STEP_GIT_PUSH, || async { git_push(ctx).await }).await?;

    println!(
        "{}",
        "✅ PHASE 2 COMPLETE - Release PR opened; GitHub Actions will produce binaries."
            .green()
            .bold()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Ship-entry helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Load the on-disk workflow state for a ship run, resetting it to a
/// fresh default when `--fresh` is in effect.  Non-fresh runs that
/// fail to parse the state file fall through to a fresh default so a
/// corrupt state can't strand the pipeline.
///
/// # Errors
///
/// Returns an error if `--fresh` saves cannot persist the reset state
/// to disk.
fn load_or_reset_ship_state(ctx: &PipelineContext) -> Result<WorkflowState> {
    if ctx.flags.fresh {
        println!(
            "{} Fresh run requested - resetting workflow state",
            "🔄".yellow()
        );
        let current_version = get_current_version().unwrap_or_else(|_| "unknown".to_string());
        let new_state = WorkflowState::new_workflow(current_version);
        new_state
            .save()
            .context("Failed to save fresh workflow state")?;
        Ok(new_state)
    } else {
        Ok(WorkflowState::load().unwrap_or_else(|_| {
            let current_version = get_current_version().unwrap_or_else(|_| "unknown".to_string());
            WorkflowState::new_workflow(current_version)
        }))
    }
}

/// Print the resume banner: how many steps are already completed,
/// which ones, and which are still pending.  No-op on `--fresh` runs
/// and on the very first ship attempt (nothing to resume).
fn print_ship_resume_banner(state: &WorkflowState, ctx: &PipelineContext) {
    let completed_count = state.step_tracker.completed_steps.len();
    if completed_count == 0 || ctx.flags.fresh {
        return;
    }
    let total_steps = ALL_STEPS.len();
    let pending_steps = state.get_pending_steps(ALL_STEPS);

    println!(
        "{} Resuming from previous run: {}/{} steps completed",
        "📋".cyan(),
        completed_count,
        total_steps
    );
    if !state.step_tracker.completed_steps.is_empty() {
        println!("   Already completed:");
        for step in &state.step_tracker.completed_steps {
            println!("     {} {}", "✓".green(), step);
        }
    }
    if !pending_steps.is_empty() {
        println!("   Remaining:");
        for step in &pending_steps {
            println!("     {} {}", "○".dimmed(), step);
        }
    }
    println!();
}

/// Print the post-run summary: version, total wall time, completed
/// step count, and per-step timing breakdown.
fn print_ship_summary(state: &WorkflowState, total_time: Duration) {
    let total_steps = ALL_STEPS.len();
    println!();
    println!(
        "{}",
        "═══════════════════════════════════════════════════════════════════".green()
    );
    println!("{} Ship Pipeline Complete!", "🎉".green().bold());
    println!("═══════════════════════════════════════════════════════════════════");
    println!("   Version:    {}", state.current_version.cyan());
    println!("   Total time: {}s", total_time.as_secs());
    println!(
        "   Steps:      {}/{} completed",
        state.step_tracker.completed_steps.len(),
        total_steps
    );

    if !state.step_durations_secs.is_empty() {
        println!();
        println!("   Step Timings:");
        for (step, secs) in &state.step_durations_secs {
            let mins = secs / 60;
            let remaining_secs = secs % 60;
            if mins > 0 {
                println!("     {step} {mins}m {remaining_secs}s");
            } else {
                println!("     {step} {secs}s");
            }
        }
    }
}

/// Print sccache `-s` stats when both sccache was enabled and the user
/// asked for verbose output.  Returns silently in any other case.
pub(crate) async fn print_sccache_stats_if_verbose(ctx: &PipelineContext) {
    if ctx.flags.sccache_enabled
        && let Ok(out) = Command::new("sccache").arg("-s").output().await
        && ctx.flags.verbose
    {
        println!(
            "{} sccache stats:\n{}",
            "⚡".green(),
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// Print the terminal "what GitHub does next" hint block that points
/// the developer at the release-workflow run list.
fn print_ship_next_steps(state: &WorkflowState) {
    println!();
    println!("{} Release PR opened; auto-merge queued", "📤".green());
    println!(
        "{} GitHub Actions will build + publish v{} on merge",
        "👷".green(),
        state.current_version
    );
    println!(
        "   💡 Watch: {}",
        "gh run list --repo <owner>/<repo> --workflow=release.yml --limit 5".cyan()
    );
}

/// Print a horizontal separator used between the two phases of the
/// ship pipeline, for visual scannability in the terminal output.
fn print_ship_phase_separator() {
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".dimmed()
    );
}

/// Combined ship pipeline: Phase 1 (validation) + Phase 2 (deploy).
/// Supports resumable execution — re-runs skip already-completed
/// steps.  Pass `--fresh` on the CLI to reset state and start over.
///
/// Thin orchestrator: state hydration, progress banner, phase calls,
/// summary, sccache stats, and next-steps hint each live in their own
/// named helper so the ship entry point reads as a single recipe.
///
/// # Errors
///
/// Propagates any failure from the two phase functions or from the
/// workflow-state writes surrounding them.
pub(crate) async fn run_ship_pipeline(ctx: &PipelineContext) -> Result<()> {
    println!(
        "{}",
        "🚢 UFFS Ship Pipeline (Phase 1 + Phase 2, Resumable)"
            .blue()
            .bold()
    );
    println!("═══════════════════════════════════════════════════════════════════");

    let mut state = load_or_reset_ship_state(ctx)?;
    print_ship_resume_banner(&state, ctx);

    print_ship_phase_separator();
    run_enhanced_phase1(&mut state, ctx)
        .await
        .context("Phase 1 (validation) failed")?;

    print_ship_phase_separator();
    run_enhanced_phase2(&mut state, ctx)
        .await
        .context("Phase 2 (release PR) failed")?;

    state.advance_phase(WorkflowPhase::Completed)?;

    let total_time = ctx.start_time.elapsed();
    print_ship_summary(&state, total_time);
    print_sccache_stats_if_verbose(ctx).await;
    print_ship_next_steps(&state);

    Ok(())
}
