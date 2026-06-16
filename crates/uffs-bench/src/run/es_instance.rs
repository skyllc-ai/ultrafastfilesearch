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
use crate::run::specs::everything_ini_path;

/// Named instance used for the bench-local Everything process.
pub(super) const INSTANCE_NAME: &str = "uffs-bench";

/// Maximum poll attempts waiting for the bench ES instance to finish indexing.
///
/// 60 attempts × 5 s = 5 minutes maximum.
const LOAD_POLL_ATTEMPTS: u32 = 60;

/// Milliseconds between bench-instance readiness polls.
const LOAD_POLL_INTERVAL_MS: u64 = 5_000;

/// Milliseconds to wait after sending `-exit` to any existing Everything
/// instances before spawning our own, giving the process time to flush its db.
const ES_KILL_GRACE_MS: u64 = 3_000;

/// Milliseconds to wait after spawning Everything.exe before the first IPC
/// poll — gives the process time to register its IPC window.
const ES_STARTUP_GRACE_MS: u64 = 5_000;

/// Parse a `key=val1,val2,...` Everything.ini array value into tokens.
///
/// Handles the quoted-string format Everything uses: `"C:","D:"` for paths
/// and bare integers `1,1,0` for flags.  Quoted tokens are kept whole.
fn parse_ini_array(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        if rest.starts_with('"') {
            // find the closing quote (starts scanning after the opening quote)
            let close = rest.char_indices().skip(1).find(|(_, ch)| *ch == '"');
            let end = close.map_or(rest.len(), |(idx, _)| idx + 1);
            let (tok, tail) = rest.split_at(end);
            tokens.push(tok.to_owned());
            rest = tail.trim_start_matches(',');
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let (tok, tail) = rest.split_at(end);
            tokens.push(tok.to_owned());
            rest = tail.trim_start_matches(',');
        }
    }
    tokens
}

