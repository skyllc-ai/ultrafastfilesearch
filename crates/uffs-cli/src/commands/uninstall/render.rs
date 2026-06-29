// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Rendering of the `uffs --uninstall` analysis (task U-12): the binary
//! resolution table + the artifact inventory, in human form and as `--json`.
//! The removal plan is layered on in later milestones.

use serde_json::{Value, json};

use super::inventory::Inventory;
use super::plan::RemovalPlan;
use super::remove::{ItemStatus, RemovalOutcome};
use super::resolve_order::{ResolutionState, StemResolution};

/// Print the discovered-binary resolution table: for each stem, every copy in
/// OS search order, with the one a bare command runs flagged ACTIVE.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_resolution_table(stems: &[StemResolution]) {
    if stems.is_empty() {
        println!("No UFFS binaries found in any install root or on PATH.");
        return;
    }
    println!("Discovered UFFS binaries (the copy a bare command runs is ACTIVE):\n");
    for stem in stems {
        println!("{}:", stem.stem);
        for copy in &stem.copies {
            let state = match copy.state {
                ResolutionState::Active => "ACTIVE",
                ResolutionState::Shadowed if copy.on_search_path => "shadowed",
                ResolutionState::Shadowed => "off-path",
            };
            let version = copy.version.as_deref().unwrap_or("-");
            println!(
                "  {state:<8}  {version:<9}  {channel:<9}  {scope:<7}  {dir}",
                channel = copy.channel.label(),
                scope = copy.scope.label(),
                dir = copy.dir.display(),
            );
        }
    }
}

/// Print the non-binary artifact inventory (data / cache / legacy / config)
/// plus the broker-service state.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_inventory(inventory: &Inventory) {
    println!("\nData / cache / config:");
    for dir in &inventory.dirs {
        let size = if dir.exists {
            human_bytes(dir.size_bytes)
        } else {
            "absent".to_owned()
        };
        println!(
            "  {kind:<13}  {size:<10}  {path}",
            kind = dir.kind.label(),
            path = dir.path.display(),
        );
    }
    println!(
        "\nBroker service ({name}): {state}",
        name = uffs_broker_protocol::SERVICE_NAME,
        state = inventory.broker_service.label(),
    );
}

/// Print the ordered removal plan (consent surface, U-21). Items are numbered
/// across groups; ones needing Administrator are flagged.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_plan(plan: &RemovalPlan) {
    if plan.is_empty() {
        println!("\nNothing to remove: no UFFS install or artifacts were found.");
        return;
    }
    println!("\nThe following will be PERMANENTLY removed (no recovery):");
    let mut index: usize = 1;
    for group in &plan.groups {
        println!("\n {}", group.title);
        for item in &group.items {
            let elevated = if item.needs_elevation {
                "  (needs Administrator)"
            } else {
                ""
            };
            println!(
                "  [{index}] {desc}{elevated}",
                desc = item.target.describe()
            );
            index = index.saturating_add(1);
        }
    }
    println!(
        "\nReclaims ~{} across {} item(s).",
        human_bytes(plan.total_bytes()),
        plan.item_count(),
    );
}

/// Print the elevation refusal (U-30): the items that need Administrator and
/// the re-run hint. Goes to stderr; the caller exits non-zero without any
/// effect.
#[expect(clippy::print_stderr, reason = "CLI user-facing error")]
pub(crate) fn print_elevation_refusal(plan: &RemovalPlan) {
    eprintln!("\nThis uninstall includes items that require Administrator:");
    for group in &plan.groups {
        for item in &group.items {
            if item.needs_elevation {
                eprintln!("  - {}", item.target.describe());
            }
        }
    }
    eprintln!(
        "\nRe-run with elevated privileges (sudo on Linux/macOS, an elevated \
         shell on Windows):\n  uffs --uninstall"
    );
}

/// Print stray UFFS-named files the deep sweep found outside the known roots.
/// These are listed for review only, never auto-removed.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_strays(strays: &[std::path::PathBuf]) {
    if strays.is_empty() {
        return;
    }
    println!(
        "\nStray UFFS-named files found elsewhere (NOT removed — review and delete\n\
         manually if they are unwanted; one may be a copy you placed yourself):"
    );
    for path in strays {
        println!("  {}", path.display());
    }
}

/// Note that a prior uninstall was interrupted and this run completes it.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_resumed_note() {
    println!(
        "A previous uninstall did not finish. Removal is idempotent, so this run \
         will complete it.\n"
    );
}

/// Warn that the in-progress journal marker could not be written/cleared.
#[expect(clippy::print_stderr, reason = "CLI user-facing error")]
pub(crate) fn print_journal_warning(error: &anyhow::Error) {
    eprintln!("note: uninstall progress marker could not be updated ({error:#}).");
}

/// Warn that the running self-binary could not be scheduled for deletion.
#[expect(clippy::print_stderr, reason = "CLI user-facing error")]
pub(crate) fn print_self_delete_warning(error: &anyhow::Error) {
    eprintln!(
        "\nCould not schedule deletion of the running uffs binary ({error:#}).\n\
         Delete it manually once this process has exited."
    );
}

