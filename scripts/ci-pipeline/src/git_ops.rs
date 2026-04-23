// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
//! `git` + `gh` CLI helpers that Phase 2 of the ship pipeline drives.
//!
//! The entry points are [`git_commit`] (create the `chore: development
//! vX.Y.Z ...` signed commit) and [`git_push`] (push the release
//! branch + open the release PR with auto-merge queued).  Each of the
//! push sub-steps (`detect_current_branch`, `rebase_onto_upstream`,
//! `push_release_branch`, `find_existing_release_pr`, `open_release_pr`,
//! `enable_auto_merge`) is its own small helper so a failure points
//! at a named function in the backtrace and refactors stay surgical.
//!
//! [`count_unpushed_commits`] is the Phase 6 (resumable-push-fix)
//! helper: it lets the ship pipeline detect "HEAD is ahead of
//! `origin/release/<ver>`" and re-run the cached-completed push step
//! instead of silently skipping it.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use tokio::process::Command;

use crate::context::PipelineContext;
use crate::exec::execute_command;
use crate::version::extract_version_from_cargo_toml;

/// Stage the release-branch working tree and create the auto-generated
/// `chore: development vX.Y.Z ... [auto-commit]` commit.  Commit
/// message shape is stable so the release PR template can parse it.
///
/// # Errors
///
/// Propagates any failure from the wrapped `git add`, the Cargo.toml
/// read, or the `git commit` subprocess.
pub(crate) async fn git_commit(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📝 Creating auto-generated commit...".blue());
    execute_command("Git add", "git", &["add", "."], ctx).await?;

    let cargo_toml = std::fs::read_to_string("Cargo.toml").context("Failed to read Cargo.toml")?;
    let version = extract_version_from_cargo_toml(&cargo_toml)?;
    let commit_message =
        format!("chore: development v{version} - comprehensive testing complete [auto-commit]");
    execute_command("Git commit", "git", &["commit", "-m", &commit_message], ctx).await?;
    Ok(())
}

/// Count commits on the local HEAD that have not yet landed on
/// `origin/<remote_branch>`.
///
/// Phase 6 helper (dev-flow-implementation-plan.md § 6.3) used by the
/// ship pipeline to decide whether a previously-completed
/// `STEP_GIT_PUSH` needs to be re-run.  After a push succeeds the step
/// is cached; if the developer then commits more locally (e.g. to fix
/// a CI-detected audit failure) and re-runs `just ship`, the cached
/// "completed" state would silently skip the push and the new commits
/// would never land.  Counting `origin/<branch>..HEAD` reliably
/// detects that case.
///
/// Special cases:
/// * If the remote ref does not yet exist (first push of a new release branch),
///   `git rev-list` fails — we treat that as "1 unpushed commit" so the push
///   runs.
/// * If HEAD equals the remote ref, the count is 0 and the cached completion is
///   honoured (idempotent re-runs stay cheap).
///
/// # Errors
///
/// Returns an error only when the `git rev-list` subprocess itself
/// cannot be spawned.  A non-zero exit status is treated as the "ref
/// missing" special case and folded into `Ok(1)`.
pub(crate) async fn count_unpushed_commits(remote_branch: &str) -> Result<u64> {
    let remote_ref = format!("origin/{remote_branch}");
    let spec = format!("{remote_ref}..HEAD");
    let out = Command::new("git")
        .args(["rev-list", "--count", &spec])
        .output()
        .await
        .with_context(|| format!("Failed to run git rev-list for {spec}"))?;
    if !out.status.success() {
        // Remote ref doesn't exist yet (first push) — be conservative
        // and treat HEAD as ahead so the push always runs.
        return Ok(1);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.trim().parse::<u64>().unwrap_or(1))
}

// ─────────────────────────────────────────────────────────────────────────────
// `git_push` sub-steps
// ─────────────────────────────────────────────────────────────────────────────

