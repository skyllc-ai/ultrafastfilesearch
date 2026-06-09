// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

#![expect(
    clippy::print_stdout,
    reason = "operational CLI tool — version bump confirmations + tip messages go to stdout (issue #212)"
)]

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

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use colored::Colorize as _;
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
            return Ok(version.to_owned());
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
                && let Some((_, after_eq)) = trimmed.split_once('=')
            {
                let version = after_eq.trim().trim_matches('"').trim_matches('\'');
                return Ok(version.to_owned());
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
