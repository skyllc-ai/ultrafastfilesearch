// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Rendering of the `uffs --uninstall` analysis (task U-12): the binary
//! resolution table + the artifact inventory, in human form and as `--json`.
//! The removal plan is layered on in later milestones.

use serde_json::{Value, json};

use super::inventory::Inventory;
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

/// Emit the full analysis (binaries + artifacts + broker state) as JSON.
#[expect(clippy::print_stdout, reason = "machine-readable CLI output")]
pub(crate) fn print_json(resolution: &[StemResolution], inventory: &Inventory) {
    let value = analysis_json(resolution, inventory);
    let text = serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"error\":\"serialize\"}".to_owned());
    println!("{text}");
}

/// Build the analysis JSON value (pure; unit-testable without IO).
fn analysis_json(resolution: &[StemResolution], inventory: &Inventory) -> Value {
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
        let value = analysis_json(&[], &inventory);
        assert!(value.get("binaries").is_some());
        assert!(value.get("artifacts").is_some());
        assert_eq!(
            value.get("broker_service").and_then(Value::as_str),
            Some("absent")
        );
    }
}