/// Write the bench `Everything.ini` into `path`.
///
/// Reads the permanent `Everything.ini` verbatim and produces the temp ini
/// with two changes:
///   1. `ntfs_volume_includes` and `ntfs_volume_monitors` are recomputed: `1`
///      for each drive in `drives`, `0` for all other drives in the permanent
///      ini.  All other volume arrays (guids/paths/roots/
///      `load_recent_changes`/`include_onlys`) are kept as-is so the temp ini
///      the same full set of drives as the permanent ini.
///   2. `auto_include_fixed_volumes` (and siblings) are forced to `0` so ES
///      does not auto-discover drives outside the configured set.
fn write_bench_ini(host: &dyn Host, path: &Path, drives: &[char]) -> std::io::Result<()> {
    let permanent_ini = everything_ini_path(host);
    let text = host
        .read_file(&permanent_ini)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();

    let bench_set: std::collections::HashSet<char> =
        drives.iter().map(char::to_ascii_uppercase).collect();

    // Parse ntfs_volume_paths to know which index maps to which drive letter.
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some((key, val)) = line.split_once('=')
            && key.trim() == "ntfs_volume_paths"
        {
            paths = parse_ini_array(val);
            break;
        }
    }

    // Build includes/monitors: 1 for bench drives, 0 for all others,
    // preserving the full positional array length from the permanent ini.
    let includes: String = paths
        .iter()
        .map(|tok| {
            let letter = tok.trim_matches('"').chars().next().unwrap_or(' ');
            if bench_set.contains(&letter.to_ascii_uppercase()) {
                "1"
            } else {
                "0"
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let monitors = includes.clone();

    let arrays = VolumeArrays {
        includes: &includes,
        monitors: &monitors,
    };
    let out = rebuild_ini(&text, &arrays);
    host.write_file(path, out.as_bytes())
}

/// Recomputed per-drive bit arrays for the bench ini.
struct VolumeArrays<'a> {
    /// `ntfs_volume_includes` value — `1` for bench drives, `0` for others.
    includes: &'a str,
    /// `ntfs_volume_monitors` value — mirrors `includes`.
    monitors: &'a str,
}

/// Rebuild the ini text from `text`, replacing only `ntfs_volume_includes`,
/// `ntfs_volume_monitors`, and the `auto_include_*`/`auto_remove_*` keys.
/// All other lines are copied verbatim.
fn rebuild_ini(text: &str, arrays: &VolumeArrays<'_>) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let key = line
            .split_once('=')
            .map_or("", |(key_part, _)| key_part.trim());
        match key {
            "ntfs_volume_includes" => {
                out.push_str("ntfs_volume_includes=");
                out.push_str(arrays.includes);
                out.push('\n');
            }
            "ntfs_volume_monitors" => {
                out.push_str("ntfs_volume_monitors=");
                out.push_str(arrays.monitors);
                out.push('\n');
            }
            // Force to 0 — without this ES ignores ntfs_volume_paths and
            // auto-discovers every fixed NTFS drive on the machine.
            "auto_include_fixed_volumes"
            | "auto_include_removable_volumes"
            | "auto_include_fixed_refs_volumes"
            | "auto_include_removable_refs_volumes"
            | "auto_remove_offline_ntfs_volumes"
            | "auto_remove_moved_ntfs_volumes"
            | "auto_remove_offline_refs_volumes"
            | "auto_remove_moved_refs_volumes" => {
                out.push_str(key);
                out.push('=');
                out.push('0');
                out.push('\n');
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
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
    // Terminate any existing Everything instances (default + stale bench) so
    // we start from a clean slate with our own temp ini.  A short grace period
    // lets the process finish writing its db before we spawn the new instance.
    kill_existing_instances(host, everything_exe);
    host.sleep_ms(ES_KILL_GRACE_MS);
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
    host.out(&format!(
        "[es-instance] spawn: {} {}",
        everything_exe,
        args.join(" ")
    ));
    if let Err(err) = host.spawn(everything_exe, &args) {
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
    host.sleep_ms(ES_STARTUP_GRACE_MS);
    for attempt in 1..=LOAD_POLL_ATTEMPTS {
        let counts: Vec<(char, u64)> = drives
            .iter()
            .map(|&letter| {
                let search = format!("{letter}:");
                let count = host
                    .run(es_exe, &[
                        "-instance",
                        INSTANCE_NAME,
                        search.as_str(),
                        "-get-result-count",
                    ])
                    .ok()
                    .and_then(|out| out.stdout.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                (letter, count)
            })
            .collect();
        let all_loaded = counts.iter().all(|(_, n)| *n > 0);
        if all_loaded {
            host.out("[es-instance] Everything index loaded — proceeding");
            return true;
        }
        let counts_str = counts
            .iter()
            .map(|(ch, n)| format!("{ch}:{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        host.out(&format!(
            "[es-instance] waiting for Everything to finish indexing … \
             (attempt {attempt}/{LOAD_POLL_ATTEMPTS}) [{counts_str}]"
        ));
        host.sleep_ms(LOAD_POLL_INTERVAL_MS);
    }
    host.out(
        "[es-instance] WARNING: Everything did not finish indexing within 5 minutes \
         — ES cells will be measured with a partial index",
    );
    false
}

/// Ask any running Everything instances to exit before we launch our own.
///
/// Sends `-exit` to both the default (unnamed) instance and any stale
/// `uffs-bench` instance left over from a previous interrupted run.
/// Errors are non-fatal: if no instance is running the IPC call simply fails
/// silently, which is the desired outcome.
fn kill_existing_instances(host: &dyn Host, everything_exe: &str) {
    drop(host.run(everything_exe, &["-exit"]));
    drop(host.run(everything_exe, &["-instance", INSTANCE_NAME, "-exit"]));
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

/// Whether the bench should launch its own isolated Everything instance.
///
/// Returns `true` for every ES status except `NotInstalled` (where
/// `Everything.exe` is not present and cannot be launched).  This means the
/// bench always replaces whatever instance the operator may have running — even
/// a fully-loaded default instance — with a private one restricted to the
/// RAM-budget-capable drives and a clean temp ini.
pub(super) fn es_needs_launch(
    preflight: &crate::preflight::PreflightResult,
    drives: &[char],
) -> bool {
    drives.iter().any(|&letter| {
        preflight.drives.iter().any(|dp| {
            dp.drive == letter && !matches!(dp.es_status, crate::preflight::EsStatus::NotInstalled)
        })
    })
}
