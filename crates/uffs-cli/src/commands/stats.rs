// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stats command implementation.
//!
//! **Daemon mode** (no path): routed through `SearchConfig::aggregate_only`
//! + `run_with_config` in `main.rs` — reuses the full search daemon
//! lifecycle (auto-start, await_ready, data-dir forwarding).
//!
//! Legacy parquet-mode stats have been removed from the thin CLI.
//! Use `uffsd` directly or `uffs --stats` (daemon mode) instead.

use std::path::Path;

use anyhow::Result;

/// Show statistics.
///
/// Daemon-mode stats (no path) are handled in `main.rs` via the search path.
/// Parquet-mode stats (path given) are no longer supported by the thin CLI.
///
/// # Errors
///
/// Returns an error if a path is given (no longer supported) or if
/// daemon routing fails.
pub fn stats(path: Option<&Path>, _top: u32) -> Result<()> {
    match path {
        None => {
            anyhow::bail!("stats without a path should be routed through search path in main.rs")
        }
        Some(dir) => {
            anyhow::bail!(
                "Legacy parquet stats for '{}' are no longer supported by the thin CLI.\n\
                 Use `uffs --stats` (daemon mode) instead.",
                dir.display()
            )
        }
    }
}
