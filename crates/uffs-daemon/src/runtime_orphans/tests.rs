// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Unit tests for [`super`] (`runtime_orphans`).
//!
//! All three tests exercise [`super::sweep_runtime_tempfile_orphans_at`]
//! directly with the production [`DefaultRuntimeDir`] over a
//! `tempfile::TempDir` so the production code path runs in isolation
//! from the real `<cache_dir>/runtime/`.

use uffs_security::runtime_dir::DefaultRuntimeDir;

use super::sweep_runtime_tempfile_orphans_at;

/// `u32::MAX` as a PID is overwhelmingly unlikely to be alive on
/// any host — Unix `kill(pid, 0)` returns ESRCH and Windows
/// `OpenProcess` returns `ERROR_INVALID_PARAMETER`.  Both surface
/// as "dead" in the platform liveness probe.
const SENTINEL_DEAD_PID: u32 = u32::MAX;

#[test]
fn sweep_creates_runtime_root_when_missing() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let runtime_root = tmp.path().join("runtime");
    assert!(!runtime_root.exists(), "precondition: root must be missing");

    sweep_runtime_tempfile_orphans_at(&runtime_root, &DefaultRuntimeDir::default());

    assert!(
        runtime_root.is_dir(),
        "sweep should create the runtime root if missing"
    );
}

#[test]
fn sweep_removes_dead_pid_dir_only() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let runtime_root = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_root).expect("create runtime root");

    // Dead-PID subdir with a stale runtime tempfile inside.
    let dead_pid_dir = runtime_root.join(SENTINEL_DEAD_PID.to_string());
    std::fs::create_dir(&dead_pid_dir).expect("create dead-pid dir");
    std::fs::write(dead_pid_dir.join("C_compact_0.live"), b"stale").expect("create stale tempfile");

    // Live-PID subdir (us — the test runner is alive by definition).
    let live_pid = std::process::id();
    let live_pid_dir = runtime_root.join(live_pid.to_string());
    std::fs::create_dir(&live_pid_dir).expect("create live-pid dir");
    std::fs::write(live_pid_dir.join("D_compact_0.live"), b"in use").expect("create live tempfile");

    sweep_runtime_tempfile_orphans_at(&runtime_root, &DefaultRuntimeDir::default());

    assert!(
        !dead_pid_dir.exists(),
        "dead-PID subdir must be swept ({})",
        dead_pid_dir.display()
    );
    assert!(
        live_pid_dir.exists(),
        "live-PID subdir must NOT be swept ({})",
        live_pid_dir.display()
    );
}

#[test]
fn sweep_does_not_panic_when_root_unwritable() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // Place a regular file at a path, then try to create a runtime
    // root *inside* it — `create_secure_dir` must fail with
    // `NotADirectory` / similar.  The helper must log and return
    // without bubbling out.
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"i am a file, not a dir").expect("create blocker file");
    let runtime_root = blocker.join("runtime");

    // No assertion on side effects — the contract is "must not
    // panic, must not propagate the error".  The function returns
    // unit so the type system enforces no error escapes.
    sweep_runtime_tempfile_orphans_at(&runtime_root, &DefaultRuntimeDir::default());
}
