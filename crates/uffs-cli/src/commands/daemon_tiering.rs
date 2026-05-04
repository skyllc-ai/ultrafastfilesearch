// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs daemon {hibernate, preload}` subcommand handlers.
//!
//! Phase 8-B / 8-C — operator-driven memory-tiering CLI commands.
//! Split off [`crate::commands::daemon_mgmt`] so the tiering cluster
//! stays under the 800-LOC policy ceiling without a file-size exception.
//! Forward-looking: sub-phases 8-D (`forget`) and 8-E
//! (`status_drives` table) will land their CLI shims in this same
//! file when the corresponding daemon RPCs come online.

use anyhow::{Context, Result};
use uffs_client::connect_sync::UffsClientSync;
use uffs_client::protocol::response::{
    DEFAULT_PRELOAD_PIN_MINUTES, HibernateParams, PreloadParams,
};

/// `uffs daemon hibernate [DRIVES...]` — demote loaded shards to `Cold`.
///
/// Releases RAM but keeps the encrypted compact cache on disk so a
/// subsequent `preload` / search can re-warm without a full MFT
/// re-parse.
///
/// Empty `drives` ⇒ hibernate every drive the daemon currently knows
/// about.  The daemon's response carries the per-pre-call-tier
/// breakdown which we render so the operator sees what actually
/// changed (vs `already_cold` no-ops).
///
/// # Errors
///
/// Returns an error when the daemon is not running or the
/// `hibernate` RPC fails.
///
/// # Example
///
/// ```bash
/// $ uffs daemon hibernate
/// Daemon hibernated 2 drive(s):
///   Hot     -> Cold:  C
///   Warm    -> Cold:  D
///   Already Cold:     (none)
/// ```
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub fn daemon_hibernate(drives: &[char]) -> Result<()> {
    let mut client = UffsClientSync::connect_raw()
        .map_err(|err| anyhow::anyhow!("Daemon is not running: {err}"))?;

    let params = HibernateParams {
        drives: drives.to_vec(),
    };
    let response = client
        .hibernate(&params)
        .with_context(|| "hibernate RPC failed")?;

    let total_demoted =
        response.hot_demoted.len() + response.warm_demoted.len() + response.parked_demoted.len();
    println!("Daemon hibernated {total_demoted} drive(s):");
    println!(
        "  Hot     -> Cold:  {}",
        format_drive_list(&response.hot_demoted)
    );
    println!(
        "  Warm    -> Cold:  {}",
        format_drive_list(&response.warm_demoted)
    );
    println!(
        "  Parked  -> Cold:  {}",
        format_drive_list(&response.parked_demoted)
    );
    println!(
        "  Already Cold:     {}",
        format_drive_list(&response.already_cold)
    );
    Ok(())
}

/// `uffs daemon preload <DRIVES...> [--pin-minutes N]` — promote
/// drive(s) to `Hot` and pin the tier against demote for
/// `pin_minutes` minutes (defaults to
/// [`DEFAULT_PRELOAD_PIN_MINUTES`] when `None`).
///
/// At least one drive is required; the CLI parser at
/// [`crate::args::parse_daemon_action`] enforces that pre-flight,
/// so by the time this function runs the `drives` slice is non-
/// empty.  Per-drive failures (drive not loaded, body load failure,
/// transient state) are reported in the daemon's `errors` field
/// rather than as a top-level `Result::Err`.
///
/// # Errors
///
/// Returns an error when the daemon is not running or the `preload`
/// RPC itself fails (network / serialisation).  Per-drive
/// preload failures land in stdout under `Errors:` and do not
/// fail the CLI.
///
/// # Example
///
/// ```bash
/// $ uffs daemon preload C D --pin-minutes 60
/// Daemon preloaded:
///   Promoted to Hot:   C, D
///   Already Hot:       (none)
///   Pin expires at:    1700001800000 (Unix-millis)
/// ```
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub fn daemon_preload(drives: &[char], pin_minutes: Option<u32>) -> Result<()> {
    let mut client = UffsClientSync::connect_raw()
        .map_err(|err| anyhow::anyhow!("Daemon is not running: {err}"))?;

    let params = PreloadParams {
        drives: drives.to_vec(),
        pin_minutes,
    };
    let response = client
        .preload(&params)
        .with_context(|| "preload RPC failed")?;

    let effective_pin = pin_minutes.unwrap_or(DEFAULT_PRELOAD_PIN_MINUTES);
    println!("Daemon preloaded ({effective_pin}-min pin):");
    println!(
        "  Promoted to Hot:  {}",
        format_drive_list(&response.promoted)
    );
    println!(
        "  Already Hot:      {}",
        format_drive_list(&response.already_hot)
    );
    if response.pin_until_unix_ms > 0 {
        println!(
            "  Pin expires at:   {} (Unix-millis)",
            response.pin_until_unix_ms
        );
    }
    if !response.errors.is_empty() {
        println!("  Errors:");
        for err in &response.errors {
            println!("    {err}");
        }
    }
    Ok(())
}

/// Render a slice of drive letters as a comma-separated list, or
/// `"(none)"` when the slice is empty.  Keeps the per-line output
/// in [`daemon_hibernate`] / [`daemon_preload`] visually aligned
/// even on no-op calls.
fn format_drive_list(drives: &[char]) -> String {
    if drives.is_empty() {
        "(none)".to_owned()
    } else {
        drives
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}
