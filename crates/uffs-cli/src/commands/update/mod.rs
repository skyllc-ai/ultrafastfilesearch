// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs update` — self-update **Phase A: detect & capture** (design in
//! `docs/dev/architecture/UFFS-Self-Update-Feasibility-and-Design.md` §5).
//!
//! This is the **detection slice only**: it discovers every install
//! *root* from the live anchors (invoking CLI + running daemon / MCP /
//! broker), enumerates the UFFS binaries in each, classifies the channel
//! that placed them, validates each binary's on-disk version, and
//! captures the running processes' launch recipes. It **mutates
//! nothing** — stopping, replacing, and restoring land in later phases.
//!
//! Entry point: `run_update` (wired to `uffs update` in `main`).

mod binaries;
mod channel;
mod model;
mod procinfo;
mod report;

use std::path::{Path, PathBuf};

use anyhow::Result;
use model::{Anchor, Channel, Component, DetectionReport, InstallRoot, RunningProcess, Scope};

/// Run `uffs update`. Phase A only: prints the detection report.
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the command-handler signature; later self-update phases return errors"
)]
pub(crate) fn run_update(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    let report = detect();
    report::print_human(&report);
    print_phase_a_footer();
    Ok(())
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

/// Print the `uffs update` help text.
#[expect(clippy::print_stdout, reason = "intentional help output")]
fn print_help() {
    println!(
        "uffs update — self-update (Phase A: detect & capture)\n\n\
         USAGE:\n  uffs update [--check]\n\n\
         Phase A discovers where UFFS is installed (from the running CLI,\n\
         daemon, MCP gateway, and broker service), lists the binaries and\n\
         their versions per location, and shows the running processes'\n\
         launch recipes. It does not change anything yet.\n"
    );
}

/// Footer clarifying that only detection is implemented so far.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_phase_a_footer() {
    println!(
        "\n(Phase A — detection only. Stop / replace / restore land in later phases; \
         nothing was changed.)"
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