/// Read the current branch name via `git rev-parse --abbrev-ref HEAD`.
///
/// # Errors
///
/// Returns an error if `git rev-parse` fails or the repo is in
/// detached-HEAD state (returns the literal `"HEAD"` in that case,
/// which is not a valid base branch for the release PR).
fn detect_current_branch() -> Result<String> {
    let branch_output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get current branch")?;
    let current_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if current_branch.is_empty() || current_branch == "HEAD" {
        bail!("Could not determine current branch (detached HEAD?)");
    }
    Ok(current_branch)
}

/// Rebase `current_branch` onto `origin/<current_branch>` so the auto-
/// commit lands on top of any intervening mainline changes.
///
/// # Errors
///
/// Propagates any failure from the wrapped `git pull --rebase`
/// subprocess (network, merge conflicts, ...).
async fn rebase_onto_upstream(ctx: &PipelineContext, current_branch: &str) -> Result<()> {
    execute_command(
        "Git pull rebase",
        "git",
        &["pull", "origin", current_branch, "--rebase"],
        ctx,
    )
    .await
}

/// Push `HEAD` to `refs/heads/<release_branch>` on `origin`.  No-op /
/// fast-forward when the pipeline is resuming after a previously-
/// failed step 11 that already pushed the same commit.
///
/// # Errors
///
/// Propagates any failure from the wrapped `git push` subprocess.
async fn push_release_branch(ctx: &PipelineContext, release_branch: &str) -> Result<()> {
    let push_ref = format!("HEAD:refs/heads/{release_branch}");
    println!("📤 Pushing HEAD to {}", release_branch.cyan());
    execute_command(
        "Git push (release branch)",
        "git",
        &["push", "origin", &push_ref],
        ctx,
    )
    .await
}

/// Return the PR number of an already-open PR for `release_branch`, or
/// `None` if no open PR exists.  Used by [`git_push`] to keep the
/// PR-creation step idempotent across resumed ship runs.
///
/// # Errors
///
/// Propagates any failure from the wrapped `gh pr list` subprocess.
fn find_existing_release_pr(release_branch: &str) -> Result<Option<String>> {
    let existing_pr_output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            release_branch,
            "--state",
            "open",
            "--json",
            "number",
            "-q",
            ".[0].number",
        ])
        .output()
        .context("Failed to query existing release PR via gh")?;
    let trimmed = String::from_utf8_lossy(&existing_pr_output.stdout)
        .trim()
        .to_string();
    Ok((!trimmed.is_empty()).then_some(trimmed))
}

/// Open a new release PR via `gh pr create`, with the branch-
/// protection-compatible title + body that explains the auto-merge
/// squash strategy.
///
/// # Errors
///
/// Propagates any failure from the wrapped `gh pr create` subprocess.
async fn open_release_pr(
    ctx: &PipelineContext,
    base_branch: &str,
    release_branch: &str,
    version: &str,
) -> Result<()> {
    let pr_title = format!("chore: release v{version} — ship pipeline auto-commit");
    let pr_body = format!(
        "## Summary\n\n\
         `just ship` Phase 2 auto-commit for **v{version}**.  Binaries + \
         GitHub Release v{version} are already live (step 09).  This PR \
         routes the corresponding commit through branch-protection rules.\n\n\
         ## Auto-merge\n\n\
         `--auto --squash` is queued — GitHub will merge as soon as the \
         required status checks pass.  Squash is required because \
         `main-protection` mandates signed commits, and GitHub's \
         rebase-auto-merge cannot sign the rebased commit; the \
         squash-merge commit is signed by GitHub's own key, which \
         satisfies `required_signatures: true`.  The original author's \
         signed commit remains verifiable in the PR branch history.\n\n\
         ## After merge\n\n\
         Local `{base_branch}` had this commit with a different SHA \
         before squash rewrote it onto main; recover with \
         `git fetch origin && git reset --hard origin/{base_branch}`."
    );

    println!("📬 Opening release PR");
    execute_command(
        "Open release PR",
        "gh",
        &[
            "pr",
            "create",
            "--base",
            base_branch,
            "--head",
            release_branch,
            "--title",
            &pr_title,
            "--body",
            &pr_body,
        ],
        ctx,
    )
    .await
}

