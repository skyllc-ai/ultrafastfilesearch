// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs --update [<action>]` — self-update (design in
//! `docs/dev/architecture/UFFS-Self-Update-Feasibility-and-Design.md`; CLI
//! grammar in `docs/architecture/cli-grammar.md`).
//!
//! Phase A (detect & capture) is the default (no action): it discovers
//! every install *root* from the live anchors (invoking CLI + running
//! daemon / MCP / broker), enumerates the UFFS binaries in each, classifies
//! the channel that placed them, validates each binary's on-disk version,
//! and captures the running processes' launch recipes — mutating nothing.
//! The `snapshot` / `acquire` / `apply` / `doctor` actions add the later
//! phases on top; `recover` finishes an interrupted update on demand.
//!
//! Entry point: `run_update` (dispatched from `--update` in `main`).

mod acquire;
mod apply;
mod binaries;
mod channel;
mod doctor;
mod model;
mod procinfo;
mod report;
mod self_heal;
mod snapshot;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use model::{Anchor, Channel, Component, DetectionReport, InstallRoot, RunningProcess, Scope};

/// Run `uffs --update [<action>]` — uniform `--<command> [action] [--options]`
/// grammar (design: `docs/architecture/cli-grammar.md`). The action is the
/// first positional token:
///
/// - *(none)* → detect only (Phase A).
/// - `snapshot` → detect + freeze a snapshot (Phase B).
/// - `acquire`  → + download + SHA-verify into staging (Phase C).
/// - `apply`    → + the full mutating update (stop/swap/smoke/commit/restore).
/// - `doctor`   → end-to-end health check (`--repair` / `--offline`).
/// - `recover`  → finish/roll back an interrupted update in the foreground.
///
/// Options (`--version`, `--repair`, `--offline`) follow the action.
pub(crate) fn run_update(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    // The action is the first *positional* token (a leading flag or nothing
    // means bare detect). Validate up front, before any detection/output.
    let action = args
        .first()
        .map(String::as_str)
        .filter(|tok| !tok.starts_with('-'));
    if let Some(act) = action
        && !matches!(act, "snapshot" | "acquire" | "apply" | "doctor" | "recover")
    {
        bail!(
            "unknown `--update` action `{act}` — expected: snapshot | acquire | apply | doctor | recover"
        );
    }

    // `-v` / `--verbose`: show the full per-binary + per-process breakdown
    // (and forward verbosity to the spawned `uffs-update` helper). Default is
    // a few plain-language lines for someone who just wants the gist.
    let verbose = args.iter().any(|arg| arg == "-v" || arg == "--verbose");

    // `recover` finishes (or rolls back) an interrupted update in the
    // foreground — the on-demand twin of the startup best-effort self-heal.
    if action == Some("recover") {
        return self_heal::run_foreground();
    }

    // `doctor` runs its own flow: detect → freeze a snapshot → hand off to the
    // helper's health check (which prints its own report).
    if action == Some("doctor") {
        let report = detect();
        let snapshot_path = snapshot::write_snapshot(&report)?;
        return doctor::spawn(&snapshot_path, args);
    }

    let report = detect();
    report::print_human(&report, verbose);
    if action == Some("snapshot") {
        write_and_report_snapshot(&report);
    }
    if verbose {
        print_phase_a_footer();
    }

    // `apply` runs the full mutating update (acquire → apply); `acquire` only
    // stages + verifies. `apply` implies the acquire step.
    if matches!(action, Some("acquire" | "apply")) {
        let snapshot_path = snapshot::write_snapshot(&report)?;
        acquire::spawn(
            &snapshot_path,
            flag_value(args, "--version").as_deref(),
            verbose,
        )?;
        if action == Some("apply") {
            apply::spawn(&snapshot_path, verbose)?;
        }
    }
    Ok(())
}

/// Phase H self-heal entry: spawn `uffs-update recover` if a live update
/// journal is present. Best-effort and non-blocking, so a crash mid-update
/// is healed on the next `uffs` invocation. Called once at CLI startup.
pub(crate) fn maybe_self_heal() {
    self_heal::trigger();
}

/// Return the value following `name` in `args` (`--name value`).
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

/// Write a Phase-B snapshot and report where it landed.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn write_and_report_snapshot(report: &DetectionReport) {
    match snapshot::write_snapshot(report) {
        Ok(path) => println!("\nSnapshot written: {}", path.display()),
        Err(err) => {
            #[expect(clippy::print_stderr, reason = "CLI user-facing error")]
            {
                eprintln!("\nSnapshot failed: {err}");
            }
        }
    }
}

