// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs mcp <action>` — thin shim that delegates to the `uffsmcp` binary.
//!
//! The actual MCP server logic lives in the `uffs-mcp` crate and its
//! standalone `uffsmcp` binary.  This module simply exec's / spawns
//! `uffsmcp` with the right arguments, keeping `uffs` thin.

use anyhow::{Context as _, Result};

/// Execute an MCP management action by forwarding raw args to `uffsmcp`.
///
/// This is the passthrough path — raw args after `uffs mcp` are forwarded
/// directly to the `uffsmcp` binary without any parsing.
///
/// # Errors
///
/// Returns an error if `uffsmcp` is not found or exits with a non-zero code.
pub(crate) fn mcp_from_args(args: &[String]) -> Result<()> {
    exec_uffsmcp(args)
}

/// Find and run `uffsmcp` binary.
fn exec_uffsmcp(args: &[String]) -> Result<()> {
    let exe = find_uffsmcp()?;

    let status = std::process::Command::new(&exe)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .with_context(|| format!("Failed to run {}", exe.display()))?;

    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(1);
        anyhow::bail!("uffsmcp exited with code {code}");
    }
}

/// Locate the `uffsmcp` binary.
///
/// Looks in the same directory as the current `uffs` executable first,
/// then falls back to `PATH`.
fn find_uffsmcp() -> Result<std::path::PathBuf> {
    // Same directory as the running uffs binary.
    if let Some(candidate) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(uffsmcp_filename())))
        .filter(|path| path.exists())
    {
        return Ok(candidate);
    }

    // Fall back to PATH lookup.
    let name = uffsmcp_filename();
    which::which(name).with_context(|| {
        format!("Cannot find `{name}` — install it alongside `uffs` or add it to PATH")
    })
}

/// Platform-specific binary name.
const fn uffsmcp_filename() -> &'static str {
    if cfg!(windows) {
        "uffsmcp.exe"
    } else {
        "uffsmcp"
    }
}