/// Enable GitHub auto-merge with the squash strategy for the release
/// PR.
///
/// Squash is mandatory on this repo because:
///
///   1. `main-protection` requires `required_signatures: true` (every commit on
///      main must be signed).
///   2. GitHub's rebase-auto-merge cannot sign the rebased commit; it fails
///      with `GraphQL: Base branch requires signed commits. Rebase merges
///      cannot be automatically signed by GitHub` (observed on PR #36, the
///      first real `just ship` for v0.5.69).
///   3. GitHub signs the squash-merge commit with its own key, which satisfies
///      `required_signatures: true` on main.
///
/// Trust trade-off: the author's GPG signature is lost on the commit
/// that lands on main (it becomes a GitHub-signed squash).  The
/// original signed commit remains verifiable in the PR branch history,
/// and every prior merged PR on this repo uses the same pattern.
///
/// # Errors
///
/// Propagates any failure from the wrapped `gh pr merge` subprocess.
async fn enable_auto_merge(ctx: &PipelineContext, release_branch: &str) -> Result<()> {
    println!("⚡ Ensuring auto-merge is enabled (squash strategy)");
    execute_command(
        "Enable auto-merge",
        "gh",
        &["pr", "merge", release_branch, "--auto", "--squash"],
        ctx,
    )
    .await
}

/// Push the release branch and open the release PR against the current
/// base branch.  Branch-protection compatible: never pushes directly
/// to `main`.
///
/// Thin orchestrator — each sub-step lives in its own helper
/// ([`detect_current_branch`], [`rebase_onto_upstream`],
/// [`push_release_branch`], [`find_existing_release_pr`],
/// [`open_release_pr`], [`enable_auto_merge`]) so the control flow
/// stays readable and individual failures map 1:1 to a named helper
/// in the backtrace.
///
/// # Errors
///
/// Propagates any failure from the helper subprocesses.  The version
/// lookup via [`crate::version::get_current_version`] is the one
/// non-subprocess error source.
pub(crate) async fn git_push(ctx: &PipelineContext) -> Result<()> {
    println!(
        "{}",
        "🚀 Opening release PR (branch-protection-compatible)...".blue()
    );

    let current_branch = detect_current_branch()?;
    println!("📌 Current branch: {}", current_branch.cyan());

    // Stay current with upstream before opening the release PR.
    rebase_onto_upstream(ctx, &current_branch).await?;

    // Derive the release branch name from the workspace version that
    // Phase 2 step 07 bumped (example: `release/v0.5.68`).
    let version = crate::version::get_current_version()?;
    let release_branch = format!("release/v{version}");

    push_release_branch(ctx, &release_branch).await?;

    // Idempotent PR creation: reuse an existing open PR for the same
    // release branch if the pipeline is resuming from a previously-
    // failed step 11.
    match find_existing_release_pr(&release_branch)? {
        Some(pr_number) => {
            println!("ℹ️  Reusing existing release PR #{}", pr_number.cyan());
        }
        None => {
            open_release_pr(ctx, &current_branch, &release_branch, &version).await?;
        }
    }

    enable_auto_merge(ctx, &release_branch).await?;

    println!(
        "{} Release PR for v{} opened with auto-merge queued",
        "✅".green(),
        version
    );
    println!(
        "   💡 Watch checks: {}",
        format!("gh pr checks {release_branch} --watch").cyan()
    );
    println!(
        "   💡 After merge:  {}",
        format!(
            "git fetch origin && git reset --hard origin/{current_branch} (squash rewrites commit SHA)"
        )
        .cyan()
    );

    Ok(())
}