/// Phase A orchestration: anchors → roots → channel + versions, plus the
/// running-process map.
fn detect() -> DetectionReport {
    let mut roots: Vec<InstallRoot> = Vec::new();
    let mut running: Vec<RunningProcess> = Vec::new();

    // A.1 anchor #1 — the invoking CLI.
    if let Some(dir) = current_exe_dir() {
        upsert_root(&mut roots, dir, Anchor::Cli);
    }
    // A.1 anchor #2 — the running daemon. Prefer its persisted launch
    // state (reliable command line); fall back to native introspection.
    capture_daemon(&mut roots, &mut running);
    // A.1 anchor #3 — running MCP gateway(s).
    for pid in procinfo::find_pids_by_name("uffsmcp") {
        capture_native(&mut roots, &mut running, Component::Mcp, Anchor::Mcp, pid);
    }
    // A.1 anchor #4 — the broker service (Windows-only).
    capture_broker(&mut roots, &mut running);

    // A.2 + A.3 + A.4 — per root: enumerate binaries, classify, version.
    for root in &mut roots {
        root.binaries = binaries::enumerate(&root.dir);
        let (chan, scope) = channel::classify(&root.dir);
        root.channel = chan;
        root.scope = scope;
    }

    DetectionReport { roots, running }
}

/// Directory of the currently-running `uffs` executable.
fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Resolve the running daemon's pid — PID file first, then a name scan.
fn daemon_pid() -> Option<u32> {
    procinfo::daemon_pid_from_file()
        .or_else(|| procinfo::find_pids_by_name("uffsd").into_iter().next())
}

/// Insert `dir` as an install root (deduplicated by canonical path) and
/// record that `anchor` surfaced it.
fn upsert_root(roots: &mut Vec<InstallRoot>, dir: PathBuf, anchor: Anchor) {
    let key = std::fs::canonicalize(&dir).unwrap_or(dir);
    if let Some(existing) = roots.iter_mut().find(|root| root.dir == key) {
        existing.note_anchor(anchor);
        return;
    }
    roots.push(InstallRoot {
        dir: key,
        channel: Channel::Unknown,
        scope: Scope::Unknown,
        anchored_by: vec![anchor],
        binaries: Vec::new(),
    });
}

/// Capture the daemon anchor: prefer its persisted launch state (which
/// carries a reliable command line); otherwise fall back to native
/// introspection of the pid from the PID file.
fn capture_daemon(roots: &mut Vec<InstallRoot>, running: &mut Vec<RunningProcess>) {
    if let Some((pid, state)) = procinfo::daemon_launch_state() {
        let image_path = state.image_path;
        if let Some(dir) = image_path.as_deref().and_then(Path::parent) {
            upsert_root(roots, dir.to_path_buf(), Anchor::Daemon);
        }
        let version = state
            .version
            .or_else(|| image_path.as_deref().and_then(binaries::probe_version));
        running.push(RunningProcess {
            component: Component::Daemon,
            pid,
            image_path,
            command_line: state.command_line,
            version,
        });
    } else if let Some(pid) = daemon_pid() {
        capture_native(roots, running, Component::Daemon, Anchor::Daemon, pid);
    }
}

/// Capture a live process via native introspection: contribute its image
/// directory as a root and record it in the running-process map.
fn capture_native(
    roots: &mut Vec<InstallRoot>,
    running: &mut Vec<RunningProcess>,
    component: Component,
    anchor: Anchor,
    pid: u32,
) {
    let image_path = procinfo::image_path(pid);
    if let Some(dir) = image_path.as_deref().and_then(Path::parent) {
        upsert_root(roots, dir.to_path_buf(), anchor);
    }
    let version = image_path.as_deref().and_then(binaries::probe_version);
    running.push(RunningProcess {
        component,
        pid,
        image_path,
        command_line: procinfo::command_line(pid),
        version,
    });
}

/// Surface the broker service's install root + running process.
///
/// Cross-platform: `procinfo::broker_service()` returns `None` off
/// Windows, so this is an early no-op there — but the `Anchor::Broker` /
/// `Component::Broker` constructions stay in-source on every target
/// (no platform-conditional dead code).
fn capture_broker(roots: &mut Vec<InstallRoot>, running: &mut Vec<RunningProcess>) {
    let Some((bin_path, service_pid)) = procinfo::broker_service() else {
        return;
    };
    if let Some(dir) = bin_path.parent() {
        upsert_root(roots, dir.to_path_buf(), Anchor::Broker);
    }
    let version = binaries::probe_version(&bin_path);
    if let Some(pid) = service_pid {
        running.push(RunningProcess {
            component: Component::Broker,
            pid,
            image_path: Some(bin_path),
            command_line: procinfo::command_line(pid),
            version,
        });
    }
}

