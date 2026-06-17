// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! The update **journal** — the crash-consistency spine (design §19.2/19.3).
//!
//! A single JSON file, updated **atomically** (write a `.tmp`, then
//! `rename` over the target — `std::fs::rename` replaces atomically on
//! both Windows and Unix) at *every* state transition. It is both the
//! recovery substrate (Phase H reads it to finish-or-undo an interrupted
//! run) and the audit trail (`--status`).
//!
//! This module owns the data model + atomic persistence + the state
//! machine. It performs **no** mutation of the install — the apply /
//! recover phases drive it. Keeping it side-effect-free (besides its own
//! file) makes the invariants in §19.0 unit-testable.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Overall state of an update run (the §19.3 state machine).
///
/// Ordering matters: everything up to and **including** [`Self::SmokeOk`]
/// is *before* the commit point; [`Self::Restored`] onward is *after*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateState {
    /// Journal created; nothing done yet.
    Init,
    /// Pre-flight checks passed (disk, perms, backup-dir).
    PreflightOk,
    /// Payload downloaded + verified into staging.
    Acquired,
    /// Core services stopped + files unlocked.
    Quiesced,
    /// Mid-swap (at least one binary replaced).
    Swapping,
    /// All targeted binaries replaced.
    Swapped,
    /// All replaced binaries passed `--self-check` (the commit point).
    SmokeOk,
    /// Services relaunched into the new binaries.
    Restored,
    /// Post-flight liveness verified.
    Verified,
    /// Successful, finished.
    Done,
    /// A failure before the commit point is being undone.
    RollingBack,
    /// Rollback complete; services restored to the old binaries.
    RolledBack,
    /// Terminal failure state after a completed rollback.
    Aborted,
}

impl UpdateState {
    /// `true` once the run has reached a terminal state — nothing for
    /// Phase H to do.
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Aborted)
    }
}

/// Per-binary progress within a target root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BinaryStatus {
    /// Not yet touched.
    Pending,
    /// Original renamed aside to its `.bak`.
    BackedUp,
    /// New image moved into place.
    Swapped,
    /// New image passed `--self-check`.
    SmokeOk,
    /// Restored from `.bak` during rollback.
    RolledBack,
}

/// One binary's journal entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BinaryEntry {
    /// Logical stem (e.g. `uffsd`).
    pub(crate) name: String,
    /// Per-binary swap/rollback progress.
    pub(crate) status: BinaryStatus,
    /// Backup file name (in `backup_dir`) once backed up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) backup: Option<String>,
    /// True when this binary was newly **added** (no prior file existed) by a
    /// completeness reconcile. There is no `.bak`, so rollback **deletes** the
    /// placed image rather than restoring a backup.
    #[serde(default)]
    pub(crate) added: bool,
}

/// One install root being updated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TargetEntry {
    /// Install directory.
    pub(crate) root: PathBuf,
    /// Channel that placed it (`winget` / `unmanaged`).
    pub(crate) channel: String,
    /// The binaries to update in this root.
    pub(crate) binaries: Vec<BinaryEntry>,
}

/// One append-only journal event (the debug/audit trail).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JournalEvent {
    /// Seconds since the Unix epoch when the step was recorded.
    pub(crate) unix: u64,
    /// Dotted step name, e.g. `quiesce.daemon.stopped`.
    pub(crate) step: String,
}

