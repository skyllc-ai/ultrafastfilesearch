// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Quiesce (design §8): stop the core resident services so their files
//! unlock, recording each stop in the journal so Phase H can restart them
//! (INV-1).
//!
//! Robust by construction — **no fragile external tools, no `sc` text
//! parsing**:
//! - daemon / MCP: graceful via our **own** in-tree `uffs daemon stop` / `uffs
//!   mcp stop`, then poll the daemon **PID file** (deleted on clean exit);
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

/// `uffs` binary next to a component's image (else bare `uffs` on PATH).
fn uffs_sibling(running: &SnapRunning) -> PathBuf {
    running
        .image_path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(
            || PathBuf::from(exe_name("uffs")),
            |dir| dir.join(exe_name("uffs")),
        )
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
    let _ignore = Command::new(&uffs).args(["daemon", "stop"]).status();
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
    let _ignore = Command::new(&uffs).args(["mcp", "stop"]).status();
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
    use super::{uffs_sibling, wait_until};
    use crate::plan::SnapRunning;

    fn running_with_image(image: &std::path::Path) -> SnapRunning {
        let json = format!(
            r#"{{ "component": "daemon", "pid": 1, "image_path": {:?} }}"#,
            image.to_string_lossy()
        );
        serde_json::from_str(&json).expect("running")
    }

    #[test]
    fn uffs_sibling_next_to_image() {
        // Host-valid path so `Path::parent` works on the test runner.
        let dir = std::env::temp_dir();
        let run = running_with_image(&dir.join("uffsd"));
        let sib = uffs_sibling(&run);
        assert_eq!(sib.parent(), Some(dir.as_path()));
        assert!(
            sib.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("uffs"))
        );
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
