// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 0c — competitor (Everything) preflight, **fully read-only**.
//!
//! Answers "what is Everything actually holding?" without ever writing
//! `Everything.ini`. [`capture`] parses the configured volumes (read-only),
//! observes each candidate drive's live record count via `es.exe`, and
//! estimates per-pattern feasibility against Everything's IPC ceiling. Every
//! side effect flows through the [`Host`] seam, so a `MockHost` test can prove
//! the stage performs **zero** `write_file`/`remove_file` operations.
//!
//! The read-only helpers (`parse_drives_from_ini`, the result-count poll) are
//! lifted from `scripts/windows/everything_capacity_probe.rs`; the destructive
//! `isolate_drive_in_ini`/`fs::write` paths are deliberately **not** carried
//! over.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BenchError, Result};
use crate::host::Host;

/// Why `es.exe` could not serve results for a drive.
///
/// Distinguishing these states lets the Stage 0 plan gate surface concrete
/// operator instructions rather than a generic "not loaded" message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EsStatus {
    /// `es.exe` binary not found: Everything (engine + CLI) is not installed.
    NotInstalled,
    /// `es.exe` found but reports IPC error 8 and `Everything.exe` is not
    /// in the process list — the daemon is installed but not started.
    DaemonNotRunning,
    /// `es.exe` found, IPC error 8 detected, but `Everything.exe` is already
    /// in the process list — started but IPC not yet ready (still loading).
    DaemonStarting,
    /// Everything is running and the drive is in `ntfs_volume_paths`, but the
    /// index is still being built (result-count poll returned 0).
    StillIndexing,
    /// Everything is running and has a non-zero result count for this drive.
    Loaded,
    /// This drive is not in Everything's `ntfs_volume_paths` at all.
    NotConfigured,
}

/// Everything's practical IPC row ceiling for a single cross-tool cell.
///
/// Everything serves results over IPC (`-export-csv` tops out near ~2 GB);
/// beyond roughly this many rows a head-to-head cell becomes infeasible, so it
/// is flagged and run UFFS-only. Lifted from the capacity probe's findings.
pub const ES_IPC_ROW_CEILING: u64 = 150_000;

/// Approximate bytes Everything's in-process index consumes per indexed record.
///
/// Source: voidtools forum ("~100 MB per 1 M files").
/// Used to estimate how much RAM Everything needs to index a drive.
pub const UFFS_BYTES_PER_RECORD: u64 = 100;

/// Maximum RAM Everything can use for its in-process index before it OOMs.
///
/// Re-measured 2026-06-11: Everything indexes C+D+E+G together —
/// 11,006,944 objects ≈ 1.025 GiB at [`UFFS_BYTES_PER_RECORD`] — without
/// crashing.  The previous 1 GiB cap (set when C+D+E was the tested ceiling)
/// excluded the fourth drive, so a head-to-head over the full C/D/E/G set fell
/// back to UFFS-only.  Bumped to 1.25 GiB so all four fit with ~22% headroom
/// for normal file-count growth, while still capping a runaway drive set.
pub const ES_RAM_BUDGET_BYTES: u64 = 1_342_177_280; // 1.25 GiB

/// A pattern whose per-drive result size is estimated via UFFS.
///
/// `args` are passed to the UFFS binary; the literal token `{DRIVE}` is
/// replaced with the drive letter before invocation. The command must print a
/// single integer row count on stdout.
#[derive(Debug, Clone)]
pub struct PatternProbe {
    /// Display name of the pattern (for example `"all_dlls"`).
    pub name: String,
    /// UFFS argument template (with `{DRIVE}` placeholders).
    pub args: Vec<String>,
}

