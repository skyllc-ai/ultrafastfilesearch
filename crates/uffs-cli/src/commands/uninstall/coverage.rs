// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows deep-sweep drive coverage for `uffs --uninstall`.
//!
//! Before the live cross-drive search, make sure the daemon is running and
//! indexes every NTFS drive, **offering** to start it and index the missing
//! drives so the sweep is actually complete. Windows-only: off Windows UFFS
//! indexes offline MFT captures, not the live filesystem, so there is no live
//! drive coverage to ensure.
//!
//! Best-effort throughout: any RPC failure leaves coverage as-is and the sweep
//! proceeds against whatever is currently indexed.

#![cfg(windows)]

use anyhow::Result;
use uffs_client::connect_sync::UffsClientSync;
use uffs_mft::platform::{DriveLetter, detect_ntfs_drives};

/// How long to wait for newly-requested drives to finish loading before the
/// sweep runs. Generous because a cold multi-drive index is genuinely slow
/// (millions of records per volume); the previous 120 s cap expired mid-load on
/// a 7-drive system (~2.5 min), so the sweep searched a not-yet-ready index and
/// missed the still-loading drive.
const INDEX_WAIT: core::time::Duration = core::time::Duration::from_secs(600);

/// Ensure the daemon covers every NTFS drive before the deep sweep, offering to
/// start it and index the missing drives. `confirm` prompts the user (returns
/// their yes/no). Returns `Ok(())` whether or not coverage was completed — the
/// caller sweeps regardless.
///
/// # Errors
///
/// Propagates only a failure of the `confirm` callback itself; daemon/RPC
/// failures are swallowed (best-effort coverage).
pub(crate) fn ensure_drive_coverage(confirm: &mut dyn FnMut(&str) -> Result<bool>) -> Result<()> {
    let all = detect_ntfs_drives();
    if all.is_empty() {
        return Ok(());
    }
    // `connect()` auto-starts the daemon if it is not already running.
    let Ok(mut client) = UffsClientSync::connect() else {
        // Could not reach or start a daemon: nothing to cover, sweep as-is.
        return Ok(());
    };
    let indexed: Vec<DriveLetter> = client
        .drives()
        .map(|response| {
            response
                .drives
                .into_iter()
                .map(|drive| drive.letter)
                .collect()
        })
        .unwrap_or_default();
    let missing: Vec<DriveLetter> = all
        .into_iter()
        .filter(|drive| !indexed.contains(drive))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let count = missing.len();
    let list = missing
        .iter()
        .map(|drive| format!("{drive}:"))
        .collect::<Vec<_>>()
        .join(", ");
    let prompt = format!(
        "\nThe deep sweep needs all {count} drives indexed first ({list}). This builds\n\
         an on-disk index cache (uses disk + memory, and is kept even on a dry run)\n\
         and can take a few minutes. Index them now? [y/N] "
    );
    if confirm(&prompt)? {
        // Fire the (blocking) load on a *background* connection so this thread can
        // poll `status_drives` for live progress while the daemon works through
        // the drives. The load RPC can exceed the client timeout on a big
        // multi-drive index — but the poll, not the RPC return, decides when the
        // drives are searchable, so a background timeout is harmless.
        let to_load = missing.clone();
        let loader = std::thread::spawn(move || {
            if let Ok(mut background) = UffsClientSync::connect_raw() {
                // Best-effort: the poll below is the source of truth for "ready".
                let _outcome = background.load_drive_letters(&to_load, false);
            }
        });
        wait_until_loaded(&mut client, &missing);
        let _joined = loader.join();
    }
    Ok(())
}

/// Poll interval while waiting for requested drives to finish loading.
const POLL_INTERVAL: core::time::Duration = core::time::Duration::from_millis(500);

/// Poll `status_drives` until every drive in `wanted` reports a loaded
/// (searchable) shard — tier `hot` or `warm` — or [`INDEX_WAIT`] elapses.
///
/// A freshly `load_drive_letters`-requested shard starts parked/cold and only
/// becomes searchable once its body is resident; searching before then is what
/// returned zero strays. Best-effort: any RPC error just keeps polling to the
/// deadline, after which the sweep proceeds with whatever is loaded.
fn wait_until_loaded(client: &mut UffsClientSync, wanted: &[DriveLetter]) {
    let deadline = std::time::Instant::now() + INDEX_WAIT;
    let mut last_ready = usize::MAX;
    loop {
        let ready = client.status_drives().map_or(0, |resp| {
            wanted
                .iter()
                .filter(|drive| {
                    resp.drives.iter().any(|row| {
                        row.letter == **drive && matches!(row.tier.as_str(), "hot" | "warm")
                    })
                })
                .count()
        });
        // Progress feedback so a multi-minute index never looks like a hang.
        if ready != last_ready {
            print_index_progress(ready, wanted.len());
            last_ready = ready;
        }
        if ready == wanted.len() || std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Print drive-index progress for [`wait_until_loaded`].
#[expect(clippy::print_stdout, reason = "CLI progress output")]
fn print_index_progress(ready: usize, total: usize) {
    println!("  indexing for the sweep: {ready}/{total} drives ready...");
}
