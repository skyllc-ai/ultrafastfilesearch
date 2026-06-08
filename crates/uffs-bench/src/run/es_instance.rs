// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Isolated Everything.exe instance for the bench suite.
//!
//! When `es.exe` reports that Everything is not running, the bench tool cannot
//! simply ask the operator to start it — that would index all drives on the
//! machine. Instead it launches a **sandboxed instance** via
//! `Everything.exe -config <tempini> -instance uffs-bench -startup`, where
//! `<tempini>` contains only the RAM-budget-capable drives identified by the
//! preflight. The permanent `Everything.ini` is never touched.
//!
//! Lifecycle:
//! 1. [`launch`] — write the temp ini and start the instance.
//! 2. [`wait_until_loaded`] — poll `es.exe -instance uffs-bench` until all
//!    bench drives report a non-zero result count.
//! 3. [`stop`] — send `Everything.exe -instance uffs-bench -exit` to shut it
//!    down cleanly.

use std::path::{Path, PathBuf};

use crate::host::Host;

/// Named instance used for the bench-local Everything process.
pub(super) const INSTANCE_NAME: &str = "uffs-bench";

/// Maximum poll attempts waiting for the bench ES instance to finish indexing.
///
/// 60 attempts × 5 s = 5 minutes maximum.
const LOAD_POLL_ATTEMPTS: u32 = 60;

/// Milliseconds between bench-instance readiness polls.
const LOAD_POLL_INTERVAL_MS: u64 = 5_000;

/// Write a minimal `Everything.ini` restricted to `drives` into `path`.
///
/// Must explicitly disable `auto_include_fixed_volumes` — without it
/// Everything ignores `ntfs_volume_paths` and indexes every fixed NTFS volume
/// on the machine automatically (confirmed via voidtools forum, author `void`).
///
/// The full set of keys required to pin indexing to exactly `drives`:
/// - `auto_include_fixed_volumes=0` — don't auto-discover fixed drives
/// - `auto_include_removable_volumes=0` — don't auto-discover removable drives
/// - `auto_remove_offline_ntfs_volumes=0` — don't remove offline volumes
/// - `ntfs_volume_paths=C:,D:,…` — exactly the bench drives to index
/// - remaining `ntfs_volume_*` keys explicitly blank so ES doesn't inherit
///   stale values from a partially-written config
fn write_bench_ini(host: &dyn Host, path: &Path, drives: &[char]) -> std::io::Result<()> {
    // Everything.ini uses a quoted comma-separated format for parallel arrays,
    // e.g. ntfs_volume_paths="C:","D:","G:"
    // ntfs_volume_includes / monitors / roots follow the same positional order.
    let n = drives.len();
    let volume_paths: String = drives
        .iter()
        .map(|letter| format!("\"{letter}:\""))
        .collect::<Vec<_>>()
        .join(",");
    let ones = vec!["1"; n].join(",");
    let empty_quoted = vec!["\"\""; n].join(",");
    let ini = format!(
        "[Everything]\n\
         auto_include_fixed_volumes=0\n\
         auto_include_removable_volumes=0\n\
         auto_remove_offline_ntfs_volumes=0\n\
         ntfs_volume_guids=\n\
         ntfs_volume_paths={volume_paths}\n\
         ntfs_volume_roots={empty_quoted}\n\
         ntfs_volume_includes={ones}\n\
         ntfs_volume_load_recent_changes={ones}\n\
         ntfs_volume_include_onlys={empty_quoted}\n\
         ntfs_volume_monitors={ones}\n"
    );
    host.write_file(path, ini.as_bytes())
}

/// Derive a path for the temporary bench ini.
///
/// Prefers `%TEMP%` on Windows; falls back to `bundle_dir` so it is always
/// inside a location we already own.
fn bench_ini_path(host: &dyn Host, bundle_dir: &Path) -> PathBuf {
    host.env("TEMP").or_else(|| host.env("TMPDIR")).map_or_else(
        || bundle_dir.join("uffs-bench-everything.ini"),
        |tmp| PathBuf::from(tmp).join("uffs-bench-everything.ini"),
    )
}