/// Inputs that scope a competitor preflight.
#[derive(Debug, Clone, Default)]
pub struct PreflightSpec {
    /// Path to `Everything.ini` (read-only).
    pub ini_path: PathBuf,
    /// Drives the operator asked about, in display order.
    pub candidate_drives: Vec<char>,
    /// The `es.exe` command to invoke.
    pub es_exe: String,
    /// The UFFS command used for row estimation.
    pub uffs_exe: String,
    /// Patterns to estimate feasibility for.
    pub patterns: Vec<PatternProbe>,
    /// How many times to poll a *configured* drive that reports zero.
    pub poll_attempts: u32,
    /// Delay between readiness-poll attempts, in milliseconds.
    pub poll_interval_ms: u64,
    /// Maximum bytes Everything may use for its in-process index.
    ///
    /// Drives are added greedily (smallest first) until this budget is
    /// exhausted; any drive that would overflow it is excluded from the
    /// cross-tool capable set.  Defaults to 0 (no cap = any drive is capable
    /// regardless of size) when not set by the caller.
    pub es_ram_budget_bytes: u64,
    /// When non-empty, all `es.exe` calls append `-instance <name>` so they
    /// target an isolated Everything instance rather than the default one.
    ///
    /// Set after the bench suite launches its own Everything instance via
    /// `Everything.exe -config <tempini> -instance <name> -startup`.
    pub es_instance_name: String,
}

/// Per-drive competitor state observed during preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrivePreflight {
    /// Drive letter (uppercase).
    pub drive: char,
    /// Whether the drive is listed in `Everything.ini`'s configured volumes.
    pub configured: bool,
    /// Whether Everything currently serves a non-zero result count for it.
    pub loaded: bool,
    /// Whether the index is hot (in RAM, serving immediately). For Everything
    /// this equals `loaded`; kept distinct for forward compatibility.
    pub hot: bool,
    /// Live record count reported by `es.exe` (`0` when not loaded).
    pub record_count: u64,
    /// Total file+dir count from UFFS (always populated, ES-independent).
    ///
    /// Used to estimate how much RAM Everything would need to index this drive
    /// (`uffs_record_count × UFFS_BYTES_PER_RECORD`).  `0` when UFFS failed.
    pub uffs_record_count: u64,
    /// Fine-grained reason why `es.exe` could not serve this drive (if any).
    pub es_status: EsStatus,
}

/// Per-(drive, pattern) feasibility against the IPC ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellFeasibility {
    /// Drive letter the estimate is for.
    pub drive: char,
    /// Pattern name the estimate is for.
    pub pattern: String,
    /// Estimated result rows (from UFFS).
    pub est_rows: u64,
    /// Whether Everything can feasibly serve this cell (`est_rows <= ceiling`).
    pub es_feasible: bool,
}

/// The full competitor preflight, serialized to `competitor-preflight.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightResult {
    /// Per-drive observed state, in candidate order.
    pub drives: Vec<DrivePreflight>,
    /// Per-(loaded-drive, pattern) feasibility cells.
    pub cells: Vec<CellFeasibility>,
}

/// Parse `ntfs_volume_paths` from `Everything.ini` into uppercase drive
/// letters.
#[must_use]
pub fn parse_drives_from_ini(ini: &str) -> Vec<char> {
    for line in ini.lines() {
        if let Some(rest) = line.strip_prefix("ntfs_volume_paths=") {
            return rest
                .split(',')
                .filter_map(|entry| {
                    entry
                        .trim()
                        .trim_matches('"')
                        .chars()
                        .next()
                        .filter(char::is_ascii_alphabetic)
                })
                .map(|letter| letter.to_ascii_uppercase())
                .collect();
        }
    }
    Vec::new()
}

