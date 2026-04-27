// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! CLI command implementations.
//!
//! This module provides the public command surface for the UFFS CLI and shared
//! helpers used by the split command modules.

/// Aggregate analytics subcommand.
pub mod aggregate;
/// Daemon management subcommands.
pub mod daemon_mgmt;
// Index and info subcommands were merged into other modules.
/// MCP server management subcommands.
pub mod mcp_mgmt;
/// Output helpers for search results.
pub mod output;
/// Search command implementation.
pub mod search;
/// Stats subcommand implementation.
pub mod stats;
/// Combined `uffs status` command.
pub mod system_status;

/// Render a one-line version summary suitable for `daemon status`,
/// `daemon stats`, and `uffs status` output.
///
/// `daemon_version` is the daemon-reported `env!("CARGO_PKG_VERSION")`
/// from [`uffs_client::protocol::response::StatusResponse::version`]
/// (or `StatsResponse::version`).  The CLI's own compile-time version
/// is read locally; when the two differ we surface both with a
/// `MISMATCH` flag so a stale long-running daemon paired with a
/// freshly-upgraded CLI binary (or vice versa) is visible at a glance
/// instead of being inferred from the existing mtime-based stale-binary
/// heuristic in `system_status::print_daemon_status`.
///
/// Pre-0.5.79 daemons send no version; rendered as `<unknown>`.
///
/// The function returns the value portion only — callers prepend
/// their own label/indentation (`Version:`, `  Version:`, etc.) so a
/// single helper covers both indented and non-indented layouts.
pub(crate) fn version_summary(daemon_version: &str) -> String {
    let cli_version = env!("CARGO_PKG_VERSION");
    if daemon_version.is_empty() {
        format!("<unknown> (daemon) / {cli_version} (cli)")
    } else if daemon_version == cli_version {
        cli_version.to_owned()
    } else {
        format!("{daemon_version} (daemon) / {cli_version} (cli)  ⚠ MISMATCH")
    }
}

/// Format a number with comma separators.
fn format_number(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, ch) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Format file size in human-readable format.
#[expect(
    clippy::float_arithmetic,
    reason = "division for human-readable size formatting"
)]
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    // Precision loss acceptable — display-only formatting where ±1 byte is fine.
    #[expect(
        clippy::cast_precision_loss,
        reason = "display-only human-readable formatting"
    )]
    let bytes_f64 = bytes as f64;
    if bytes >= TB {
        #[expect(
            clippy::cast_precision_loss,
            reason = "display-only human-readable formatting"
        )]
        let divisor = TB as f64;
        format!("{:.2} TB", bytes_f64 / divisor)
    } else if bytes >= GB {
        #[expect(
            clippy::cast_precision_loss,
            reason = "display-only human-readable formatting"
        )]
        let divisor = GB as f64;
        format!("{:.2} GB", bytes_f64 / divisor)
    } else if bytes >= MB {
        #[expect(
            clippy::cast_precision_loss,
            reason = "display-only human-readable formatting"
        )]
        let divisor = MB as f64;
        format!("{:.2} MB", bytes_f64 / divisor)
    } else if bytes >= KB {
        #[expect(
            clippy::cast_precision_loss,
            reason = "display-only human-readable formatting"
        )]
        let divisor = KB as f64;
        format!("{:.2} KB", bytes_f64 / divisor)
    } else {
        format!("{bytes} B")
    }
}
