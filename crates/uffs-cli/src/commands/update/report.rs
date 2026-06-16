// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Human-readable rendering of the Phase-A [`DetectionReport`].
//!
//! Pure formatting: it never mutates the system, it only prints what
//! detection found (the `--check` view of the self-update flow).
//!
//! Two views — chosen by the caller's `verbose` flag:
//! - **default**: a few plain-language lines (where, which version, what's
//!   running) for someone who just wants to know if they're up to date;
//! - **`-v`**: the full breakdown — every binary's version, every running
//!   process's PID / image path / launch command.

use super::model::{DetectionReport, RunningProcess};

/// Print the detection report. `verbose` selects the full breakdown over the
/// concise default.
pub(crate) fn print_human(report: &DetectionReport, verbose: bool) {
    if verbose {
        print_full(report);
    } else {
        print_concise(report);
    }
}

/// Concise, non-technical summary: where UFFS is, which version, and what is
/// running — with a pointer to `-v` for the details.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_concise(report: &DetectionReport) {
    println!("UFFS self-update");

    match report.roots.as_slice() {
        [] => {
            println!("  Install:  (none found — is UFFS on your PATH?)");
            return;
        }
        [only] => println!("  Install:  {}", only.dir.display()),
        [first, rest @ ..] => println!(
            "  Install:  {}  (and {} more)",
            first.dir.display(),
            rest.len()
        ),
    }

    match distinct_versions(report).as_slice() {
        [] => println!("  Version:  (could not be read)"),
        [only] => println!("  Version:  {only}"),
        many => println!(
            "  Version:  \u{26a0} mixed ({}) — applying an update will realign them",
            many.join(", ")
        ),
    }

    let running = running_summary(report);
    if !running.is_empty() {
        println!("  Running:  {running}");
    }

    println!("  (run with -v for per-binary versions, PIDs, and launch commands)");
}

/// Full breakdown: every root, every binary version, every running process'
/// image + launch recipe, and the version-skew line.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_full(report: &DetectionReport) {
    println!("UFFS self-update — Phase A (detect & capture)\n");

    println!("Install roots / update targets ({}):", report.roots.len());
    if report.roots.is_empty() {
        println!("  (none found — is UFFS on PATH / installed?)");
    }
    for (idx, root) in report.roots.iter().enumerate() {
        let anchors = root
            .anchored_by
            .iter()
            .map(|anchor| anchor.label())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  [{}] {}", idx + 1, root.dir.display());
        println!(
            "      channel: {}   scope: {}   anchored-by: {anchors}",
            root.channel.label(),
            root.scope.label(),
        );
        for binary in &root.binaries {
            println!(
                "      - {:<12} {}",
                binary.name,
                binary.version.as_deref().unwrap_or("?")
            );
        }
    }

    println!("\nRunning components ({}):", report.running.len());
    if report.running.is_empty() {
        println!("  (none running)");
    }
    for proc in &report.running {
        print_running(proc);
    }

    print_skew(report);
}

/// Print one running component's image + launch recipe.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_running(proc: &RunningProcess) {
    println!(
        "  {:<7} pid={:<7} version={}",
        proc.component.label(),
        proc.pid,
        proc.version.as_deref().unwrap_or("?"),
    );
    if let Some(image) = &proc.image_path {
        println!("      image: {}", image.display());
    }
    if let Some(cmd) = &proc.command_line {
        println!("      cmd:   {cmd}");
    }
}

/// The distinct binary/process versions seen across the whole report, sorted.
/// More than one ⇒ a skewed install that an update would realign.
pub(crate) fn distinct_versions(report: &DetectionReport) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();
    for root in &report.roots {
        for binary in &root.binaries {
            versions.extend(binary.version.clone());
        }
    }
    for proc in &report.running {
        versions.extend(proc.version.clone());
    }
    versions.sort_unstable();
    versions.dedup();
    versions
}

/// A short list of running components with multiplicity, e.g. `daemon, mcp ×3`.
fn running_summary(report: &DetectionReport) -> String {
    // Preserve first-seen order while counting duplicates (3 `mcp` processes
    // collapse to `mcp ×3` instead of three identical entries).
    let mut order: Vec<&str> = Vec::new();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for proc in &report.running {
        let label = proc.component.label();
        if let Some(entry) = counts.iter_mut().find(|(name, _)| *name == label) {
            entry.1 += 1;
        } else {
            order.push(label);
            counts.push((label, 1));
        }
    }
    order
        .iter()
        .filter_map(|label| counts.iter().find(|(name, _)| name == label))
        .map(|(name, count)| {
            if *count > 1 {
                format!("{name} \u{d7}{count}")
            } else {
                (*name).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print a one-line version-skew summary (full view only): the distinct
/// versions seen across every discovered binary. More than one ⇒ the install
/// is skewed and an update would realign it.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_skew(report: &DetectionReport) {
    println!();
    match distinct_versions(report).as_slice() {
        [] => println!("Version skew: (no versions could be read)"),
        [only] => println!("Version skew: none — all components at {only}"),
        many => println!(
            "Version skew: \u{26a0} multiple versions present: {}",
            many.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{Component, DetectionReport, RunningProcess};
    use super::{distinct_versions, running_summary};

    fn proc(component: Component, version: &str) -> RunningProcess {
        RunningProcess {
            component,
            pid: 1,
            image_path: None,
            command_line: None,
            version: Some(version.to_owned()),
        }
    }

    #[test]
    fn running_summary_collapses_duplicates_with_counts() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: vec![
                proc(Component::Daemon, "0.6.4"),
                proc(Component::Mcp, "0.6.4"),
                proc(Component::Mcp, "0.6.4"),
                proc(Component::Mcp, "0.6.4"),
            ],
        };
        // First-seen order preserved; 3 `mcp` processes collapse to `mcp ×3`
        // instead of three identical lines (the user-reported noise).
        assert_eq!(running_summary(&report), "daemon, mcp \u{d7}3");
    }

    #[test]
    fn running_summary_is_empty_when_nothing_runs() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: Vec::new(),
        };
        assert_eq!(running_summary(&report), "");
    }

    #[test]
    fn distinct_versions_dedups_and_sorts() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: vec![
                proc(Component::Daemon, "0.6.4"),
                proc(Component::Mcp, "0.6.2"),
                proc(Component::Broker, "0.6.4"),
            ],
        };
        // Two 0.6.4 collapse to one; sorted ascending → drives the "mixed"
        // skew warning in the concise view.
        assert_eq!(distinct_versions(&report), vec![
            "0.6.2".to_owned(),
            "0.6.4".to_owned()
        ]);
    }
}