/// Check whether `es.exe` is reachable and the Everything daemon is running.
///
/// Returns `None` when the binary executes but no IPC error is detected (daemon
/// is running). Returns `Some(EsStatus)` with a fine-grained status otherwise:
/// - `NotInstalled` — binary could not be spawned at all.
/// - `DaemonStarting` — IPC error but `Everything.exe` is in the process list
///   (started but not yet ready).
/// - `DaemonNotRunning` — IPC error and `Everything.exe` is not running at all.
fn check_es_available(host: &dyn Host, es_exe: &str, instance: &str) -> Option<EsStatus> {
    let args: &[&str] = if instance.is_empty() {
        &["-get-everything-version"]
    } else {
        &["-instance", instance, "-get-everything-version"]
    };
    match host.run(es_exe, args) {
        Err(_) => Some(EsStatus::NotInstalled),
        Ok(out) => {
            let combined = format!("{} {}", out.stdout, out.stderr);
            (combined.contains("Error 8") || combined.contains("IPC window not found")).then(|| {
                if is_everything_process_running(host) {
                    EsStatus::DaemonStarting
                } else {
                    EsStatus::DaemonNotRunning
                }
            })
        }
    }
}

/// Return `true` when `Everything.exe` appears in the running process list.
///
/// Uses `tasklist` on Windows (always available) and `pgrep` on Unix.
/// Returns `false` on any execution failure — conservative default.
fn is_everything_process_running(host: &dyn Host) -> bool {
    #[cfg(windows)]
    {
        host.run("tasklist", &[
            "/FI",
            "IMAGENAME eq Everything.exe",
            "/NH",
            "/FO",
            "CSV",
        ])
        .is_ok_and(|out| out.stdout.contains("Everything.exe"))
    }
    #[cfg(not(windows))]
    {
        host.run("pgrep", &["-x", "Everything"])
            .is_ok_and(|out| !out.stdout.trim().is_empty())
    }
}

/// Poll `es.exe <drive>: -get-result-count`, returning the first non-zero
/// count within `attempts` (sleeping `interval_ms` between tries), else `0`.
///
/// Uses `"C:"` (no trailing backslash) as the drive scope, which matches
/// `everything_capacity_probe.rs`'s L1+ convention.
fn poll_result_count(
    host: &dyn Host,
    es_exe: &str,
    instance: &str,
    drive: char,
    attempts: u32,
    interval_ms: u64,
) -> u64 {
    let search = format!("{drive}:");
    for attempt in 0..attempts {
        if attempt > 0 {
            host.sleep_ms(interval_ms);
        }
        let args: &[&str] = if instance.is_empty() {
            &[search.as_str(), "-get-result-count"]
        } else {
            &["-instance", instance, search.as_str(), "-get-result-count"]
        };
        let count = host
            .run(es_exe, args)
            .ok()
            .and_then(|out| out.stdout.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if count > 0 {
            return count;
        }
    }
    0
}

/// Parse the set of drive letters UFFS knows about from `uffs daemon status`
/// stdout, regardless of tier or record count.
///
/// Parked drives appear without a record count (`[Parked]  C:`) so
/// [`parse_daemon_status_drives`] does not capture them.  This function
/// captures every drive letter that appears on a bracketed-tier line so
/// callers can distinguish "UFFS knows this drive but it is parked" from
/// "UFFS has never heard of this drive".
#[must_use]
pub(crate) fn parse_daemon_known_drives(status: &str) -> alloc::collections::BTreeSet<char> {
    let mut set = alloc::collections::BTreeSet::new();
    for line in status.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('[') {
            continue;
        }
        // Lines look like:  [Warm]   C: — …  or  [Parked]  D:
        // The drive letter is the first alphabetic token that ends with ':'.
        if let Some(letter) = trimmed.split_whitespace().find_map(|token| {
            let ch = token.trim_end_matches(':').chars().next()?;
            (ch.is_ascii_alphabetic() && token.ends_with(':')).then(|| ch.to_ascii_uppercase())
        }) {
            set.insert(letter);
        }
    }
    set
}

