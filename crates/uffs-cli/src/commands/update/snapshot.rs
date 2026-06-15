// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase B — snapshot: freeze the Phase-A detection plus the daemon's
//! live drive/tier state into a timestamped JSON file under the update
//! working directory (`<lifecycle_dir>/update/snapshot-<unix>.json`).
//!
//! This is the durable record the later acquire / stop / replace /
//! restore phases read back. Writing it costs nothing and lets a failed
//! later step abort with a faithful "what was here before" picture.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::model::DetectionReport;
use super::procinfo;

/// Directory that holds update snapshots + staging
/// (`<lifecycle_dir>/update`).
pub(crate) fn update_dir() -> PathBuf {
    procinfo::lifecycle_dir().join("update")
}

/// Capture + persist a snapshot for `report`. Returns the file path.
///
/// # Errors
///
/// Propagates any directory-create or file-write failure.
pub(crate) fn write_snapshot(report: &DetectionReport) -> std::io::Result<PathBuf> {
    let dir = update_dir();
    std::fs::create_dir_all(&dir)?;
    let captured_unix = unix_now();
    let value = build_snapshot_value(report, &daemon_drive_state(), captured_unix);
    let path = dir.join(format!("snapshot-{captured_unix}.json"));
    let body = serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"schema\":2,\"error\":\"serialize\"}".to_owned());
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Seconds since the Unix epoch (0 if the clock is before it).
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |dur| dur.as_secs())
}

/// Query the daemon's live drive/tier state via the `status_drives`
/// RPC. Empty when no daemon is running (best-effort).
fn daemon_drive_state() -> Vec<Value> {
    let Ok(mut client) = uffs_client::connect_sync::UffsClientSync::connect_raw() else {
        return Vec::new();
    };
    let Ok(response) = client.status_drives() else {
        return Vec::new();
    };
    response
        .drives
        .into_iter()
        .map(|drive| {
            json!({
                "letter": drive.letter.to_string(),
                "tier": drive.tier,
                "resident_bytes": drive.resident_bytes,
                "pin_until_unix_ms": drive.pin_until_unix_ms,
            })
        })
        .collect()
}

/// Build the snapshot JSON (pure — unit-testable without I/O).
fn build_snapshot_value(
    report: &DetectionReport,
    daemon_drives: &[Value],
    captured_unix: u64,
) -> Value {
    let targets: Vec<Value> = report
        .roots
        .iter()
        .map(|root| {
            json!({
                "root": root.dir.display().to_string(),
                "channel": root.channel.label(),
                "scope": root.scope.label(),
                "anchored_by": root.anchored_by.iter().map(|anchor| anchor.label()).collect::<Vec<_>>(),
                "binaries": root.binaries.iter().map(|binary| json!({
                    "name": binary.name,
                    "on_disk_version": binary.version,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    let running: Vec<Value> = report
        .running
        .iter()
        .map(|proc| {
            json!({
                "component": proc.component.label(),
                "pid": proc.pid,
                "image_path": proc.image_path.as_ref().map(|path| path.display().to_string()),
                "command_line": proc.command_line,
                "version": proc.version,
            })
        })
        .collect();

    json!({
        "schema": 2,
        "captured_unix": captured_unix,
        "targets": targets,
        "running": running,
        "daemon": { "drives": daemon_drives },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::model::{
        Anchor, BinaryInfo, Channel, Component, DetectionReport, InstallRoot, RunningProcess, Scope,
    };
    use super::build_snapshot_value;

    fn sample_report() -> DetectionReport {
        DetectionReport {
            roots: vec![InstallRoot {
                dir: "/opt/uffs".into(),
                channel: Channel::Unmanaged,
                scope: Scope::Unknown,
                anchored_by: vec![Anchor::Cli, Anchor::Daemon],
                binaries: vec![BinaryInfo {
                    name: "uffsd".to_owned(),
                    version: Some("0.6.2".to_owned()),
                }],
            }],
            running: vec![RunningProcess {
                component: Component::Daemon,
                pid: 4242,
                image_path: Some("/opt/uffs/uffsd".into()),
                command_line: Some("uffsd --no-retire".to_owned()),
                version: Some("0.6.2".to_owned()),
            }],
        }
    }

    #[test]
    fn snapshot_shape_is_stable() {
        let drives = vec![json!({"letter": "C", "tier": "warm"})];
        let value = build_snapshot_value(&sample_report(), &drives, 1_700_000_000_u64);
        // `pointer` avoids panicking index ops; `json!(typed)` pins the
        // expected literal type (no default numeric fallback).
        let probe = |path: &str, expected: serde_json::Value| {
            assert_eq!(value.pointer(path), Some(&expected), "mismatch at {path}");
        };
        probe("/schema", json!(2_u64));
        probe("/captured_unix", json!(1_700_000_000_u64));
        probe("/targets/0/channel", json!("unmanaged"));
        probe("/targets/0/anchored_by/1", json!("daemon"));
        probe("/targets/0/binaries/0/on_disk_version", json!("0.6.2"));
        probe("/running/0/component", json!("daemon"));
        probe("/running/0/pid", json!(4242_u32));
        probe("/running/0/command_line", json!("uffsd --no-retire"));
        probe("/daemon/drives/0/letter", json!("C"));
    }
}