/// Print the `uffs --update` help text.
#[expect(clippy::print_stdout, reason = "intentional help output")]
fn print_help() {
    println!(
        "uffs --update — self-update\n\n\
         USAGE:\n\
         \x20 uffs --update [<action>] [--options]\n\n\
         Discovers where UFFS is installed (from the running CLI, daemon,\n\
         MCP gateway, and broker service), lists binaries + versions per\n\
         location, and shows the running processes' launch recipes.\n\n\
         ACTIONS:\n\
         \x20 (none)              Detect only — non-mutating; nothing is changed.\n\
         \x20 snapshot            Detect + persist the state to JSON.\n\
         \x20 acquire             + download + SHA-256-verify the release into\n\
         \x20                     staging (via the uffs-update helper). No replace.\n\
         \x20 apply               + the FULL mutating update: stop services,\n\
         \x20                     atomically swap + smoke-test, commit, restart.\n\
         \x20                     Journaled + auto-rollback on failure.\n\
         \x20 doctor              End-to-end health check (versions, dirs, journal,\n\
         \x20                     backups, services, broker pipe, release reach).\n\
         \x20 recover             Finish or roll back an interrupted update now\n\
         \x20                     (foreground; the on-demand self-heal).\n\n\
         OPTIONS:\n\
         \x20 -v, --verbose       Show the full breakdown — per-binary versions,\n\
         \x20                     PIDs, launch commands, every doctor check.\n\
         \x20 --version <tag>     Acquire/apply a specific release tag (default: latest).\n\
         \x20 --repair            (doctor) self-heal what can be fixed.\n\
         \x20 --offline           (doctor) skip the network checks.\n\n\
         EXAMPLES:\n\
         \x20 uffs --update                 uffs --update acquire --version v0.6.3\n\
         \x20 uffs --update apply           uffs --update doctor --repair\n"
    );
}

/// Footer clarifying which mutating phases are not yet implemented.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_phase_a_footer() {
    println!(
        "\n(Detect + snapshot + acquire are non-mutating. Stop / replace / restore \
         land in the apply phase; nothing on a live install was changed.)"
    );
}

#[cfg(test)]
mod tests {
    use super::model::{Anchor, InstallRoot};
    use super::upsert_root;

    fn root_dirs(roots: &[InstallRoot]) -> Vec<String> {
        roots
            .iter()
            .map(|root| root.dir.display().to_string())
            .collect()
    }

    fn anchors_of(root: &InstallRoot) -> Vec<Anchor> {
        root.anchored_by.clone()
    }

    #[test]
    fn upsert_dedupes_same_dir_and_merges_anchors() {
        let mut roots: Vec<InstallRoot> = Vec::new();
        // A non-existent path won't canonicalise, so the raw path is the key.
        let dir = std::path::PathBuf::from("/nonexistent/uffs/root");
        upsert_root(&mut roots, dir.clone(), Anchor::Cli);
        upsert_root(&mut roots, dir, Anchor::Daemon);
        assert_eq!(roots.len(), 1, "same dir must not create a second root");
        let first = roots.first().expect("one root");
        assert_eq!(anchors_of(first), vec![Anchor::Cli, Anchor::Daemon]);
    }

    #[test]
    fn upsert_keeps_distinct_dirs_separate() {
        let mut roots: Vec<InstallRoot> = Vec::new();
        upsert_root(&mut roots, "/nonexistent/a".into(), Anchor::Cli);
        upsert_root(&mut roots, "/nonexistent/b".into(), Anchor::Daemon);
        assert_eq!(roots.len(), 2);
        assert_eq!(root_dirs(&roots), vec!["/nonexistent/a", "/nonexistent/b"]);
    }

    #[test]
    fn upsert_same_anchor_twice_is_idempotent() {
        let mut roots: Vec<InstallRoot> = Vec::new();
        upsert_root(&mut roots, "/nonexistent/a".into(), Anchor::Cli);
        upsert_root(&mut roots, "/nonexistent/a".into(), Anchor::Cli);
        let first = roots.first().expect("one root");
        assert_eq!(anchors_of(first), vec![Anchor::Cli]);
    }
}