/// Parse per-drive record counts from `uffs daemon status` stdout.
///
/// Each loaded drive appears as a line matching:
/// ```text
///   [Warm]   C: —  3,409,074 records (live) — …
/// ```
/// The drive letter is the first ASCII-alphabetic character after the tier
/// tag, and the record count is the comma-separated integer before the word
/// `records`. Returns a map of uppercase drive letter → record count.
/// Drives not present in the output (e.g. still loading) are absent from
/// the map; callers treat a missing entry as count = 0.
#[must_use]
pub(crate) fn parse_daemon_status_drives(status: &str) -> alloc::collections::BTreeMap<char, u64> {
    let mut map = alloc::collections::BTreeMap::new();
    for line in status.lines() {
        let trimmed = line.trim_start();
        // Lines look like:  [Warm]   C: —  3,409,074 records (live)
        // Skip header lines and any line that doesn't contain "records".
        if !trimmed.contains("records") {
            continue;
        }
        // Find the first alpha char after the tier tag (e.g. 'C' in 'C:').
        let drive = trimmed.split_whitespace().find_map(|token| {
            let ch = token.trim_end_matches(':').chars().next()?;
            ch.is_ascii_alphabetic().then(|| ch.to_ascii_uppercase())
        });
        // Find the comma-separated integer immediately before "records".
        let count = trimmed
            .split_whitespace()
            .zip(trimmed.split_whitespace().skip(1))
            .find_map(|(token, next)| {
                (next == "records").then(|| token.replace(',', "").parse::<u64>().ok())?
            });
        if let (Some(letter), Some(records)) = (drive, count) {
            map.insert(letter, records);
        }
    }
    map
}

/// Probe one drive's ES state.
///
/// `uffs_record_count` is sourced from the already-parsed `uffs daemon status`
/// output (passed in by the caller) — no additional UFFS IPC call is made.
/// Returns early with zeroed ES fields if the daemon is unavailable.
/// Otherwise polls `es.exe -get-result-count` (with backoff for configured
/// drives, once for unconfigured drives).
fn probe_drive(
    host: &dyn Host,
    spec: &PreflightSpec,
    drive: char,
    configured: bool,
    es_available: Option<&EsStatus>,
    uffs_record_count: u64,
) -> DrivePreflight {
    if let Some(status) = es_available {
        return DrivePreflight {
            drive,
            configured,
            loaded: false,
            hot: false,
            record_count: 0,
            uffs_record_count,
            es_status: status.clone(),
        };
    }
    let attempts = if configured {
        spec.poll_attempts.max(1)
    } else {
        1
    };
    let record_count = poll_result_count(
        host,
        &spec.es_exe,
        &spec.es_instance_name,
        drive,
        attempts,
        spec.poll_interval_ms,
    );
    let loaded = record_count > 0;
    let es_status = if loaded {
        EsStatus::Loaded
    } else if configured {
        EsStatus::StillIndexing
    } else {
        EsStatus::NotConfigured
    };
    DrivePreflight {
        drive,
        configured,
        loaded,
        hot: loaded,
        record_count,
        uffs_record_count,
        es_status,
    }
}

