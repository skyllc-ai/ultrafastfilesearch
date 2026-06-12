// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! At-a-glance report header.
//!
//! Synthesizes the structured Stage 0 data — host environment, tool versions,
//! and the storage inventory — into a single `## At a glance` digest so a
//! reader knows, in five lines, what machine ran the bench, which tools and
//! versions competed, which drives (and of what kind) were actually measured,
//! and what is being measured. The detail tables (Test environment, Storage
//! devices, Negotiated matrix, Everything RAM budget) follow.
//!
//! Built at run time (not assembly) because it needs the live, structured
//! [`EnvFingerprint`] and drive records — and the Everything GUI version that
//! is backfilled only after the private instance launches.

use crate::env::EnvFingerprint;
use crate::storage::{self, DriveRecord};

/// Bundle-relative name of the rendered at-a-glance header.
pub const SUMMARY_MD: &str = "summary.md";

/// The version of tool `name` in the fingerprint, or `"?"` if absent.
fn tool_version<'fp>(fp: &'fp EnvFingerprint, name: &str) -> &'fp str {
    fp.tools
        .iter()
        .find(|tool| tool.name == name)
        .map_or("?", |tool| tool.version.as_str())
}

/// Whether `rec`'s drive letter is in the benched set.
fn is_benched(rec: &DriveRecord, benched: &[char]) -> bool {
    rec.drive
        .chars()
        .next()
        .is_some_and(|letter| benched.contains(&letter.to_ascii_uppercase()))
}

/// Render the `## At a glance` markdown digest.
///
/// Pure in its inputs (no host access), so it is covered by a golden test.
#[must_use]
pub fn render_md(fp: &EnvFingerprint, drives: &[DriveRecord], benched: &[char]) -> String {
    let elevated = if fp.elevated {
        "elevated"
    } else {
        "not elevated"
    };
    let system = format!(
        "{cpu} ({cpus} logical) · {ram} RAM · {os}/{arch} · {elevated}",
        cpu = fp.cpu,
        cpus = fp.logical_cpus,
        ram = fp.total_ram,
        os = fp.os,
        arch = fp.arch,
    );

    let tools = format!(
        "UFFS {uffs} vs UFFS-C++ {cpp} vs Everything {gui} (GUI) / es {es} (CLI)",
        uffs = tool_version(fp, "uffs"),
        cpp = tool_version(fp, "uffs_cpp"),
        gui = tool_version(fp, "everything_gui"),
        es = tool_version(fp, "everything"),
    );

    let benched_drives: Vec<&DriveRecord> = drives
        .iter()
        .filter(|rec| is_benched(rec, benched))
        .collect();
    let benched_records: u64 = benched_drives.iter().map(|rec| rec.mft_records).sum();
    let benched_list = if benched_drives.is_empty() {
        "none".to_owned()
    } else {
        benched_drives
            .iter()
            .map(|rec| format!("{}: {}", rec.drive, rec.drive_type))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let benched_cell = format!(
        "{n} of {total} ({list}) — {records} MFT records under test",
        n = benched_drives.len(),
        total = drives.len(),
        list = benched_list,
        records = storage::commas(benched_records),
    );

    let total_records: u64 = drives.iter().map(|rec| rec.mft_records).sum();
    let total_bytes: u64 = drives.iter().map(|rec| rec.total_bytes).sum();
    let inventory = format!(
        "{n} NTFS volume(s) · {size} · {records} MFT records total",
        n = drives.len(),
        size = storage::fmt_bytes(total_bytes),
        records = storage::commas(total_records),
    );

    // Scannable facts in a 2-column table; the "what we measure" description
    // (a sentence, not a metric) reads as an italic note below it.
    format!(
        "## At a glance\n\n\
         | | |\n\
         |:--|:--|\n\
         | **System** | {system} |\n\
         | **Tools** | {tools} |\n\
         | **Benchmarked** | {benched_cell} |\n\
         | **Inventory** | {inventory} |\n\
         \n\
         _Measured: cross-tool head-to-head on real output (file / stdout sinks) + UFFS native \
         full-suite (count latency, hot tier). Drive **kind** dominates latency — NVMe ≫ SSD ≫ \
         HDD — so the benched mix above frames every number that follows._\n"
    )
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::env::ToolVersion;

    fn fixed_time() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    fn tool(name: &str, version: &str) -> ToolVersion {
        ToolVersion {
            name: name.to_owned(),
            exe: format!("{name}.exe"),
            version: version.to_owned(),
            state: "running".to_owned(),
        }
    }

    fn sample_fp() -> EnvFingerprint {
        EnvFingerprint {
            captured_at: fixed_time(),
            os: "windows".to_owned(),
            arch: "x86_64".to_owned(),
            hostname: "box".to_owned(),
            elevated: true,
            cpu: "AMD Ryzen 9 3900XT".to_owned(),
            logical_cpus: "24".to_owned(),
            total_ram: "63.9 GiB".to_owned(),
            ram_bytes: 68_633_886_720,
            tools: vec![
                tool("uffs", "0.5.120"),
                tool("uffs_cpp", "1.0.0"),
                tool("everything", "1.1.0.30"),
                tool("everything_gui", "1.4.1.1024"),
            ],
        }
    }

    fn drives() -> Vec<DriveRecord> {
        storage::parse(
            r#"[
              {"drive":"C","boot":true,"label":"OS","drive_type":"NVMe","total_bytes":1099511627776,"used_pct":50.0,"mft_records":4000000},
              {"drive":"D","boot":false,"label":"DATA","drive_type":"HDD","total_bytes":2199023255552,"used_pct":90.0,"mft_records":7000000},
              {"drive":"S","boot":false,"label":"BIG","drive_type":"HDD","total_bytes":8796093022208,"used_pct":99.0,"mft_records":11000000}
            ]"#,
        )
    }

    #[test]
    fn renders_system_tools_and_benched_split() {
        let md = render_md(&sample_fp(), &drives(), &['C', 'D']);
        assert!(md.starts_with("## At a glance"));
        assert!(md.contains("|:--|:--|"));
        assert!(md.contains(
            "| **System** | AMD Ryzen 9 3900XT (24 logical) · 63.9 GiB RAM · windows/x86_64 · elevated |"
        ));
        assert!(md.contains(
            "| **Tools** | UFFS 0.5.120 vs UFFS-C++ 1.0.0 vs Everything 1.4.1.1024 (GUI) / es 1.1.0.30 (CLI) |"
        ));
        // 2 of 3 benched (C NVMe + D HDD), 11M records under test.
        assert!(md.contains(
            "| **Benchmarked** | 2 of 3 (C: NVMe, D: HDD) — 11,000,000 MFT records under test |"
        ));
        // Full inventory: 3 drives, 22,000,000 records.
        assert!(md.contains(
            "| **Inventory** | 3 NTFS volume(s) · 11.0 TB · 22,000,000 MFT records total |"
        ));
        // "Measured" reads as a note, not a table row.
        assert!(md.contains("_Measured: cross-tool head-to-head"));
    }

    #[test]
    fn missing_tool_renders_question_mark() {
        let mut fp = sample_fp();
        fp.tools.retain(|tool| tool.name != "uffs_cpp");
        let md = render_md(&fp, &[], &[]);
        assert!(md.contains("UFFS-C++ ?"));
        assert!(md.contains("0 of 0 (none)"));
    }
}
