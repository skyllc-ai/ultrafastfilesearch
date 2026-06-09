// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for secure filesystem operations (split out of `fs.rs` to
//! keep that file under the 800-line policy ceiling; re-attached via
//! `#[path = "fs/tests.rs"] mod tests;`).

use super::*;

#[cfg(unix)]
#[test]
fn create_new_secure_file_is_0600() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.bin");
    let file = create_new_secure_file(&path).unwrap();
    let mode = file.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "file must be born 0600, got {mode:o}");
}

#[test]
fn create_new_secure_file_rejects_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exists.bin");
    std::fs::write(&path, b"pre-existing").unwrap();
    let err = create_new_secure_file(&path).expect_err("must refuse existing path");
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[cfg(unix)]
#[test]
fn create_new_secure_file_rejects_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, b"sentinel").unwrap();
    let link = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    // create_new must refuse to follow/replace the symlink.
    let err = create_new_secure_file(&link).expect_err("must refuse symlink");
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    // The symlink's target content is untouched.
    assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
}

#[cfg(unix)]
#[test]
fn write_secret_file_is_0600_with_content() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key.bin");
    write_secret_file(&path, b"deadbeef").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read(&path).unwrap(), b"deadbeef");
}

#[cfg(unix)]
#[test]
fn create_secure_dir_births_0700() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a").join("b").join("c");
    create_secure_dir(&nested).unwrap();
    // Each component WE created is born 0700.
    for comp in [dir.path().join("a"), dir.path().join("a").join("b"), nested] {
        let mode = std::fs::metadata(&comp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{} mode {mode:o} != 0700", comp.display());
    }
}

#[test]
fn create_secure_dir_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("x").join("y");
    create_secure_dir(&target).unwrap();
    // Second call on an existing dir must still succeed.
    create_secure_dir(&target).unwrap();
}

#[cfg(unix)]
#[test]
fn atomic_write_sets_0600() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.bin");
    atomic_write(&path, b"payload").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "atomic_write final file must be 0600, got {mode:o}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
}

#[test]
fn atomic_write_overwrites_existing_target() {
    // The TARGET may pre-exist (rename replaces it); only the randomised
    // TEMP uses create_new. Confirms we didn't break replace semantics.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.bin");
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
}

#[test]
fn atomic_write_concurrent_no_collision() {
    // `tempdir` outlives the threads because we join all of them before
    // it drops at end-of-scope — so each thread can take an owned
    // `PathBuf` clone without needing `Arc`.
    let tempdir = tempfile::tempdir().unwrap();
    let target = tempdir.path().join("shared.bin");

    let handles: Vec<_> = (0_u8..8)
        .map(|writer_id| {
            let thread_target = target.clone();
            std::thread::spawn(move || {
                let payload = vec![writer_id; 256];
                atomic_write(&thread_target, &payload).unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    // Final content must be exactly one writer's payload (256 equal bytes),
    // never an interleaving of two writers (randomised temps must not
    // collide).
    let final_bytes = std::fs::read(&target).unwrap();
    assert_eq!(final_bytes.len(), 256);
    let first = final_bytes.first().copied().unwrap();
    assert!(
        final_bytes.iter().all(|&byte| byte == first),
        "final file must be one writer's payload, not interleaved"
    );

    // No leftover temp files in the dir.
    let leftover = std::fs::read_dir(tempdir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".uffs.tmp"))
        .count();
    assert_eq!(leftover, 0, "no temp files should remain");
}

#[test]
fn secure_remove_zeroes_then_unlinks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("victim.bin");
    std::fs::write(&path, vec![0xFF_u8; 4096]).unwrap();
    secure_remove(&path).unwrap();
    assert!(!path.exists(), "file must be unlinked after secure_remove");
}

#[test]
fn secure_remove_absent_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.bin");
    // Removing a missing path is a no-op success.
    secure_remove(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn paths_identical_true_for_hardlink() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("orig.bin");
    std::fs::write(&original, b"data").unwrap();
    let hardlink = dir.path().join("hard.bin");
    std::fs::hard_link(&original, &hardlink).unwrap();
    assert!(
        paths_identical(&original, &hardlink).unwrap(),
        "hardlink names the same file"
    );
}

#[cfg(unix)]
#[test]
fn paths_identical_true_for_symlink_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, b"data").unwrap();
    let link = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    // paths_identical follows symlinks → same file.
    assert!(paths_identical(&target, &link).unwrap());
}

#[cfg(unix)]
#[test]
fn paths_identical_false_for_different_files() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("a.bin");
    let second = dir.path().join("b.bin");
    std::fs::write(&first, b"a").unwrap();
    std::fs::write(&second, b"b").unwrap();
    assert!(
        !paths_identical(&first, &second).unwrap(),
        "distinct files must not be identical"
    );
}

#[test]
fn create_new_file_exclusive_rejects_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exists.bin");
    std::fs::write(&path, b"pre-existing").unwrap();
    let err = create_new_file_exclusive(&path).expect_err("must refuse existing path");
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[cfg(unix)]
#[test]
fn create_new_file_exclusive_rejects_symlink() {
    // The TOCTOU/symlink hardening must survive even without the owner-only
    // ACL: `create_new` still refuses to follow a pre-planted symlink.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, b"sentinel").unwrap();
    let link = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let err = create_new_file_exclusive(&link).expect_err("must refuse symlink");
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
}

#[test]
fn create_new_file_exclusive_writes_content() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.csv");
    let mut file = create_new_file_exclusive(&path).unwrap();
    file.write_all(b"a,b,c\n").unwrap();
    drop(file);
    assert_eq!(std::fs::read(&path).unwrap(), b"a,b,c\n");
}

/// Regression guard for the v0.5.111 performance regression: the secure-fs
/// module must never shell out to a subprocess (the old owner-only ACL path
/// invoked `icacls.exe`) to set permissions. That put a ~tens-of-ms process
/// spawn on every result write, because `create_new_secure_file` runs per
/// `--out` query. Permissions are now applied via native Win32 ACL APIs; this
/// test fails loudly if a `process::Command` spawn creeps back into `fs.rs`.
/// We match the spawn API (not the word "icacls", which appears in the
/// explanatory comments) so the guard tracks behaviour, not prose.
#[test]
fn fs_module_spawns_no_subprocess() {
    let source = include_str!("../fs.rs");
    assert!(
        !source.contains("Command::new") && !source.contains("process::Command"),
        "fs.rs must not spawn a subprocess (perf regression guard, WI-8.1 / v0.5.111)"
    );
}

#[cfg(unix)]
#[test]
fn secure_remove_follows_symlink_to_target_then_unlinks_link() {
    // Documents the chosen semantics: `secure_remove` opens the path for
    // write (following a symlink to its target, the OS default), zeroes
    // the TARGET's bytes via the fd, then unlinks the LINK. We assert the
    // observable end state: the link is gone. (The pre-stat removal in
    // WI-1.1 means size + overwrite now go through one fd, eliminating the
    // metadata→open re-resolution.)
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, vec![0xAB_u8; 1024]).unwrap();
    let link = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    secure_remove(&link).unwrap();
    assert!(!link.exists(), "the symlink itself must be removed");
}
