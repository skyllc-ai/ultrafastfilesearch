// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Deep sweep for `uffs --uninstall` (task U-70/U-71): use UFFS's own search to
//! find stray family files anywhere on the indexed drives, beyond the known
//! install roots. Strays are versioned and removed only under a **separate,
//! explicit second confirmation** (a `uffs.exe` under `Downloads` might be the
//! user's own copy, so they never ride the main plan's single yes — design §8).
//!
//! The dedup logic is pure + unit-tested against a fake [`Search`]; the live
//! backend ([`DaemonSearch`]) is best-effort (no daemon ⇒ no hits, never a
//! hard failure).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

/// Family-file name patterns the sweep searches for.
const STRAY_PATTERNS: &[&str] = &[
    "uffs.exe",
    "uffsd.exe",
    "uffsmcp.exe",
    "uffs-broker.exe",
    "uffs-update.exe",
    "uffs-mft.exe",
    "uffs-tui*.exe",
    "uffs-gui*.exe",
    "*_compact.uffs",
    "*_usn.cursor",
];

/// A search backend, injected so the dedup logic is testable without a daemon.
pub(crate) trait Search {
    /// Absolute paths matching `pattern` (best-effort; empty on any failure).
    fn find(&mut self, pattern: &str) -> Result<Vec<PathBuf>>;
}

/// A stray family file found outside the known install roots, with its parsed
/// `--version` for binaries (data files like caches carry `None`).
#[derive(Debug, Clone)]
pub(crate) struct StrayHit {
    /// Absolute path of the stray file.
    pub(crate) path: PathBuf,
    /// Parsed `--version`, or `None` for a data file or an unreadable version.
    pub(crate) version: Option<String>,
}

/// Attach a version to each stray: probe `--version` on the executable hits and
/// leave UFFS data files (`*_compact.uffs`, `*_usn.cursor`) unversioned. No
/// daemon needed — each binary is run directly (the same probe the standard
/// detection uses).
pub(crate) fn version_strays(paths: Vec<PathBuf>) -> Vec<StrayHit> {
    paths
        .into_iter()
        .map(|path| {
            let version = is_probeable_binary(&path)
                .then(|| crate::commands::update::binaries::probe_version(&path))
                .flatten();
            StrayHit { path, version }
        })
        .collect()
}

/// Whether `path` names an executable we can run `--version` on, rather than a
/// UFFS data file (`*.uffs` cache / `*.cursor`) that has no version.
fn is_probeable_binary(path: &Path) -> bool {
    !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("uffs") || ext.eq_ignore_ascii_case("cursor"))
}

/// Find stray family files across every pattern, dropping any hit already under
/// a directory the plan handles. Sorted + de-duplicated.
pub(crate) fn find_strays(search: &mut dyn Search, known_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut strays: Vec<PathBuf> = Vec::new();
    for pattern in STRAY_PATTERNS {
        for hit in search.find(pattern)? {
            if !is_under_any(&hit, known_dirs) {
                strays.push(hit);
            }
        }
    }
    strays.sort();
    strays.dedup();
    Ok(strays)
}

/// Whether `path` is `dir` or lives beneath it (case-insensitive, separator
/// aware so `/opt/uffs` does not spuriously match `/opt/uffs-other`).
fn is_under_any(path: &Path, dirs: &[PathBuf]) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    dirs.iter().any(|dir| {
        let base = dir.to_string_lossy().to_ascii_lowercase();
        lower == base
            || lower.starts_with(&format!("{base}/"))
            || lower.starts_with(&format!("{base}\\"))
    })
}

/// Live search backend over the resident daemon. Best-effort: no daemon, or any
/// RPC error, yields no hits rather than failing the uninstall.
pub(crate) struct DaemonSearch;

impl Search for DaemonSearch {
    fn find(&mut self, pattern: &str) -> Result<Vec<PathBuf>> {
        let Ok(mut client) = uffs_client::connect_sync::UffsClientSync::connect_raw() else {
            return Ok(Vec::new());
        };
        let args = vec![
            pattern.to_owned(),
            "--files-only".to_owned(),
            "--limit".to_owned(),
            "1000".to_owned(),
        ];
        let Ok(value) = client.search_cli_raw(&args) else {
            return Ok(Vec::new());
        };
        Ok(extract_paths(&value))
    }
}

/// Pull every `"path"` string out of a search-result JSON value (defensive: the
/// shape varies, so walk it recursively).
fn extract_paths(value: &Value) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_paths(value, &mut out);
    out
}

/// Recursive helper for [`extract_paths`].
fn collect_paths(value: &Value, out: &mut Vec<PathBuf>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(path)) = map.get("path") {
                out.push(PathBuf::from(path));
            }
            for child in map.values() {
                collect_paths(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_paths(item, out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::Result;

    use super::{Search, extract_paths, find_strays, version_strays};

    /// Returns the same hits for every pattern (the dedup must collapse them).
    struct FakeSearch(Vec<PathBuf>);

    impl Search for FakeSearch {
        fn find(&mut self, _pattern: &str) -> Result<Vec<PathBuf>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn hits_under_known_dirs_are_filtered_and_deduped() {
        let mut search = FakeSearch(vec![
            PathBuf::from("/opt/uffs/uffs"),
            PathBuf::from("/home/me/Downloads/uffs.exe"),
        ]);
        let known = [PathBuf::from("/opt/uffs")];
        let strays = find_strays(&mut search, &known).unwrap();
        // The /opt/uffs hit is already planned; only the Downloads stray remains,
        // de-duplicated despite being returned once per pattern.
        assert_eq!(strays.len(), 1);
        assert_eq!(
            strays.first().expect("a stray"),
            &PathBuf::from("/home/me/Downloads/uffs.exe")
        );
    }

    #[test]
    fn sibling_prefix_is_not_treated_as_under() {
        let mut search = FakeSearch(vec![PathBuf::from("/opt/uffs-other/uffs.exe")]);
        let known = [PathBuf::from("/opt/uffs")];
        let strays = find_strays(&mut search, &known).unwrap();
        assert_eq!(strays.len(), 1, "sibling dir must not be filtered");
    }

    #[test]
    fn data_files_are_not_probed_for_a_version() {
        // Cache/cursor data files have no version and must not be executed; a
        // (nonexistent) binary path probes to None rather than panicking.
        let strays = version_strays(vec![
            PathBuf::from("/x/drive_c_compact.uffs"),
            PathBuf::from("/x/journal_usn.cursor"),
            PathBuf::from("/x/definitely-not-here/uffs"),
        ]);
        assert_eq!(strays.len(), 3);
        assert!(
            strays.iter().all(|stray| stray.version.is_none()),
            "data files (and an absent binary) carry no version"
        );
    }

    #[test]
    fn extracts_path_fields_recursively() {
        let value = serde_json::json!({
            "rows": [{ "path": "/a/uffs.exe" }, { "name": "x", "path": "/b/uffsd.exe" }],
        });
        let paths = extract_paths(&value);
        assert_eq!(paths.len(), 2);
    }
}
