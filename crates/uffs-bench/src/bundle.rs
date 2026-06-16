// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bundle directory creation and tool-path resolution.
//!
//! A *bundle* is the timestamped output directory that holds a run's
//! `state.json`, fingerprints, and result artifacts. [`new_bundle`] mints one;
//! [`resolve_tool`] decides which executable to invoke for a given tool by a
//! fixed precedence (explicit override > home-relative > bare name on `PATH`).

use std::path::{Path, PathBuf};

use crate::error::{BenchError, Result};
use crate::host::Host;

/// Compute the bundle directory path for *now* without creating it.
///
/// The name is `bench-<UTC timestamp>-v<version>`, so concurrent or repeated
/// runs never collide and the bundle self-documents its suite version. Split
/// out from [`new_bundle`] so a dry-run can display the would-be path while
/// creating nothing.
#[must_use]
pub fn bundle_path(host: &dyn Host, root: &Path, version: &str) -> PathBuf {
    let stamp = host.now().format("%Y%m%dT%H%M%SZ").to_string();
    root.join(format!("bench-{stamp}-v{version}"))
}

/// Create a fresh, timestamped bundle directory under `root`.
///
/// Names it via [`bundle_path`] (the single source of the naming convention)
/// then creates it.
///
/// # Errors
/// Returns an error if the directory cannot be created.
pub fn new_bundle(host: &dyn Host, root: &Path, version: &str) -> Result<PathBuf> {
    let dir = bundle_path(host, root, version);
    host.create_dir_all(&dir)
        .map_err(|err| BenchError::io(&dir, err))?;
    Ok(dir)
}

/// Where a resolved tool's path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// An explicit operator-supplied path/command.
    Explicit,
    /// Resolved relative to a configured home directory.
    Home,
    /// A bare executable name to be found on `PATH`.
    Path,
}

/// A resolved tool invocation target and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// The command/path to invoke.
    pub command: String,
    /// How the command was resolved.
    pub source: ToolSource,
}

/// Resolve which command to run for a tool by fixed precedence.
///
/// 1. `explicit` — an operator override wins outright.
/// 2. `home` — if non-empty, the tool is taken as `<home>/<path_name>`.
/// 3. otherwise `path_name` is returned bare, to be found on `PATH`.
#[must_use]
pub fn resolve_tool(explicit: Option<&str>, home: &str, path_name: &str) -> ResolvedTool {
    match explicit {
        Some(command) => ResolvedTool {
            command: command.to_owned(),
            source: ToolSource::Explicit,
        },
        None if home.is_empty() => ResolvedTool {
            command: path_name.to_owned(),
            source: ToolSource::Path,
        },
        None => {
            let path = Path::new(home).join(path_name);
            ResolvedTool {
                command: path.display().to_string(),
                source: ToolSource::Home,
            }
        }
    }
}
