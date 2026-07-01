// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows deep-sweep drive coverage for `uffs --uninstall`.
//!
//! Before the live cross-drive search, make sure the daemon manages **every**
//! NTFS drive on the system. Windows-only: off Windows UFFS indexes offline MFT
//! captures, not the live filesystem, so there is no live drive coverage to
//! ensure.
//!
//! The robust part: if the daemon is already managing every drive (any tier —
//! hot / warm / parked / cold — a search re-promotes a parked drive on demand)
//! we do nothing. If even **one** drive is missing, we do NOT hot-load it —
//! that races the daemon's own background startup load over the single-instance
//! Access Broker pipe and churns the registry (observed `2/6 -> 0/6`). Instead
//! we replicate the proven CLI flow: `uffs --daemon kill` then a clean
//! `uffs --daemon start`, which loads every drive with proper broker warm-up
//! and only returns once the daemon reports Ready.
//!
//! Best-effort throughout: any RPC failure leaves coverage as-is and the sweep
//! proceeds against whatever is currently managed.

#![cfg(windows)]

use core::time::Duration;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use uffs_client::connect_sync::UffsClientSync;
use uffs_mft::platform::{DriveLetter, detect_ntfs_drives};

/// Max time to wait for the clean `--daemon start` to bring every system drive
/// under management before the sweep proceeds anyway (best-effort). Generous
/// because a cold multi-drive index is genuinely slow (millions of records per
/// volume).
const COVERAGE_WAIT: Duration = Duration::from_secs(600);

/// Poll interval while waiting for the daemon to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long to wait for the daemon to fully exit after `--daemon kill` before
/// starting a fresh one (a lingering pipe would make `--daemon start` think a
/// daemon is still running and skip the start).
const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);

/// Ensure the daemon manages every NTFS drive before the deep sweep.
///
/// No-op when coverage is already complete. Otherwise runs the clean
/// kill-then-start flow. Best-effort: a missing daemon or RPC failure just
/// means the sweep covers whatever is already managed.
pub(crate) fn ensure_drive_coverage() {
    let all = detect_ntfs_drives();
    if all.is_empty() {
        return;
    }
    let managed = current_managed_drives();
    let missing: Vec<DriveLetter> = all
        .iter()
        .filter(|drive| !managed.contains(drive))
        .copied()
        .collect();
    if missing.is_empty() {
        // The daemon already covers every system drive — nothing to do.
        return;
    }
    clean_restart_for_coverage(&all, &missing);
}

/// The set of drive letters the daemon currently manages (any tier). An empty
/// list means the daemon is not running or did not answer.
fn current_managed_drives() -> Vec<DriveLetter> {
    UffsClientSync::connect_raw()
        .map_or_else(|_| Vec::new(), |mut client| managed_letters(&mut client))
}

/// Read the managed drive letters from `status_drives` (every row, regardless
/// of tier). Any RPC error yields an empty list (best-effort).
fn managed_letters(client: &mut UffsClientSync) -> Vec<DriveLetter> {
    client.status_drives().map_or_else(
        |_| Vec::new(),
        |resp| resp.drives.into_iter().map(|row| row.letter).collect(),
    )
}

/// Replicate the robust CLI flow — `--daemon kill` then a clean `--daemon
/// start` — so the daemon reloads every system drive from scratch, then wait
/// for it to come back covering them all.
fn clean_restart_for_coverage(all: &[DriveLetter], missing: &[DriveLetter]) {
    print_restart_intro(missing, all.len());
    let exe = uffs_client::daemon_ctl::find_uffs_exe();

    // KILL: bring the partially-loaded daemon fully down first.
    let _kill = run_uffs(&exe, &["--daemon", "kill"]);
    wait_until_daemon_down();

    // START: the clean startup path loads every detected drive (with broker
    // warm-up) and only returns once the daemon reports Ready.
    let _start = run_uffs(&exe, &["--daemon", "start"]);

    // Confirm the daemon came back covering every drive (progress feedback).
    wait_until_covered(all);
}

/// Run `uffs <args>` as a child, inheriting stdio so the daemon start output is
/// visible. Best-effort: a spawn failure is returned for the caller to ignore.
fn run_uffs(exe: &Path, args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
}

/// Poll until the daemon is no longer reachable (fully shut down) or
/// [`SHUTDOWN_WAIT`] elapses.
fn wait_until_daemon_down() {
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while Instant::now() < deadline {
        if UffsClientSync::connect_raw().is_err() {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Poll `status_drives` until every drive in `all` is managed again, or
/// [`COVERAGE_WAIT`] elapses. Prints progress so a multi-minute reload never
/// looks like a hang.
fn wait_until_covered(all: &[DriveLetter]) {
    let deadline = Instant::now() + COVERAGE_WAIT;
    let mut last_covered = usize::MAX;
    loop {
        let managed = current_managed_drives();
        let covered = all.iter().filter(|drive| managed.contains(drive)).count();
        if covered != last_covered {
            print_coverage_progress(covered, all.len());
            last_covered = covered;
        }
        if covered == all.len() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Announce the kill-then-start because coverage is incomplete.
#[expect(clippy::print_stdout, reason = "CLI progress output")]
fn print_restart_intro(missing: &[DriveLetter], total: usize) {
    let list = missing
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "\nDaemon is not indexing every drive (missing {list}; {covered} of {total} covered).\n\
         Restarting it cleanly (kill + start) for a complete deep sweep:",
        covered = total.saturating_sub(missing.len()),
    );
}

/// Print drive-coverage progress while the freshly started daemon reloads.
#[expect(clippy::print_stdout, reason = "CLI progress output")]
fn print_coverage_progress(covered: usize, total: usize) {
    println!("  indexing for the sweep: {covered}/{total} drives ready...");
}