/// Print the post-removal verification: clean, or the locations that survived.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_verification(remaining: &[std::path::PathBuf]) {
    if remaining.is_empty() {
        println!("\nVerified: all targeted UFFS locations are gone.");
        return;
    }
    println!(
        "\nVerification: {} location(s) still present (a reboot may be pending, or \
         elevation/sudo is needed):",
        remaining.len()
    );
    for path in remaining {
        println!("  {}", path.display());
    }
}

/// Print the outcome of a removal run: counts, any failures, and a retry hint.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_outcome(outcome: &RemovalOutcome) {
    println!(
        "\nRemoval finished: {} removed, {} failed.",
        outcome.done_count(),
        outcome.failed_count(),
    );
    for (description, status) in &outcome.results {
        if let ItemStatus::Failed(error) = status {
            println!("  FAILED  {description}  ({error})");
        }
    }
    if !outcome.all_done() {
        println!(
            "\nSome items could not be removed. Retry with elevated privileges \
             (sudo on Linux/macOS, an elevated shell on Windows)."
        );
    }
}

/// Emit the full analysis (binaries + artifacts + broker state + plan) as JSON.
#[expect(clippy::print_stdout, reason = "machine-readable CLI output")]
pub(crate) fn print_json(resolution: &[StemResolution], inventory: &Inventory, plan: &RemovalPlan) {
    let value = analysis_json(resolution, inventory, plan);
    let text = serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"error\":\"serialize\"}".to_owned());
    println!("{text}");
}

/// Build the plan JSON value (pure).
fn plan_json(plan: &RemovalPlan) -> Value {
    let groups: Vec<Value> = plan
        .groups
        .iter()
        .map(|group| {
            let items: Vec<Value> = group
                .items
                .iter()
                .map(|item| {
                    json!({
                        "action": item.target.action_label(),
                        "description": item.target.describe(),
                        "needs_elevation": item.needs_elevation,
                        "bytes": item.bytes,
                    })
                })
                .collect();
            json!({ "title": group.title, "items": items })
        })
        .collect();
    json!({
        "total_bytes": plan.total_bytes(),
        "item_count": plan.item_count(),
        "requires_elevation": plan.requires_elevation(),
        "groups": groups,
    })
}

/// Build the analysis JSON value (pure; unit-testable without IO).
fn analysis_json(
    resolution: &[StemResolution],
    inventory: &Inventory,
    plan: &RemovalPlan,
) -> Value {
    let binaries: Vec<Value> = resolution
        .iter()
        .map(|stem| {
            let copies: Vec<Value> = stem
                .copies
                .iter()
                .map(|copy| {
                    json!({
                        "state": match copy.state {
                            ResolutionState::Active => "active",
                            ResolutionState::Shadowed => "shadowed",
                        },
                        "on_search_path": copy.on_search_path,
                        "version": copy.version,
                        "channel": copy.channel.label(),
                        "scope": copy.scope.label(),
                        "dir": copy.dir.display().to_string(),
                    })
                })
                .collect();
            json!({ "stem": stem.stem, "copies": copies })
        })
        .collect();
    let artifacts: Vec<Value> = inventory
        .dirs
        .iter()
        .map(|dir| {
            json!({
                "kind": dir.kind.label(),
                "path": dir.path.display().to_string(),
                "exists": dir.exists,
                "size_bytes": dir.size_bytes,
            })
        })
        .collect();
    json!({
        "binaries": binaries,
        "artifacts": artifacts,
        "broker_service": inventory.broker_service.label(),
        "plan": plan_json(plan),
    })
}

/// Format a byte count for humans using integer math (no float casts, which the
/// workspace `cast_precision_loss` lint forbids). One decimal place.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    let (unit, label) = if bytes >= GIB {
        (GIB, "GB")
    } else if bytes >= MIB {
        (MIB, "MB")
    } else if bytes >= KIB {
        (KIB, "KB")
    } else {
        return format!("{bytes} B");
    };
    let whole = bytes / unit;
    let frac = (bytes % unit).saturating_mul(10) / unit;
    format!("{whole}.{frac} {label}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::inventory::{ArtifactDir, ArtifactKind, BrokerServiceState, Inventory};
    use super::super::plan::RemovalPlan;
    use super::{Value, analysis_json, human_bytes};

    #[test]
    fn human_bytes_picks_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(
            human_bytes(1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "1.5 GB"
        );
    }

    #[test]
    fn json_has_top_level_sections() {
        let inventory = Inventory {
            dirs: vec![ArtifactDir {
                kind: ArtifactKind::Cache,
                path: PathBuf::from("/x/cache"),
                exists: true,
                size_bytes: 10,
            }],
            broker_service: BrokerServiceState::Absent,
        };
        let value = analysis_json(&[], &inventory, &RemovalPlan::default());
        assert!(value.get("binaries").is_some());
        assert!(value.get("artifacts").is_some());
        assert!(value.get("plan").is_some());
        assert_eq!(
            value.get("broker_service").and_then(Value::as_str),
            Some("absent")
        );
    }
}