/// The update journal (the §19.2 schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Journal {
    /// Unique id for this run.
    pub(crate) update_id: String,
    /// When the run started (unix seconds).
    pub(crate) started_unix: u64,
    /// PID that owns the run — used by Phase H to detect a dead owner.
    pub(crate) owner_pid: u32,
    /// Whether the run is elevated (broker step needs it).
    pub(crate) elevated: bool,
    /// Version before the update.
    pub(crate) from_version: String,
    /// Target version.
    pub(crate) to_version: String,
    /// File name of the Phase-B snapshot this run is based on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot_ref: Option<String>,
    /// Backup directory for this run's `.bak` files.
    pub(crate) backup_dir: PathBuf,
    /// Current overall state.
    pub(crate) state: UpdateState,
    /// `true` once every targeted binary is swapped **and** smoke-tested —
    /// the point of no return (§19.3).
    pub(crate) commit_point_passed: bool,
    /// Roots + per-binary status.
    pub(crate) targets: Vec<TargetEntry>,
    /// Services stopped during Quiesce that Phase H must restart (INV-1).
    pub(crate) services_stopped: Vec<String>,
    /// `WinGet`-managed roots this run **delegated** (did not swap, §19.6).
    /// Recorded so `--status`/support can see a winget install that the
    /// updater intentionally left for `winget upgrade` — never a silent
    /// version mismatch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) delegated_winget: Vec<String>,
    /// Append-only event trail.
    pub(crate) events: Vec<JournalEvent>,
    /// Where this journal persists. Not serialised.
    #[serde(skip)]
    path: PathBuf,
}

impl Journal {
    /// Create a fresh journal in [`UpdateState::Init`].
    pub(crate) fn new(
        path: PathBuf,
        from_version: &str,
        to_version: &str,
        backup_dir: PathBuf,
    ) -> Self {
        let started_unix = unix_now();
        Self {
            update_id: format!("{started_unix:x}-{}", std::process::id()),
            started_unix,
            owner_pid: std::process::id(),
            elevated: false,
            from_version: from_version.to_owned(),
            to_version: to_version.to_owned(),
            snapshot_ref: None,
            backup_dir,
            state: UpdateState::Init,
            commit_point_passed: false,
            targets: Vec::new(),
            services_stopped: Vec::new(),
            delegated_winget: Vec::new(),
            events: Vec::new(),
            path,
        }
    }

    /// Load a journal from `path` (Phase H).
    ///
    /// # Errors
    ///
    /// Fails if the file is missing or not valid journal JSON.
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading journal {}", path.display()))?;
        let mut journal: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing journal {}", path.display()))?;
        journal.path = path.to_path_buf();
        Ok(journal)
    }

    /// Atomically persist the journal (write `.tmp`, then `rename` over
    /// the target — never leaves a torn file, INV-2).
    ///
    /// # Errors
    ///
    /// Propagates any write/rename failure.
    pub(crate) fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self).context("serialising journal")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    /// Archive a terminal journal: rename the live `journal.json` to a
    /// sibling `journal.json.last` (overwriting any prior). This keeps an
    /// audit copy while freeing the live path as the unambiguous
    /// "an update is in-flight or crashed" signal that the CLI self-heal
    /// trigger checks (§19.4). Best-effort — a failure to archive is not
    /// fatal to an already-completed update.
    pub(crate) fn archive(&self) {
        let dest = self.path.with_extension("json.last");
        let _archived = std::fs::rename(&self.path, &dest);
    }

    /// Record an event, transition to `state`, and persist atomically.
    ///
    /// # Errors
    ///
    /// Propagates the persistence failure.
    pub(crate) fn transition(&mut self, state: UpdateState, step: &str) -> Result<()> {
        self.state = state;
        self.events.push(JournalEvent {
            unix: unix_now(),
            step: step.to_owned(),
        });
        self.save()
    }

    /// Cross the commit point — the point of no return (§19.3). After
    /// this, recovery rolls *forward*; before it, *back*.
    ///
    /// # Errors
    ///
    /// Propagates the persistence failure.
    pub(crate) fn commit(&mut self) -> Result<()> {
        self.commit_point_passed = true;
        self.transition(UpdateState::SmokeOk, "commit_point.passed")
    }

    /// Update one binary's status (no-op if the binary isn't tracked).
    pub(crate) fn set_binary_status(&mut self, root: &Path, name: &str, status: BinaryStatus) {
        for target in &mut self.targets {
            if target.root == root {
                for binary in &mut target.binaries {
                    if binary.name == name {
                        binary.status = status;
                    }
                }
            }
        }
    }

    /// Mark a binary as newly added (no prior file) so rollback deletes it
    /// rather than restoring a non-existent `.bak`.
    pub(crate) fn set_binary_added(&mut self, root: &Path, name: &str) {
        for target in &mut self.targets {
            if target.root == root {
                for binary in &mut target.binaries {
                    if binary.name == name {
                        binary.added = true;
                    }
                }
            }
        }
    }

    /// `true` when this journal describes an interrupted run that Phase H
    /// should finish or undo: not terminal **and** its owner is gone.
    pub(crate) const fn needs_recovery(&self, owner_alive: bool) -> bool {
        !self.state.is_terminal() && !owner_alive
    }
}

