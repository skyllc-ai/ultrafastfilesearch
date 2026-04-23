// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
//! Version discovery + bump helpers for the UFFS ship pipeline.
//!
//! * [`get_current_version`] — read `[workspace.package].version` out of the
//!   root `Cargo.toml` (simple whole-file scan).
//! * [`extract_version_from_cargo_toml`] — same, but strict: only considers the
//!   `[workspace.package]` section.
//! * [`increment_version`] — shell out to the `./build/update_all_versions.rs`
//!   rust-script that actually rewrites the Cargo.toml in place.
//! * [`version_bump`] — tracked-step wrapper around [`increment_version`] that
//!   threads through the pipeline's logging / timeout conventions.
//! * [`update_polars_git`] — pin the polars git dep to the latest upstream
//!   `main` HEAD (or honour the `rev = "..."` override if present).

use std::path::Path;

use anyhow::{Context, Result, bail};
use colored::Colorize;
use tokio::process::Command;

use crate::context::PipelineContext;
use crate::exec::execute_command;

/// Read the workspace root `Cargo.toml` and return the first
/// `version = "..."` string found.  Used by the push step to build
/// `origin/release/vX.Y.Z`.
///
/// # Errors
///
/// Returns an error if `Cargo.toml` cannot be read, or if no `version`
/// line is present.
pub(crate) fn get_current_version() -> Result<String> {
    let cargo_toml = std::fs::read_to_string("Cargo.toml").context("Failed to read Cargo.toml")?;
    for line in cargo_toml.lines() {
        if line.trim().starts_with("version = ")
            && let Some(version) = line.split('"').nth(1)
        {
            return Ok(version.to_string());
        }
    }
    bail!("Could not find version in Cargo.toml")
}

/// Parse `content` (the text of a workspace root `Cargo.toml`) and
/// extract the `version = "..."` entry from the `[workspace.package]`
/// table specifically — ignores any unrelated `version = ...` lines in
/// `[dependencies]` or per-crate overrides.
///
/// # Errors
///
/// Returns an error if `[workspace.package]` is missing, or if it does
/// not contain a parseable `version` entry.
pub(crate) fn extract_version_from_cargo_toml(content: &str) -> Result<String> {
    let mut in_workspace_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if in_workspace_package {
            if trimmed.starts_with('[') && trimmed != "[workspace.package]" {
                break;
            }
            if trimmed.starts_with("version")
                && let Some(equals_pos) = trimmed.find('=')
            {
                let version_part = &trimmed[equals_pos + 1..].trim();
                let version = version_part.trim_matches('"').trim_matches('\'');
                return Ok(version.to_string());
            }
        }
    }
    bail!("Version extraction failed - no version found in [workspace.package]")
}

/// Parse the current `[workspace.package].version`, bump the patch
/// component, and rewrite `Cargo.toml` in place.  Separated from
/// [`version_bump`] so it can be called directly from the workflow
/// state machine without involving a subprocess.
///
/// # Errors
///
/// Returns an error if the `./build/update_all_versions.rs` helper
/// cannot be spawned, or if it exits with a non-zero status.
pub(crate) async fn increment_version() -> Result<()> {
    println!("📈 Incrementing version...");
    let output = Command::new("./build/update_all_versions.rs")
        .arg("patch")
        .output()
        .await
        .context("Failed to execute version update script")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Version bump failed: {stderr}");
    }
    println!("✅ Version incremented successfully");
    Ok(())
}

/// Bump the workspace `[package].version` in root `Cargo.toml`.
/// Runs the shared [`increment_version`] helper under the usual
/// logging and timeout wrapping.
///
/// # Errors
///
/// Propagates any failure from the wrapped [`execute_command`]
/// subprocess.  Fails fast if the helper script is missing.
pub(crate) async fn version_bump(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📈 Incrementing version...".blue());
    let script_path = Path::new("./build/update_all_versions.rs");
    if script_path.exists() {
        execute_command(
            "Version increment",
            "./build/update_all_versions.rs",
            &["patch"],
            ctx,
        )
        .await?;
    } else {
        println!("{}", "⚠️  Version script not found".yellow());
        bail!("Version bump failed - ./build/update_all_versions.rs not found");
    }
    Ok(())
}

/// Update Polars git dependencies to the latest commit on `main`.
///
/// **Skipped** when `uffs-polars/Cargo.toml` uses `rev = "..."` pinning
/// (which prevents upstream breakage).  In that case the pinned commit
/// is used as-is and `cargo update` is called with
/// `--precise <pinned-rev>`.
///
/// # Errors
///
/// Returns an error if `crates/uffs-polars/Cargo.toml` cannot be read,
/// if `git ls-remote` fails, or if `cargo update` returns non-zero.
pub(crate) async fn update_polars_git(_ctx: &PipelineContext) -> Result<()> {
    // Check if uffs-polars/Cargo.toml uses rev pinning
    let cargo_toml = std::fs::read_to_string("crates/uffs-polars/Cargo.toml")
        .context("Failed to read crates/uffs-polars/Cargo.toml")?;
    if let Some(rev_line) = cargo_toml
        .lines()
        .find(|l| l.contains("polars") && l.contains("rev ="))
    {
        // Extract the rev hash
        if let Some(start) = rev_line.find("rev = \"") {
            let hash_start = start + 7;
            if let Some(end) = rev_line[hash_start..].find('"') {
                let pinned_rev = &rev_line[hash_start..hash_start + end];
                println!(
                    "{}",
                    format!(
                        "📌 Polars pinned to rev={} — skipping auto-update",
                        &pinned_rev[..12]
                    )
                    .blue()
                );
                // Still run cargo update to ensure lockfile matches the pinned rev
                let status = Command::new("cargo")
                    .args(["update", "-p", "polars", "--precise", pinned_rev])
                    .status()
                    .await
                    .context("Failed to run cargo update for pinned polars")?;
                if !status.success() {
                    println!("⚠️  cargo update --precise failed (lockfile may already be correct)");
                }
                return Ok(());
            }
        }
    }

    println!(
        "{}",
        "📦 Updating Polars (git, branch=main) to latest commit...".blue()
    );

    // 1) Discover latest commit on main
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("https://github.com/pola-rs/polars")
        .arg("refs/heads/main")
        .output()
        .await
        .context("Failed to run 'git ls-remote' for Polars")?;
    if !output.status.success() {
        bail!("git ls-remote failed for Polars main");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Unable to parse Polars main HEAD sha"))?;

    // 2) Pin workspace lockfile to that exact commit for the 'polars' package
    let status = Command::new("cargo")
        .arg("update")
        .arg("-w")
        .arg("-p")
        .arg("polars")
        .arg("--precise")
        .arg(sha)
        .status()
        .await
        .context("Failed to execute 'cargo update -w -p polars --precise <sha>'")?;

    if !status.success() {
        bail!(
            "Polars update failed - 'cargo update -w -p polars --precise <sha>' exited with non-zero status"
        );
    }

    println!("{} {}", "✅ Polars pinned to commit".green(), sha);
    Ok(())
}
