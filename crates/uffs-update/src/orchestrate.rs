// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Journal-driven apply orchestration (design §19.3): build a journal
//! from the snapshot, then `backup → swap → smoke → commit`, rolling back
//! on any **pre-commit** failure. After the commit point the new binaries
//! are good; only forward motion (restore + verify) remains.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};

use crate::apply;
use crate::journal::{BinaryEntry, BinaryStatus, Journal, TargetEntry, UpdateState};
use crate::plan::Snapshot;

/// Smoke-check argument (`--self-check` where supported, §19.8).
pub(crate) const SMOKE_ARG: &str = "--version";

/// Platform executable file name for a binary stem — the **on-disk**
/// name an installed binary has (`uffsd` → `uffsd.exe` on Windows).
pub(crate) fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// Release **asset** name for a binary stem — the platform-suffixed name
/// the release publishes (e.g. `uffsd-windows-x64.exe`), matching the
/// `final-release/` naming in `.github/workflows/release.yml`. Distinct
/// from [`exe_name`]: we download the suffixed asset but stage it under
/// the plain on-disk name so the apply phase finds it. The suffix tracks
/// the release build matrix (`x86_64` Windows/Linux, `aarch64` macOS).
pub(crate) fn asset_name(stem: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{stem}-windows-x64.exe")
    } else if cfg!(target_os = "macos") {
        format!("{stem}-macos-arm64")
    } else {
        format!("{stem}-linux-x64")
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
                    added: false,
                })
                .collect(),
        })
        .collect();
    // Record WinGet roots we intentionally delegate (§19.6, R3) so they
    // are visible in the journal rather than silently dropped.
    journal.delegated_winget = snapshot
        .winget_targets()
        .map(|target| target.root.display().to_string())
        .collect();
    journal
}

/// Pre-flight guard (§19, Phase D): prove the update *can* succeed
/// **before** any service is stopped — every staged binary is present,
/// and every target directory is writable. A failure here is zero-
/// downtime: nothing has been quiesced or swapped yet.
///
/// # Errors
///
/// A missing staged binary, or a target directory we cannot write to
/// (typically: needs elevation).
pub(crate) fn preflight(journal: &Journal, staged_dir: &Path) -> Result<()> {
    for (root, stem, _target) in collect_plan(journal) {
        let staged = staged_dir.join(exe_name(&stem));
        if !staged.is_file() {
            return Err(anyhow!(
                "staged binary missing: {} (acquire incomplete?)",
                staged.display()
            ));
        }
        ensure_writable_dir(&root)?;
    }
    Ok(())
}

/// Confirm `dir` accepts writes by round-tripping a uniquely-named probe
/// file. A failure means the swap would later fail mid-flight (after a
/// service is already stopped) — so we surface it up front.
fn ensure_writable_dir(dir: &Path) -> Result<()> {
    let probe = dir.join(format!(".uffs-update-probe-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .with_context(|| format!("target dir not writable: {} (try elevated)", dir.display()))?;
    let _removed = std::fs::remove_file(&probe);
    Ok(())
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

    // backup + swap each (or, for a missing core binary, *add* it)
    for (root, stem, target) in &plan {
        let staged = staged_dir.join(exe_name(stem));
        match apply::backup_and_swap(&staged, target) {
            // `None` ⇒ the target did not exist: a completeness add. Record it
            // so rollback deletes the placed image instead of hunting a `.bak`.
            Ok(backup) => {
                if backup.is_none() {
                    journal.set_binary_added(root, stem);
                }
                journal.set_binary_status(root, stem, BinaryStatus::Swapped);
                journal.save()?;
            }
            Err(err) => {
                rollback_all(journal)?;
                return Err(err);
            }
        }
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

/// Roll back every touched binary (pre-commit rollback, INV-3): restore a
/// **replaced** binary from its `.bak`, and **delete** an **added** binary
/// (no `.bak` exists). Both primitives are idempotent on untouched targets.
///
/// # Errors
///
/// Propagates a restore / remove failure.
pub(crate) fn rollback_all(journal: &mut Journal) -> Result<()> {
    journal.transition(UpdateState::RollingBack, "rollback.begin")?;
    // Snapshot (root, stem, target, added) first — the `set_binary_status`
    // below needs `&mut journal`, so we cannot hold an iterator into it.
    let items: Vec<(PathBuf, String, PathBuf, bool)> = journal
        .targets
        .iter()
        .flat_map(|target| {
            let root = target.root.clone();
            target.binaries.iter().map(move |binary| {
                let path = root.join(exe_name(&binary.name));
                (root.clone(), binary.name.clone(), path, binary.added)
            })
        })
        .collect();
    for (root, stem, target, added) in items {
        if added {
            apply::remove_added(&target)?;
        } else {
            apply::restore(&target)?;
        }
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

    use super::{apply_all, asset_name, exe_name, journal_from_snapshot, preflight};
    use crate::journal::{BinaryStatus, UpdateState};
    use crate::plan::Snapshot;

    #[test]
    fn asset_name_is_platform_suffixed_and_differs_from_on_disk_name() {
        let asset = asset_name("uffsd");
        // The release publishes platform-suffixed assets; the on-disk name
        // is plain. Acquire downloads the former, stages as the latter.
        assert_ne!(asset, exe_name("uffsd"), "asset must be platform-suffixed");
        let expected = if cfg!(target_os = "windows") {
            "uffsd-windows-x64.exe"
        } else if cfg!(target_os = "macos") {
            "uffsd-macos-arm64"
        } else {
            "uffsd-linux-x64"
        };
        assert_eq!(asset, expected);
    }

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
    fn journal_records_delegated_winget_roots() {
        let json = r#"{ "to_version": "0.6.2", "targets": [
             { "root": "/opt/uffs", "channel": "unmanaged",
               "binaries": [ { "name": "uffsd", "on_disk_version": "0.6.1" } ] },
             { "root": "/winget/uffs", "channel": "winget",
               "binaries": [ { "name": "uffs", "on_disk_version": "0.6.1" } ] } ],
           "running": [] }"#;
        let snapshot: Snapshot = serde_json::from_str(json).expect("snap");
        let journal = journal_from_snapshot("/tmp/j.json".into(), &snapshot, "/tmp/bak".into());
        assert_eq!(journal.targets.len(), 1, "only unmanaged is swapped");
        assert_eq!(
            journal.delegated_winget,
            vec!["/winget/uffs".to_owned()],
            "winget root recorded for visibility, not silently dropped"
        );
    }

    #[test]
    fn preflight_passes_when_staged_present_and_writable() {
        let (_root, stage, _base, journal) = setup("pf-ok");
        preflight(&journal, &stage).expect("preflight should pass");
    }

    #[test]
    fn preflight_fails_when_staged_binary_missing() {
        let (_root, _stage, base, journal) = setup("pf-missing");
        let empty_stage = base.join("empty-stage");
        std::fs::create_dir_all(&empty_stage).expect("mkdir");
        let err = preflight(&journal, &empty_stage).expect_err("missing staged must fail");
        assert!(err.to_string().contains("staged binary missing"), "{err}");
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