/// Seconds since the Unix epoch (0 if the clock predates it).
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |dur| dur.as_secs())
}

#[cfg(test)]
mod tests {
    use super::{BinaryEntry, BinaryStatus, Journal, TargetEntry, UpdateState};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("uffs-journal-test-{}-{name}", std::process::id()))
            .join("journal.json")
    }

    fn sample(path: std::path::PathBuf) -> Journal {
        let mut journal = Journal::new(path, "0.6.1", "0.6.2", "/tmp/backup".into());
        journal.targets = vec![TargetEntry {
            root: "/opt/uffs".into(),
            channel: "unmanaged".to_owned(),
            binaries: vec![BinaryEntry {
                name: "uffsd".to_owned(),
                status: BinaryStatus::Pending,
                backup: None,
                added: false,
            }],
        }];
        journal
    }

    #[test]
    fn atomic_round_trip_preserves_state() {
        let path = temp_path("roundtrip");
        let mut journal = sample(path.clone());
        journal
            .transition(UpdateState::Acquired, "acquire.done")
            .expect("save");
        journal.set_binary_status(
            std::path::Path::new("/opt/uffs"),
            "uffsd",
            BinaryStatus::Swapped,
        );
        journal.save().expect("save");

        let loaded = Journal::load(&path).expect("load");
        assert_eq!(loaded.state, UpdateState::Acquired);
        assert_eq!(loaded.from_version, "0.6.1");
        let binary = loaded
            .targets
            .first()
            .and_then(|target| target.binaries.first())
            .expect("one binary");
        assert_eq!(binary.status, BinaryStatus::Swapped);
        assert!(
            loaded
                .events
                .iter()
                .any(|event| event.step == "acquire.done")
        );
        let _removed = std::fs::remove_file(&path);
    }

    #[test]
    fn commit_point_sets_flag_and_state() {
        let path = temp_path("commit");
        let mut journal = sample(path.clone());
        assert!(!journal.commit_point_passed);
        journal.commit().expect("commit");
        assert!(journal.commit_point_passed);
        assert_eq!(journal.state, UpdateState::SmokeOk);
        let _removed = std::fs::remove_file(&path);
    }

    #[test]
    fn needs_recovery_only_when_interrupted_and_owner_dead() {
        let path = temp_path("recover");
        let mut journal = sample(path);
        journal.state = UpdateState::Swapping;
        // owner alive → defer, don't recover
        assert!(!journal.needs_recovery(true));
        // owner dead + non-terminal → recover
        assert!(journal.needs_recovery(false));
        // terminal → never recover
        journal.state = UpdateState::Done;
        assert!(!journal.needs_recovery(false));
    }

    #[test]
    fn terminal_states() {
        assert!(UpdateState::Done.is_terminal());
        assert!(UpdateState::Aborted.is_terminal());
        assert!(!UpdateState::Swapping.is_terminal());
        assert!(!UpdateState::SmokeOk.is_terminal());
    }

    #[test]
    fn archive_frees_live_path_and_keeps_audit_copy() {
        let path = temp_path("archive");
        let journal = sample(path.clone());
        journal.save().expect("save");
        assert!(path.exists(), "live journal should exist before archive");
        journal.archive();
        assert!(!path.exists(), "live journal must be gone after archive");
        let last = path.with_extension("json.last");
        assert!(last.exists(), "audit copy must exist after archive");
        let _removed = std::fs::remove_file(&last);
    }
}
