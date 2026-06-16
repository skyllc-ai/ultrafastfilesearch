// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Quiesce (design §8): stop the core resident services so their files
//! unlock, recording each stop in the journal so Phase H can restart them
//! (INV-1).
//!
//! Robust by construction — **no fragile external tools, no `sc` text
//! parsing**:
//! - daemon / MCP: graceful via our **own** in-tree `uffs --daemon stop` /
//!   `uffs --mcp stop`, then poll the daemon **PID file** (deleted on clean
//!   exit);
//! - broker: native SCM stop via `uffs-winsvc` (numeric, locale-proof state —
//!   never `sc query` text, which is localized).

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use uffs_broker_protocol::SERVICE_NAME;

use crate::journal::{Journal, UpdateState};
use crate::orchestrate::exe_name;
use crate::plan::{SnapRunning, Snapshot};

/// How long to wait for a service to actually stop.
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting.
const POLL: Duration = Duration::from_millis(200);

/// Stop every running core component in dependency order (consumers
/// before providers: MCP → daemon → broker), recording each in
/// `journal.services_stopped`.
///
/// # Errors
///
/// Fails if a component will not stop within [`STOP_TIMEOUT`].
pub(crate) fn quiesce(journal: &mut Journal, snapshot: &Snapshot) -> Result<()> {
    for component in ["mcp", "daemon", "broker"] {
        for running in snapshot
            .running
            .iter()
            .filter(|run| run.component == component)
        {
            stop_component(component, running)?;
            journal.services_stopped.push(component.to_owned());
            journal.transition(
                UpdateState::Quiesced,
                &format!("quiesce.{component}.stopped"),
            )?;
        }
    }
    journal.transition(UpdateState::Quiesced, "quiesce.done")?;
    Ok(())
}

/// Stop one component by its kind.
fn stop_component(component: &str, running: &SnapRunning) -> Result<()> {
    match component {
        "daemon" => stop_daemon(running),
        "broker" => stop_broker(),
        "mcp" => {
            stop_mcp(running);
            Ok(())
        }
        other => bail!("unknown component to stop: {other}"),
    }
}

/// The `uffs` binary to drive a service stop. Prefer the one next to the
/// running `uffs-update` helper (the install being updated — its CLI carries
/// the **current** management logic, e.g. the privilege-aware stop gate), then
/// the component's own image dir, then bare `uffs` on PATH.
///
/// Stopping goes through the daemon socket, so *any* working `uffs` can stop
/// *any* daemon. Preferring the helper's sibling avoids driving the stop with
/// a **stale co-located** binary: a daemon launched from an old
/// `target/release` build would otherwise be stopped by that old `uffs`, whose
/// pre-fix Unix gate refuses → "daemon did not stop within 20s". (Manual
/// `~/bin/uffs --daemon stop` works precisely because it uses the current CLI.)
fn uffs_sibling(running: &SnapRunning) -> PathBuf {
    let helper_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let image_dir = running
        .image_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    pick_uffs(helper_dir.as_deref(), image_dir.as_deref())
}

/// Resolve the `uffs` binary: the first that exists among the helper dir then
/// the component image dir, else bare `uffs` (PATH). Pure — unit-tested.
fn pick_uffs(helper_dir: Option<&Path>, image_dir: Option<&Path>) -> PathBuf {
    for dir in [helper_dir, image_dir].into_iter().flatten() {
        let candidate = dir.join(exe_name("uffs"));
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(exe_name("uffs"))
}

/// Daemon PID file (`<lifecycle_dir>/daemon.pid`) — its absence is the
/// "daemon stopped" signal (the daemon deletes it on clean exit).
pub(crate) fn daemon_pid_file() -> PathBuf {
    dirs_next::data_local_dir()
        .map_or_else(|| PathBuf::from("/tmp/uffs"), |base| base.join("uffs"))
        .join("daemon.pid")
}

/// Graceful daemon stop, then wait for the PID file to disappear.
fn stop_daemon(running: &SnapRunning) -> Result<()> {
    let uffs = uffs_sibling(running);
    let _ignore = Command::new(&uffs).args(["--daemon", "stop"]).status();
    if wait_until(STOP_TIMEOUT, || !daemon_pid_file().exists()) {
        Ok(())
    } else {
        bail!("daemon did not stop within {STOP_TIMEOUT:?} (PID file lingered)")
    }
}

/// Stop the broker service via native SCM and wait until it reports STOPPED
/// (numeric state — `uffs-winsvc` waits internally). A no-op if it is not
/// installed or already stopped.
fn stop_broker() -> Result<()> {
    uffs_winsvc::stop(SERVICE_NAME).context("stopping the broker service")
}

/// Best-effort MCP gateway stop (a client just reconnects later).
fn stop_mcp(running: &SnapRunning) {
    let uffs = uffs_sibling(running);
    let _ignore = Command::new(&uffs).args(["--mcp", "stop"]).status();
}

/// Poll `done` every [`POLL`] until it returns `true` or `timeout` elapses.
pub(crate) fn wait_until(timeout: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{pick_uffs, wait_until};
    use crate::orchestrate::exe_name;

    #[test]
    fn pick_uffs_prefers_helper_then_image_then_path() {
        let base = std::env::temp_dir().join(format!("uffs-sib-{}", std::process::id()));
        let helper = base.join("helper");
        let image = base.join("image");
        std::fs::create_dir_all(&helper).expect("mk helper dir");
        std::fs::create_dir_all(&image).expect("mk image dir");
        let name = exe_name("uffs");

        // Both dirs have a `uffs` → prefer the helper's (install under update),
        // NOT the component's possibly-stale co-located binary.
        std::fs::write(helper.join(&name), b"x").expect("write helper uffs");
        std::fs::write(image.join(&name), b"x").expect("write image uffs");
        assert_eq!(pick_uffs(Some(&helper), Some(&image)), helper.join(&name));

        // Helper has none → fall back to the component's image dir.
        std::fs::remove_file(helper.join(&name)).expect("rm helper uffs");
        assert_eq!(pick_uffs(Some(&helper), Some(&image)), image.join(&name));

        // Neither has one → bare name (resolved on PATH at spawn time).
        std::fs::remove_file(image.join(&name)).expect("rm image uffs");
        assert_eq!(pick_uffs(Some(&helper), Some(&image)), PathBuf::from(&name));

        let _cleanup = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn wait_until_returns_true_when_predicate_holds() {
        let hits = core::cell::Cell::new(0_u8);
        let ok = wait_until(core::time::Duration::from_secs(2), || {
            hits.set(hits.get() + 1);
            hits.get() >= 2
        });
        assert!(ok);
        assert!(hits.get() >= 2);
    }

    #[test]
    fn wait_until_times_out() {
        let ok = wait_until(core::time::Duration::from_millis(10), || false);
        assert!(!ok);
    }
}