/// Launch an isolated Everything instance that indexes only `drives`.
///
/// Writes a temp ini and spawns `Everything.exe -config <ini>
/// -instance uffs-bench [-admin] -startup`.  Returns the ini path so the
/// caller can remove it after [`stop`].
///
/// Pass `admin = true` to add `-admin` (run Everything elevated).
///
/// Does nothing and returns `None` when `everything_exe` resolves to the
/// GUI-less `es.exe` stub (non-Windows hosts where Everything cannot run).
pub(super) fn launch(
    host: &dyn Host,
    everything_exe: &str,
    drives: &[char],
    bundle_dir: &Path,
    admin: bool,
) -> Option<PathBuf> {
    if drives.is_empty() {
        return None;
    }
    let ini_path = bench_ini_path(host, bundle_dir);
    if let Err(err) = write_bench_ini(host, &ini_path, drives) {
        host.out(&format!(
            "[es-instance] WARNING: could not write temp ini — {err}"
        ));
        return None;
    }
    let admin_tag = if admin { " (admin)" } else { "" };
    host.out(&format!(
        "[es-instance] launching Everything{admin_tag} (drives: {}) …",
        drives
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let ini_str = ini_path.to_string_lossy();
    let mut args: Vec<&str> = vec!["-config", ini_str.as_ref(), "-instance", INSTANCE_NAME];
    if admin {
        args.push("-admin");
    }
    args.push("-startup");
    if let Err(err) = host.run(everything_exe, &args) {
        host.out(&format!(
            "[es-instance] WARNING: could not launch Everything — {err}"
        ));
        return None;
    }
    Some(ini_path)
}

/// Poll `es.exe -instance uffs-bench` until every drive in `drives` reports a
/// non-zero result count, or the poll budget is exhausted.
///
/// Returns `true` when all drives are loaded within the budget.
pub(super) fn wait_until_loaded(host: &dyn Host, es_exe: &str, drives: &[char]) -> bool {
    for attempt in 1..=LOAD_POLL_ATTEMPTS {
        let all_loaded = drives.iter().all(|&letter| {
            let search = format!("{letter}:");
            host.run(es_exe, &[
                "-instance",
                INSTANCE_NAME,
                search.as_str(),
                "-get-result-count",
            ])
            .ok()
            .and_then(|out| out.stdout.trim().parse::<u64>().ok())
            .unwrap_or(0)
                > 0
        });
        if all_loaded {
            host.out("[es-instance] Everything index loaded — proceeding");
            return true;
        }
        host.out(&format!(
            "[es-instance] waiting for Everything to finish indexing … \
             (attempt {attempt}/{LOAD_POLL_ATTEMPTS})"
        ));
        host.sleep_ms(LOAD_POLL_INTERVAL_MS);
    }
    host.out(
        "[es-instance] WARNING: Everything did not finish indexing within 5 minutes \
         — ES cells will be measured with a partial index",
    );
    false
}

/// Send `Everything.exe -instance uffs-bench -exit` and remove the temp ini.
pub(super) fn stop(host: &dyn Host, everything_exe: &str, ini_path: Option<&Path>) {
    if let Err(err) = host.run(everything_exe, &["-instance", INSTANCE_NAME, "-exit"]) {
        host.out(&format!(
            "[es-instance] WARNING: could not stop Everything instance — {err}"
        ));
    }
    if let Some(path) = ini_path {
        host.remove_file(path).unwrap_or_else(|err| {
            host.out(&format!(
                "[es-instance] WARNING: could not remove temp ini — {err}"
            ));
        });
    }
}

/// Whether the preflight result shows Everything is not running on any of the
/// given drives (i.e. we need to launch our own instance).
pub(super) fn es_needs_launch(
    preflight: &crate::preflight::PreflightResult,
    drives: &[char],
) -> bool {
    drives.iter().any(|&letter| {
        preflight.drives.iter().any(|dp| {
            dp.drive == letter
                && matches!(
                    dp.es_status,
                    crate::preflight::EsStatus::DaemonNotRunning
                        | crate::preflight::EsStatus::NotConfigured
                )
        })
    })
}
