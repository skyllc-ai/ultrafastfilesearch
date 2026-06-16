// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Operator-facing service control: `--status` / `--start` / `--stop`.
//!
//! All three use native SCM via `uffs-winsvc` (numeric, locale-proof
//! states), so they behave identically regardless of the console's
//! language — unlike parsing `sc query` text. `--install` / `--uninstall`
//! stay in [`super::service`]; these are the day-to-day control verbs.

use anyhow::Result;
use uffs_broker_protocol::{PIPE_NAME, SERVICE_NAME};

/// Short pipe-probe budget for `--status` (ms) — a snapshot, not a wait.
const STATUS_PIPE_PROBE_MS: u32 = 1_000;

/// Print the broker service's status: state, pid, and whether its pipe is
/// actually serving (the probe is skipped unless the service is running).
#[expect(clippy::print_stdout, reason = "operator-facing status output")]
pub(crate) fn status() {
    let info = uffs_winsvc::query(SERVICE_NAME);
    println!("Service : {SERVICE_NAME}");
    println!("State   : {}", info.state.label());
    match info.pid {
        Some(pid) => println!("PID     : {pid}"),
        None => println!("PID     : -"),
    }
    let serving =
        info.state.is_running() && uffs_winsvc::pipe_serving(PIPE_NAME, STATUS_PIPE_PROBE_MS);
    println!(
        "Pipe    : {}",
        if serving { "serving" } else { "not serving" }
    );
}

/// Start the broker service and wait until it reports RUNNING.
///
/// # Errors
///
/// Propagates an SCM open/start failure or a start timeout.
pub(crate) fn start() -> Result<()> {
    uffs_winsvc::start(SERVICE_NAME)
}

/// Stop the broker service and wait until it reports STOPPED.
///
/// # Errors
///
/// Propagates an SCM open/control failure or a stop timeout.
pub(crate) fn stop() -> Result<()> {
    uffs_winsvc::stop(SERVICE_NAME)
}
