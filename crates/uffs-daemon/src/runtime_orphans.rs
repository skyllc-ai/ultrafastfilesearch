// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 2b memory-tiering: runtime-tempfile orphan cleanup.
//!
//! `compact_cache::deserialize_compact_into_runtime` (Phase 2b Commit D)
//! materialises records + names columns through
//! `<cache_dir>/runtime/<pid>/<drive>_compact_<seq>.live` runtime
//! tempfiles.  On Unix those files outlive the daemon process because
//! the kernel only reclaims them when both the file handle and every
//! mmap referencing them close — a `kill -9` / out-of-memory-kill /
//! power loss leaves the files behind.  Windows
//! `FILE_FLAG_DELETE_ON_CLOSE` self-cleans, but the empty `<pid>/`
//! directory wrapper still needs sweeping.
//!
//! [`sweep_runtime_tempfile_orphans`] runs once at daemon startup,
//! after [`crate::bootstrap_lifecycle_manager`] (so the PID file
//! proves we're the live daemon) and before any drive load (so the
//! sweep can't accidentally remove our own future runtime tempfile
//! subdir).
//!
//! Sweep failures emit `warn!` and never bubble out — startup must
//! not be blocked by a transient FS error.  Both helpers return `()`
//! so the type system enforces that contract.
//!
//! Plan reference: `docs/refactor/memory-tiering-implementation-plan.md`
//! §3 Phase 2b Commit E.

use std::path::Path;

use uffs_security::runtime_dir::{DefaultRuntimeDir, RuntimeDir};

/// Wipe runtime-tempfile leftovers from dead daemon PIDs.
///
/// Resolves `<cache_dir>/runtime/` via
/// [`uffs_core::compact_cache::compact_runtime_root`] and forwards to
/// [`sweep_runtime_tempfile_orphans_at`] with the production
/// [`DefaultRuntimeDir`].
pub(crate) fn sweep_runtime_tempfile_orphans() {
    let runtime_root = uffs_core::compact_cache::compact_runtime_root();
    sweep_runtime_tempfile_orphans_at(&runtime_root, &DefaultRuntimeDir::default());
}

/// Inner sweep implementation taking explicit runtime root +
/// [`RuntimeDir`] so tests can drive it with a `tempfile::TempDir`
/// instead of the process-wide cache dir.
///
/// Behaviour matches [`sweep_runtime_tempfile_orphans`] exactly: best
/// effort, never panics, never returns errors.
pub(crate) fn sweep_runtime_tempfile_orphans_at(runtime_root: &Path, runtime_dir: &dyn RuntimeDir) {
    // Idempotent — `create_secure_dir` is `mkdir -p` with owner-only
    // perms applied to existing dirs too.  Without this, the very
    // first daemon start on a fresh host would log "directory not
    // found" because no runtime root has ever been created.
    if let Err(err) = uffs_mft::cache::create_secure_dir(runtime_root) {
        tracing::warn!(
            path = %runtime_root.display(),
            error = %err,
            "runtime-tempfile root creation failed; skipping orphan sweep"
        );
        return;
    }

    match runtime_dir.cleanup_orphans(runtime_root) {
        Ok(removed) => {
            tracing::info!(
                runtime_root = %runtime_root.display(),
                removed,
                "Runtime-tempfile orphan sweep complete"
            );
        }
        Err(err) => {
            tracing::warn!(
                runtime_root = %runtime_root.display(),
                error = %err,
                "Runtime-tempfile orphan sweep failed (continuing startup)"
            );
        }
    }
}

#[cfg(test)]
mod tests;
