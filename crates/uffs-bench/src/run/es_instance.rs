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
/// Reads the permanent `Everything.ini` and copies it line-for-line, replacing
/// the `ntfs_volume_*` parallel arrays with versions filtered to only the
/// `drives` the bench needs (alphabetically sorted, quoted format exactly as
/// Everything writes them — `"C:","D:"`).
fn write_bench_ini(host: &dyn Host, path: &Path, drives: &[char]) -> std::io::Result<()> {
    let permanent_ini = everything_ini_path(host);
    let text = host
        .read_file(&permanent_ini)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();

    let mut bench_drives: Vec<char> = drives.to_vec();
    bench_drives.sort_unstable_by_key(char::to_ascii_uppercase);
    let bench_set: std::collections::HashSet<char> =
        bench_drives.iter().map(char::to_ascii_uppercase).collect();

    // Parse the seven parallel ntfs_volume_* arrays.
    let mut guids = Vec::new();
    let mut paths = Vec::new();
    let mut roots = Vec::new();
    let mut includes = Vec::new();
    let mut load_recent = Vec::new();
    let mut include_onlys = Vec::new();
    let mut monitors = Vec::new();

    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ntfs_volume_guids" => guids = parse_ini_array(val),
            "ntfs_volume_paths" => paths = parse_ini_array(val),
            "ntfs_volume_roots" => roots = parse_ini_array(val),
            "ntfs_volume_includes" => includes = parse_ini_array(val),
            "ntfs_volume_load_recent_changes" => load_recent = parse_ini_array(val),
            "ntfs_volume_include_onlys" => include_onlys = parse_ini_array(val),
            "ntfs_volume_monitors" => monitors = parse_ini_array(val),
            _ => {}
        }
    }

    // Which indices correspond to bench drives?
    let indices: Vec<usize> = paths
        .iter()
        .enumerate()
        .filter(|(_, path_tok)| {
            let letter = path_tok.trim_matches('"').chars().next().unwrap_or(' ');
            bench_set.contains(&letter.to_ascii_uppercase())
        })
        .map(|(i, _)| i)
        .collect();

    let select = |arr: &[String], fallback: &str| -> String {
        indices
            .iter()
            .map(|&i| arr.get(i).map_or(fallback, String::as_str))
            .collect::<Vec<_>>()
            .join(",")
    };

    let out_guids = select(&guids, "\"\"");
    let out_paths = select(&paths, "\"\"");
    let out_roots = select(&roots, "\"\"");
    let out_includes = select(&includes, "1");
    let out_load_recent = select(&load_recent, "1");
    let out_include_onlys = select(&include_onlys, "\"\"");
    let out_monitors = select(&monitors, "1");

    let arrays = VolumeArrays {
        guids: &out_guids,
        paths: &out_paths,
        roots: &out_roots,
        includes: &out_includes,
        load_recent: &out_load_recent,
        include_onlys: &out_include_onlys,
        monitors: &out_monitors,
    };
    let out = rebuild_ini(&text, &arrays);
    host.write_file(path, out.as_bytes())
}

/// Filtered `ntfs_volume_*` array strings for the bench ini.
struct VolumeArrays<'a> {
    /// `ntfs_volume_guids` value.
    guids: &'a str,
    /// `ntfs_volume_paths` value.
    paths: &'a str,
    /// `ntfs_volume_roots` value.
    roots: &'a str,
    /// `ntfs_volume_includes` value.
    includes: &'a str,
    /// `ntfs_volume_load_recent_changes` value.
    load_recent: &'a str,
    /// `ntfs_volume_include_onlys` value.
    include_onlys: &'a str,
    /// `ntfs_volume_monitors` value.
    monitors: &'a str,
}

/// Rebuild the ini text, replacing only the seven `ntfs_volume_*` array lines.
fn rebuild_ini(text: &str, arrays: &VolumeArrays<'_>) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let key = line
            .split_once('=')
            .map_or("", |(key_part, _)| key_part.trim());
        match key {
            "ntfs_volume_guids" => {
                out.push_str("ntfs_volume_guids=");
                out.push_str(arrays.guids);
                out.push('\n');
            }
            "ntfs_volume_paths" => {
                out.push_str("ntfs_volume_paths=");
                out.push_str(arrays.paths);
                out.push('\n');
            }
            "ntfs_volume_roots" => {
                out.push_str("ntfs_volume_roots=");
                out.push_str(arrays.roots);
                out.push('\n');
            }
            "ntfs_volume_includes" => {
                out.push_str("ntfs_volume_includes=");
                out.push_str(arrays.includes);
                out.push('\n');
            }
            "ntfs_volume_load_recent_changes" => {
                out.push_str("ntfs_volume_load_recent_changes=");
                out.push_str(arrays.load_recent);
                out.push('\n');
            }
            "ntfs_volume_include_onlys" => {
                out.push_str("ntfs_volume_include_onlys=");
                out.push_str(arrays.include_onlys);
                out.push('\n');
            }
            "ntfs_volume_monitors" => {
                out.push_str("ntfs_volume_monitors=");
                out.push_str(arrays.monitors);
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
