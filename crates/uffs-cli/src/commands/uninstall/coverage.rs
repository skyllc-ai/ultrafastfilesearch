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
/// sweep runs. A best-effort cap — a slow HDD index may still be in flight.
const INDEX_WAIT: core::time::Duration = core::time::Duration::from_secs(120);

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
    let list = missing
        .iter()
        .map(|drive| format!("{drive}:"))
        .collect::<Vec<_>>()
        .join(", ");
    let prompt = format!(
        "\nThe deep sweep searches every indexed drive. Not yet indexed: {list}.\n\
         Indexing builds the on-disk index cache for those drives (uses disk +\n\
         memory, and persists even under --dry-run). Index {list} now for a\n\
         complete sweep? [y/N] "
    );
    if confirm(&prompt)? && client.load_drive_letters(&missing, false).is_ok() {
        // Wait for the freshly-requested drives to become searchable — poll for
        // readiness rather than a blind fixed wait, so the sweep never searches a
        // still-parked shard (and returns as soon as they are loaded).
        wait_until_loaded(&mut client, &missing);
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
    loop {
        let all_loaded = client.status_drives().is_ok_and(|resp| {
            wanted.iter().all(|drive| {
                resp.drives
                    .iter()
                    .any(|row| row.letter == *drive && matches!(row.tier.as_str(), "hot" | "warm"))
            })
        });
        if all_loaded || std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
