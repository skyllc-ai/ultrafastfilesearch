// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Human-readable rendering of the Phase-A [`DetectionReport`].
//!
//! Pure formatting: it never mutates the system, it only prints what
//! detection found (the `--check` view of the self-update flow).

use super::model::{DetectionReport, RunningProcess};

/// Print the detection report to stdout.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_human(report: &DetectionReport) {
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

/// Print a one-line version-skew summary: the distinct versions seen
/// across every discovered binary. More than one ⇒ the install is
/// skewed and an update would realign it.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_skew(report: &DetectionReport) {
    let mut versions: Vec<&str> = Vec::new();
    for root in &report.roots {
        for binary in &root.binaries {
            versions.extend(binary.version.as_deref());
        }
    }
    for proc in &report.running {
        versions.extend(proc.version.as_deref());
    }
    versions.sort_unstable();
    versions.dedup();

    println!();
    match versions.as_slice() {
        [] => println!("Version skew: (no versions could be read)"),
        [only] => println!("Version skew: none — all components at {only}"),
        many => println!(
            "Version skew: ⚠ multiple versions present: {}",
            many.join(", ")
        ),
    }
}
