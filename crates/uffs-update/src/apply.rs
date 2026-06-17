// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Apply primitives: atomic **backup → swap → smoke** and **restore**
//! (design §9 / §19.3 / §19.8).
//!
//! The crash-sensitive core. Every file transition is an **atomic
//! same-volume rename** (`std::fs::rename` replaces atomically on Windows
//! and Unix), and the original is always renamed aside to `<file>.bak`
//! **before** the new image moves in — so a crash at any point leaves
//! either the old file, the `.bak`, or the new file in place, never a
//! torn one (INV-2), and rollback is always possible (INV-3).
//!
//! These operate on already-staged binaries; extracting the verified
//! bundle into the staging dir is a separate step. Smoke-testing
//! (`--self-check` / `--version`) gates the commit point (§19.8): we
//! confirm the new image *executes on this host* before betting services
//! on it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// The `.bak` path for a binary (same directory ⇒ same volume ⇒ atomic
/// rename).
pub(crate) fn backup_path(binary: &Path) -> PathBuf {
    let mut name = binary.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// Rename `binary` aside to its `.bak`. Idempotent: if the `.bak` already
/// exists (a prior interrupted run), this is a no-op so recovery can
/// re-run it safely (INV-5).
///
/// # Errors
///
/// Propagates a rename failure (e.g. the file is still locked — Quiesce
/// should have prevented that).
pub(crate) fn backup(binary: &Path) -> Result<PathBuf> {
    let bak = backup_path(binary);
    if bak.exists() {
        return Ok(bak); // already backed up
    }
    std::fs::rename(binary, &bak)
        .with_context(|| format!("backing up {} → {}", binary.display(), bak.display()))?;
    Ok(bak)
}

/// Move a staged new image into place at `target` (atomic rename). The
/// caller must have [`backup`]'d `target` first.
///
/// **Self-replacement (§19.7):** when `target` is the running
/// orchestrator's *own* image, this still succeeds — Windows permits
/// renaming a running `.exe` aside (the prior [`backup`]) and creating a
/// new file at the freed name; the process keeps executing from the
/// renamed image's section. The only residue is the `.bak` the live
/// process cannot delete during its own run; [`sweep_stale_backups`]
/// reclaims it on the next orchestrator launch (we deliberately do **not**
/// re-exec mid-run — finishing with the code we started is predictable;
/// hot-swapping a running orchestrator's code mid-flight is not).
///
/// # Errors
///
/// Propagates a rename failure.
pub(crate) fn swap_in(staged: &Path, target: &Path) -> Result<()> {
    std::fs::rename(staged, target)
        .with_context(|| format!("swapping {} → {}", staged.display(), target.display()))
}

/// Sweep orphaned `<name>.bak` files in `dir`: remove each `.bak` whose
/// live sibling (`<name>`) currently exists — i.e. a *completed* swap
/// whose backup a prior run could not prune (the self-replacement case,
/// §19.7). A `.bak` with **no** live sibling is left untouched: that is a
/// mid-rollback state owned by recovery, never a stray to reclaim here.
///
/// Best-effort: a still-locked `.bak` (the prior process not yet exited)
/// is skipped silently and reclaimed on a later launch. Returns the count
/// removed. The caller must only invoke this when **no** update is
/// in-flight (no live journal), so it never races an active swap.
pub(crate) fn sweep_stale_backups(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0_usize;
    for entry in entries.flatten() {
        let bak = entry.path();
        if bak.extension().is_none_or(|ext| ext != "bak") {
            continue;
        }
        // The live sibling is the path with `.bak` stripped.
        let live = bak.with_extension("");
        if live.exists() && std::fs::remove_file(&bak).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

/// Restore `binary` from its `.bak` (rollback). Idempotent: a no-op if no
/// `.bak` exists (nothing was backed up, or already restored).
///
/// # Errors
///
/// Propagates a rename failure.
pub(crate) fn restore(binary: &Path) -> Result<()> {
    let bak = backup_path(binary);
    if !bak.exists() {
        return Ok(());
    }
    // Remove a half-swapped new image first so the restore rename can land.
    if binary.exists() {
        std::fs::remove_file(binary)
            .with_context(|| format!("clearing {} before restore", binary.display()))?;
    }
    std::fs::rename(&bak, binary)
        .with_context(|| format!("restoring {} → {}", bak.display(), binary.display()))
}

/// Smoke-test a binary: run `<binary> <check_arg>` and require exit `0`.
///
/// Confirms the image *executes on this host* (right arch, no missing
/// runtime DLL, not corrupt) **before** the commit point. `check_arg` is
/// `--self-check` where supported, else `--version`.
pub(crate) fn smoke_ok(binary: &Path, check_arg: &str) -> bool {
    Command::new(binary)
        .arg(check_arg)
        .status()
        .is_ok_and(|status| status.success())
}

/// Prune a binary's `.bak` after a committed, verified update.
///
/// # Errors
///
/// Propagates a remove failure.
pub(crate) fn prune_backup(binary: &Path) -> Result<()> {
    let bak = backup_path(binary);
    if bak.exists() {
        std::fs::remove_file(&bak).with_context(|| format!("pruning backup {}", bak.display()))?;
    }
    Ok(())
}

/// Place the staged image at `target`, atomically.
///
/// - **Replace** (target exists): back the old image aside to `.bak`, then swap
///   the new one in — returns `Some(bak)`. Rollback = restore the `.bak`.
/// - **Add** (target absent): no prior image to back up, so just place the new
///   one — returns `None`. This is the completeness path; rollback of an added
///   binary is a *delete* (the caller records `BinaryEntry::added`).
///
/// # Errors
///
/// The staged image is missing, or a backup/rename fails.
pub(crate) fn backup_and_swap(staged: &Path, target: &Path) -> Result<Option<PathBuf>> {
    if !staged.is_file() {
        bail!("staged image missing: {}", staged.display());
    }
    if target.exists() {
        let bak = backup(target)?;
        swap_in(staged, target)?;
        Ok(Some(bak))
    } else {
        swap_in(staged, target)?;
        Ok(None)
    }
}

/// Roll back an **added** binary (no `.bak` exists): delete the image we
/// placed. Idempotent — an already-absent target is success.
///
/// # Errors
///
/// The file exists but cannot be removed.
pub(crate) fn remove_added(target: &Path) -> Result<()> {
    if target.exists() {
        std::fs::remove_file(target)
            .with_context(|| format!("removing added binary {}", target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        backup, backup_and_swap, backup_path, prune_backup, remove_added, restore, swap_in,
        sweep_stale_backups,
    };

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("uffs-apply-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::write(path, content).expect("write");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    #[test]
    fn backup_swap_round_trip() {
        let dir = scratch("swap");
        let target = dir.join("uffsd");
        let staged = dir.join("uffsd.new");
        write(&target, "OLD");
        write(&staged, "NEW");

        let bak = backup_and_swap(&staged, &target)
            .expect("apply")
            .expect("replace returns a backup");
        assert_eq!(read(&target), "NEW", "target holds the new image");
        assert_eq!(read(&bak), "OLD", "backup holds the old image");
        assert!(!staged.exists(), "staged consumed by the rename");

        // Rollback restores the old image.
        restore(&target).expect("restore");
        assert_eq!(read(&target), "OLD");
        assert!(!bak.exists(), "backup consumed by the restore");
    }

    #[test]
    fn add_then_rollback_deletes_the_added_binary() {
        // Completeness "add": the target does not exist yet.
        let dir = scratch("add");
        let target = dir.join("uffs-mft");
        let staged = dir.join("uffs-mft.new");
        write(&staged, "NEW");

        // Add → no backup, target placed, return is None (signals "added").
        let backup = backup_and_swap(&staged, &target).expect("add");
        assert!(backup.is_none(), "an add has no backup");
        assert_eq!(read(&target), "NEW", "added image is in place");
        assert!(!backup_path(&target).exists(), "no .bak for an add");

        // Rollback of an add = delete the placed image.
        remove_added(&target).expect("remove_added");
        assert!(!target.exists(), "rollback removed the added binary");
        // Idempotent on an already-absent target.
        remove_added(&target).expect("remove_added is idempotent");
    }

    #[test]
    fn backup_is_idempotent() {
        let dir = scratch("idem");
        let target = dir.join("uffs");
        write(&target, "OLD");
        let bak1 = backup(&target).expect("first backup");
        // Simulate a re-run: a new image now sits at target.
        write(&target, "NEW");
        let bak2 = backup(&target).expect("second backup is a no-op");
        assert_eq!(bak1, bak2);
        assert_eq!(
            read(&bak1),
            "OLD",
            "idempotent backup never clobbers the saved old image"
        );
    }

    #[test]
    fn restore_clears_half_swapped_new_image() {
        let dir = scratch("half");
        let target = dir.join("uffsmcp");
        write(&target, "OLD");
        let _bak = backup(&target).expect("backup");
        // Crash simulated *after* a new image landed but before commit.
        write(&target, "HALF-NEW");
        restore(&target).expect("restore over the half-new image");
        assert_eq!(read(&target), "OLD");
    }

    #[test]
    fn restore_without_backup_is_noop() {
        let dir = scratch("noop");
        let target = dir.join("uffs-tui");
        write(&target, "CURRENT");
        restore(&target).expect("noop restore");
        assert_eq!(read(&target), "CURRENT");
    }

    #[test]
    fn missing_staged_image_aborts_before_backup() {
        let dir = scratch("missing");
        let target = dir.join("uffsd");
        write(&target, "OLD");
        let staged = dir.join("absent.new");
        backup_and_swap(&staged, &target).expect_err("missing staged image must abort");
        // The original must be untouched (no backup, no swap) on a pre-check fail.
        assert_eq!(read(&target), "OLD");
        assert!(!backup_path(&target).exists(), "no backup created on abort");
    }

    #[test]
    fn prune_removes_backup() {
        let dir = scratch("prune");
        let target = dir.join("uffs");
        write(&target, "NEW");
        write(&backup_path(&target), "OLD");
        prune_backup(&target).expect("prune");
        assert!(!backup_path(&target).exists());
    }

    #[test]
    fn sweep_reclaims_orphaned_backup_but_spares_rollback_state() {
        let dir = scratch("sweep");
        // Completed swap: live `uffs-update.exe` + its unprunable `.bak`.
        let live = dir.join("uffs-update.exe");
        write(&live, "NEW");
        write(&backup_path(&live), "OLD");
        // Mid-rollback state: a `.bak` whose live sibling is GONE.
        let orphan_bak = dir.join("uffsd.exe.bak");
        write(&orphan_bak, "OLD-ROLLBACK");

        let removed = sweep_stale_backups(&dir);
        assert_eq!(removed, 1, "only the completed-swap backup is reclaimed");
        assert!(!backup_path(&live).exists(), "completed-swap .bak removed");
        assert!(live.exists(), "the live binary is untouched");
        assert!(
            orphan_bak.exists(),
            "a .bak with no live sibling is rollback state — must be spared"
        );
    }

    #[test]
    fn sweep_missing_dir_is_zero() {
        let dir = scratch("sweep-missing").join("does-not-exist");
        assert_eq!(sweep_stale_backups(&dir), 0);
    }

    #[test]
    fn swap_in_is_a_plain_rename() {
        let dir = scratch("plain");
        let staged = dir.join("x.new");
        let target = dir.join("x");
        write(&staged, "Z");
        swap_in(&staged, &target).expect("swap");
        assert_eq!(read(&target), "Z");
    }
}
