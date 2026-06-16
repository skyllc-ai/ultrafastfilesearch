// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase H — recover / resume (design §19.4).
//!
//! Reads the journal and, when its owning updater is **gone** (crashed,
//! killed, powered-off) and the run is **not terminal**, deterministically
//! finishes it: roll **forward** if past the commit point (the new
//! binaries are good — just restart services), else roll **back** (restore
//! `.bak`, restart services). Either branch ends by ensuring every service
//! the run stopped is running again (INV-1).
//!
//! Decided **solely** by `state` vs `commit_point_passed` — no guessing.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::journal::{Journal, UpdateState};
use crate::{orchestrate, plan, proc, restore};

/// What a recovery run did (for the caller's report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// No journal, or the run already finished — nothing to do.
    NothingToDo,
    /// The owning updater is still alive — another run owns this; defer.
    InProgress,
    /// Past the commit point: finished the update (restart + verify).
    RolledForward,
    /// Before the commit point: restored the old binaries.
    RolledBack,
}

/// Run Phase H against the journal at `journal_path`.
///
/// # Errors
///
/// Propagates a rollback / restore failure (rare — both are best-effort
/// where possible).
pub(crate) fn recover(journal_path: &Path) -> Result<Recovery> {
    let Ok(mut journal) = Journal::load(journal_path) else {
        return Ok(Recovery::NothingToDo); // no/invalid journal
    };
    if journal.state.is_terminal() {
        return Ok(Recovery::NothingToDo);
    }
    let owner_alive = proc::is_alive(journal.owner_pid);
    if !journal.needs_recovery(owner_alive) {
        return Ok(Recovery::InProgress); // owner still running
    }

    let snapshot = load_snapshot(&journal)?;
    if journal.commit_point_passed {
        roll_forward(&mut journal, &snapshot)?;
        Ok(Recovery::RolledForward)
    } else {
        roll_back(&mut journal, &snapshot)?;
        Ok(Recovery::RolledBack)
    }
}

/// Load the snapshot the journal was built against.
fn load_snapshot(journal: &Journal) -> Result<plan::Snapshot> {
    let snapshot_ref = journal
        .snapshot_ref
        .as_deref()
        .context("journal has no snapshot_ref; cannot recover service state")?;
    plan::Snapshot::load(Path::new(snapshot_ref))
}

/// Past the commit point: the new binaries are good. Restart services into
/// them, prune backups, finish.
fn roll_forward(journal: &mut Journal, snapshot: &plan::Snapshot) -> Result<()> {
    let _failed = restore::restore(snapshot); // INV-1: bring services back up
    journal.transition(UpdateState::Restored, "recover.roll_forward")?;
    orchestrate::prune_all(journal);
    journal.transition(UpdateState::Done, "recover.done")?;
    journal.archive();
    Ok(())
}

/// Before the commit point: restore the old binaries, then restart
/// services so nothing is left down (INV-1).
fn roll_back(journal: &mut Journal, snapshot: &plan::Snapshot) -> Result<()> {
    orchestrate::rollback_all(journal)?;
    let _failed = restore::restore(snapshot); // INV-1
    journal.transition(UpdateState::Aborted, "recover.roll_back")?;
    journal.archive();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Recovery, recover};
    use crate::journal::{Journal, UpdateState};

    fn journal_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("uffs-recover-{}-{tag}", std::process::id()))
            .join("journal.json")
    }

    #[test]
    fn missing_journal_is_nothing_to_do() {
        let path = journal_path("missing");
        assert_eq!(recover(&path).expect("recover"), Recovery::NothingToDo);
    }

    #[test]
    fn live_owner_defers() {
        // A journal owned by THIS (alive) process must not be recovered.
        let path = journal_path("live");
        let mut journal = Journal::new(path.clone(), "0.6.1", "0.6.2", "/tmp/bak".into());
        journal.state = UpdateState::Swapping; // non-terminal, owner = us (alive)
        journal.save().expect("save");
        assert_eq!(recover(&path).expect("recover"), Recovery::InProgress);
        let _removed = std::fs::remove_file(&path);
    }

    #[test]
    fn terminal_journal_is_nothing_to_do() {
        let path = journal_path("done");
        let mut journal = Journal::new(path.clone(), "0.6.1", "0.6.2", "/tmp/bak".into());
        journal.state = UpdateState::Done;
        journal.save().expect("save");
        assert_eq!(recover(&path).expect("recover"), Recovery::NothingToDo);
        let _removed = std::fs::remove_file(&path);
    }
}
