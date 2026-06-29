// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Artifact inventory for `uffs --uninstall` (task U-11): resolve every
//! non-binary trace the UFFS family leaves — the data, cache, legacy-cache, and
//! config dirs, plus the broker service — with presence, recursive size, and
//! (later) elevation requirement. Read-only: it stats paths and queries the
//! service, never mutates anything.

use std::path::{Path, PathBuf};

/// The pre-migration legacy cache dir name (mirrors the private constant in
/// `uffs_mft::cache`; kept in sync intentionally so the analysis can offer to
/// remove a stale legacy cache).
const LEGACY_CACHE_DIR_NAME: &str = "uffs_index_cache";

/// Kind of inventoried artifact (for grouping + rendering).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    /// Lifecycle / runtime data dir (`%LOCALAPPDATA%\uffs\`): the daemon pid +
    /// state and the update working dir (snapshots, journal, backups).
    Data,
    /// Encrypted cache dir (`%LOCALAPPDATA%\uffs\cache\`): per-drive compact
    /// indexes, USN cursors, runtime.
    Cache,
    /// Pre-migration legacy cache dir (`%TEMP%\uffs_index_cache\`).
    LegacyCache,
    /// Per-user config / settings dir.
    Config,
}

impl ArtifactKind {
    /// Short human label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Cache => "cache",
            Self::LegacyCache => "legacy-cache",
            Self::Config => "config",
        }
    }
}

/// One inventoried filesystem artifact (a directory tree).
#[derive(Debug, Clone)]
pub(crate) struct ArtifactDir {
    /// What kind of artifact this is.
    pub(crate) kind: ArtifactKind,
    /// The directory path.
    pub(crate) path: PathBuf,
    /// Whether it currently exists on disk.
    pub(crate) exists: bool,
    /// Recursive size in bytes (0 when absent or unreadable).
    pub(crate) size_bytes: u64,
}

/// State of the Windows broker service (`UffsAccessBroker`). Off Windows the
/// service concept does not exist, so it always reads `Absent` there (via the
/// cross-platform `uffs_winsvc` stub), which is correct for removal purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerServiceState {
    /// Installed (running or stopped). Removal needs elevation.
    Installed,
    /// Not installed (or non-Windows).
    Absent,
}

impl BrokerServiceState {
    /// Short human label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Absent => "absent",
        }
    }
}

/// The full non-binary inventory.
#[derive(Debug, Clone)]
pub(crate) struct Inventory {
    /// The artifact directories (present or not).
    pub(crate) dirs: Vec<ArtifactDir>,
    /// Broker-service state.
    pub(crate) broker_service: BrokerServiceState,
}

/// Resolve the inventory: stat the known artifact dirs and query the broker
/// service. Read-only.
pub(crate) fn collect() -> Inventory {
    let mut dirs = vec![
        stat_dir(
            ArtifactKind::Data,
            crate::commands::update::procinfo::lifecycle_dir(),
        ),
        stat_dir(ArtifactKind::Cache, uffs_mft::cache::secure_cache_dir()),
        stat_dir(
            ArtifactKind::LegacyCache,
            std::env::temp_dir().join(LEGACY_CACHE_DIR_NAME),
        ),
    ];
    // On some platforms (e.g. macOS) the config base equals the data base, so
    // skip the config entry when it would duplicate a dir already listed —
    // removal must never act on the same path twice.
    if let Some(config) = config_dir()
        && !dirs.iter().any(|existing| existing.path == config)
    {
        dirs.push(stat_dir(ArtifactKind::Config, config));
    }
    Inventory {
        dirs,
        broker_service: broker_service_state(),
    }
}

/// The per-user UFFS config / settings dir, if a config base is resolvable.
fn config_dir() -> Option<PathBuf> {
    dirs_next::config_dir().map(|base| base.join("uffs"))
}

/// Stat a directory: existence + recursive size.
fn stat_dir(kind: ArtifactKind, path: PathBuf) -> ArtifactDir {
    let exists = path.is_dir();
    let size_bytes = if exists { dir_size_bytes(&path) } else { 0 };
    ArtifactDir {
        kind,
        path,
        exists,
        size_bytes,
    }
}

/// Recursive byte size of `dir` (best-effort; unreadable entries count as 0).
/// `DirEntry::metadata` does not traverse symlinks, so this cannot loop.
fn dir_size_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total = total.saturating_add(dir_size_bytes(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Query the broker-service state. `uffs_winsvc::is_installed` stubs to `false`
/// off Windows, so this reads `Absent` there.
fn broker_service_state() -> BrokerServiceState {
    if uffs_winsvc::is_installed(uffs_broker_protocol::SERVICE_NAME) {
        BrokerServiceState::Installed
    } else {
        BrokerServiceState::Absent
    }
}
