// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Journal-driven apply orchestration (design §19.3): build a journal
//! from the snapshot, then `backup → swap → smoke → commit`, rolling back
//! on any **pre-commit** failure. After the commit point the new binaries
//! are good; only forward motion (restore + verify) remains.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::apply;
use crate::journal::{BinaryEntry, BinaryStatus, Journal, TargetEntry, UpdateState};
use crate::plan::Snapshot;

/// Smoke-check argument (`--self-check` where supported, §19.8).
pub(crate) const SMOKE_ARG: &str = "--version";

/// Platform executable file name for a binary stem.
pub(crate) fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// Build a fresh journal from a snapshot: one target per **unmanaged**
/// root (`WinGet` is delegated elsewhere), every binary `Pending`.
pub(crate) fn journal_from_snapshot(
    journal_path: PathBuf,
    snapshot: &Snapshot,
    backup_dir: PathBuf,
) -> Journal {
    let mut journal = Journal::new(
        journal_path,
        &snapshot.prior_version(),
        snapshot.to_version(),
        backup_dir,
    );
    journal.targets = snapshot
        .unmanaged_targets()
        .map(|target| TargetEntry {
            root: target.root.clone(),
            channel: target.channel.clone(),
            binaries: target
                .binaries
                .iter()
                .map(|binary| BinaryEntry {
                    name: binary.name.clone(),
                    status: BinaryStatus::Pending,
                    backup: None,
                })
                .collect(),
        })
        .collect();
    journal
}

/// (root, stem, on-disk target path) for every targeted binary.
fn collect_plan(journal: &Journal) -> Vec<(PathBuf, String, PathBuf)> {
    journal
        .targets
        .iter()
        .flat_map(|target| {
            target.binaries.iter().map(move |binary| {
                let path = target.root.join(exe_name(&binary.name));
                (target.root.clone(), binary.name.clone(), path)
            })
        })
        .collect()
}

/// Apply every targeted binary from `staged_dir`, smoke-test all, then
/// cross the commit point. `smoke` is injected so both paths are
/// testable; production passes [`apply::smoke_ok`].
///
/// On any failure **before** the commit point, rolls back fully and
/// returns the error (the originals are restored, INV-3).
///
/// # Errors
///
/// Returns the underlying swap/smoke failure (after rollback).
pub(crate) fn apply_all<F>(journal: &mut Journal, staged_dir: &Path, smoke: F) -> Result<()>
where
    F: Fn(&Path) -> bool,
{
    journal.transition(UpdateState::Swapping, "apply.begin")?;
    let plan = collect_plan(journal);

    // backup + swap each
    for (root, stem, target) in &plan {
        let staged = staged_dir.join(exe_name(stem));
        if let Err(err) = apply::backup_and_swap(&staged, target) {
            rollback_all(journal)?;
            return Err(err);
        }
        journal.set_binary_status(root, stem, BinaryStatus::Swapped);
        journal.save()?;
    }
    journal.transition(UpdateState::Swapped, "apply.all_swapped")?;

    // smoke each new image before the commit point
    for (root, stem, target) in &plan {
        if !smoke(target) {
            rollback_all(journal)?;
            return Err(anyhow!("smoke test failed for {stem}; rolled back"));
        }
        journal.set_binary_status(root, stem, BinaryStatus::SmokeOk);
    }

    journal.commit()?; // point of no return
    Ok(())
}

/// Restore every backed-up binary (pre-commit rollback, INV-3).
///
/// # Errors
///
/// Propagates a restore failure.
pub(crate) fn rollback_all(journal: &mut Journal) -> Result<()> {
    journal.transition(UpdateState::RollingBack, "rollback.begin")?;
    for (root, stem, target) in collect_plan(journal) {
        apply::restore(&target)?;
        journal.set_binary_status(&root, &stem, BinaryStatus::RolledBack);
    }
    journal.transition(UpdateState::RolledBack, "rollback.done")?;
    Ok(())
}

/// Prune every backup after a committed, verified update (best-effort).
pub(crate) fn prune_all(journal: &Journal) {
    for (_, _, target) in collect_plan(journal) {
        let _pruned = apply::prune_backup(&target);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{apply_all, exe_name, journal_from_snapshot};
    use crate::journal::{BinaryStatus, UpdateState};
    use crate::plan::Snapshot;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uffs-orch-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn snapshot_for(root: &Path) -> Snapshot {
        let json = format!(
            r#"{{ "to_version": "0.6.2", "targets": [
                 {{ "root": {root:?}, "channel": "unmanaged",
                    "binaries": [ {{ "name": "uffsd", "on_disk_version": "0.6.1" }} ] }} ],
               "running": [] }}"#,
        );
        serde_json::from_str(&json).expect("snap")
    }

    fn setup(tag: &str) -> (PathBuf, PathBuf, PathBuf, crate::journal::Journal) {
        let base = scratch(tag);
        let root = base.join("install");
        let stage = base.join("stage");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&stage).expect("stage");
        std::fs::write(root.join(exe_name("uffsd")), "OLD").expect("old");
        std::fs::write(stage.join(exe_name("uffsd")), "NEW").expect("new");
        let journal = journal_from_snapshot(
            base.join("journal.json"),
            &snapshot_for(&root),
            base.join("backup"),
        );
        (root, stage, base, journal)
    }

    #[test]
    fn apply_success_swaps_and_commits() {
        let (root, stage, _base, mut journal) = setup("ok");
        apply_all(&mut journal, &stage, |_| true).expect("apply");
        assert!(journal.commit_point_passed);
        assert_eq!(journal.state, UpdateState::SmokeOk);
        assert_eq!(
            std::fs::read_to_string(root.join(exe_name("uffsd"))).expect("read"),
            "NEW"
        );
    }

    #[test]
    fn smoke_failure_rolls_back_to_old() {
        let (root, stage, _base, mut journal) = setup("rollback");
        let result = apply_all(&mut journal, &stage, |_| false);
        assert!(result.is_err());
        assert_eq!(journal.state, UpdateState::RolledBack);
        assert!(!journal.commit_point_passed);
        // Original restored.
        assert_eq!(
            std::fs::read_to_string(root.join(exe_name("uffsd"))).expect("read"),
            "OLD"
        );
    }

    #[test]
    fn journal_built_from_unmanaged_targets() {
        let (root, _stage, _base, journal) = setup("build");
        assert_eq!(journal.to_version, "0.6.2");
        assert_eq!(journal.from_version, "0.6.1");
        assert_eq!(journal.targets.len(), 1);
        let target = journal.targets.first().expect("one target");
        assert_eq!(target.root, root);
        let binary = target.binaries.first().expect("one binary");
        assert_eq!(binary.status, BinaryStatus::Pending);
    }
}