/// Estimate a (drive, pattern) result-set size via UFFS (`0` on failure).
fn estimate_rows(host: &dyn Host, uffs_exe: &str, drive: char, pattern: &PatternProbe) -> u64 {
    let drive_token = drive.to_string();
    let args: Vec<String> = pattern
        .args
        .iter()
        .map(|arg| arg.replace("{DRIVE}", &drive_token))
        .collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    host.run(uffs_exe, &arg_refs)
        .ok()
        .and_then(|out| out.stdout.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Build feasibility cells for every loaded drive × pattern combination.
fn feasibility_cells(
    host: &dyn Host,
    spec: &PreflightSpec,
    drives: &[DrivePreflight],
) -> Vec<CellFeasibility> {
    let mut cells = Vec::new();
    for drive in drives.iter().filter(|drive| drive.loaded) {
        for pattern in &spec.patterns {
            let est_rows = estimate_rows(host, &spec.uffs_exe, drive.drive, pattern);
            cells.push(CellFeasibility {
                drive: drive.drive,
                pattern: pattern.name.clone(),
                est_rows,
                es_feasible: est_rows <= ES_IPC_ROW_CEILING,
            });
        }
    }
    cells
}

/// Capture a [`PreflightResult`] for the given [`PreflightSpec`].
///
/// Reads `Everything.ini` (never writes it), fetches drive record counts from
/// `uffs daemon status` in a single call, probes each candidate drive's ES
/// state, then estimates per-pattern feasibility for the loaded drives.
#[must_use]
pub fn capture(host: &dyn Host, spec: &PreflightSpec) -> PreflightResult {
    let ini = host
        .read_file(&spec.ini_path)
        .map(|bytes| String::from_utf8(bytes).unwrap_or_default())
        .unwrap_or_default();
    let configured_drives = parse_drives_from_ini(&ini);

    let es_available = check_es_available(host, &spec.es_exe, &spec.es_instance_name);

    let status = daemon_status_output(host, &spec.uffs_exe);
    let mut uffs_counts = parse_daemon_status_drives(&status);

    // Preload any candidate drives that UFFS knows about (present in the
    // status output, even as [Parked]) but whose record count is not yet
    // available.  Drives completely absent from the status (e.g. a drive
    // letter the operator typed but UFFS has never indexed) are silently
    // skipped — the filter below will warn and drop them.
    let known_drives = parse_daemon_known_drives(&status);
    warm_parked_drives(
        host,
        &spec.uffs_exe,
        &spec.candidate_drives,
        &uffs_counts,
        &known_drives,
    );
    if spec
        .candidate_drives
        .iter()
        .any(|drive| known_drives.contains(drive) && !uffs_counts.contains_key(drive))
    {
        let status2 = daemon_status_output(host, &spec.uffs_exe);
        uffs_counts = parse_daemon_status_drives(&status2);
    }

    let drives: Vec<DrivePreflight> = spec
        .candidate_drives
        .iter()
        .filter(|&&drive| match uffs_counts.get(&drive).copied() {
            None => {
                host.out(&format!(
                    "[preflight] WARNING: drive {drive}: not found in UFFS daemon \
                         index — skipping"
                ));
                false
            }
            Some(0) => {
                host.out(&format!(
                    "[preflight] WARNING: drive {drive}: 0 records in UFFS daemon \
                         index — skipping"
                ));
                false
            }
            Some(_) => true,
        })
        .map(|&drive| {
            let uffs_record_count = uffs_counts.get(&drive).copied().unwrap_or(0);
            probe_drive(
                host,
                spec,
                drive,
                configured_drives.contains(&drive),
                es_available.as_ref(),
                uffs_record_count,
            )
        })
        .collect();

    let cells = feasibility_cells(host, spec, &drives);
    PreflightResult { drives, cells }
}

/// Run `uffs daemon status` once and return the raw stdout (`""` on failure).
fn daemon_status_output(host: &dyn Host, uffs_exe: &str) -> String {
    host.run(uffs_exe, &["daemon", "status"])
        .map(|out| out.stdout)
        .unwrap_or_default()
}

/// Preload any candidate drive that is absent from `warm_counts` (i.e. parked
/// or cold) so the follow-up `uffs daemon status` returns a `[Warm]` line with
/// a live record count.
///
/// Fires `uffs daemon preload <DRIVE>` for each such drive and prints a status
/// line so the operator knows the tool is promoting drives.  A preload failure
/// is non-fatal — the drive will still appear with count = 0.
fn warm_parked_drives(
    host: &dyn Host,
    uffs_exe: &str,
    candidates: &[char],
    warm_counts: &alloc::collections::BTreeMap<char, u64>,
    known_drives: &alloc::collections::BTreeSet<char>,
) {
    for &drive in candidates {
        if warm_counts.contains_key(&drive) {
            continue; // already warm
        }
        if !known_drives.contains(&drive) {
            continue; // UFFS has never heard of this drive — skip silently
        }
        host.out(&format!(
            "[preflight] Preloading parked drive {drive}: into Warm tier …"
        ));
        let drive_s = drive.to_string();
        if let Err(err) = host.run(uffs_exe, &["daemon", "preload", &drive_s]) {
            host.out(&format!(
                "[preflight] WARNING: could not preload drive {drive}: {err}"
            ));
        }
    }
}

/// Render a GFM ES RAM budget table for display before the matrix.
///
/// Columns: Drive | UFFS records | Est. RAM | ES index | Fits budget
/// Drives are accepted in candidate order (as the operator specified them)
/// until `es_ram_budget_bytes` would be exceeded.  A footer shows the total
/// RAM of fitting drives vs the budget cap.
#[must_use]
pub fn render_drive_table(result: &PreflightResult, es_ram_budget_bytes: u64) -> String {
    if result.drives.is_empty() {
        return String::new();
    }
    let header = "| Drive | UFFS records | Est. RAM | ES index  | Fits budget |";
    let sep = "|-------|-------------|----------|-----------|-------------|";

    let mut cumulative_bytes: u64 = 0;
    let mut budget_fits: alloc::collections::BTreeSet<char> = alloc::collections::BTreeSet::new();
    for dp in &result.drives {
        let est = dp.uffs_record_count.saturating_mul(UFFS_BYTES_PER_RECORD);
        if es_ram_budget_bytes == 0 || cumulative_bytes.saturating_add(est) <= es_ram_budget_bytes {
            cumulative_bytes = cumulative_bytes.saturating_add(est);
            budget_fits.insert(dp.drive);
        }
    }

    let rows: Vec<String> = result
        .drives
        .iter()
        .map(|dp| {
            let records = fmt_count(dp.uffs_record_count);
            let est_ram = fmt_ram(dp.uffs_record_count.saturating_mul(UFFS_BYTES_PER_RECORD));
            let es_index = match dp.es_status {
                EsStatus::Loaded => "loaded",
                EsStatus::NotInstalled => "not installed",
                EsStatus::DaemonNotRunning => "not running",
                EsStatus::DaemonStarting => "starting",
                EsStatus::StillIndexing => "indexing",
                EsStatus::NotConfigured => "not configured",
            };
            let fits = if budget_fits.contains(&dp.drive) {
                "✓"
            } else {
                "✗ over budget"
            };
            format!(
                "| {drive}     | {records:>12} | {est_ram:>8} | {es_index:<9} | {fits:<11} |",
                drive = dp.drive
            )
        })
        .collect();

    let budget_summary = if es_ram_budget_bytes > 0 {
        format!(
            "\n> ES RAM budget: {} used of {} cap ({} drive(s) fit)",
            fmt_ram(cumulative_bytes),
            fmt_ram(es_ram_budget_bytes),
            budget_fits.len()
        )
    } else {
        String::new()
    };

    format!(
        "### ES RAM budget\n\n{header}\n{sep}\n{}\n{budget_summary}\n",
        rows.join("\n")
    )
}

/// Format a record count with thousands separators (e.g. `3,408,843`).
fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Format bytes as `"<whole>.<tenth> GiB"` or `"<whole> MiB"` using integer
/// math only — no floating-point arithmetic.
fn fmt_ram(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        let whole = bytes / GIB;
        let tenth = (bytes % GIB) * 10 / GIB;
        format!("{whole}.{tenth} GiB")
    } else {
        let mib = bytes / MIB;
        format!("{mib} MiB")
    }
}

/// Serialize `result` to `bundle_dir/competitor-preflight.json`.
///
/// # Errors
/// Returns an error if serialization fails or the file cannot be written.
pub fn write(host: &dyn Host, result: &PreflightResult, bundle_dir: &Path) -> Result<()> {
    let json = serde_json::to_vec_pretty(result)?;
    let path = bundle_dir.join("competitor-preflight.json");
    host.write_file(&path, &json)
        .map_err(|err| BenchError::io(&path, err))?;
    Ok(())
}

#[cfg(test)]
mod tests;
