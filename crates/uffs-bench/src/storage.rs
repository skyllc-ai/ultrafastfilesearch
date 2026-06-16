// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Storage-device inventory for the report.
//!
//! Captures `uffs-mft drives --format json` (our own detector — the same
//! `detect_drive_type` UFFS uses to pick its I/O mode) and renders a
//! `## Storage devices` markdown table that shows **every** NTFS volume on the
//! host with its kind (`NVMe` / SSD / HDD), capacity, fullness, and MFT record
//! count, flagging which drives the bench actually measured.
//!
//! Drive **kind** is the dominant latency factor in a benchmark (`NVMe` ≫ SSD ≫
//! HDD), and fullness / MFT-record count explain per-drive cost — so this table
//! tells a reader at a glance *why* a given drive was fast, slow, or excluded.

use std::path::Path;

use serde::Deserialize;

use crate::host::Host;
use crate::resolve;

/// Bundle-relative name of the captured drive inventory JSON.
pub const DRIVES_JSON: &str = "drives.json";

/// One NTFS volume as reported by `uffs-mft drives --format json`.
///
/// Field names mirror the producer's stable JSON keys (see
/// `uffs-mft`'s `DriveRecord`).
#[derive(Debug, Clone, Deserialize)]
pub struct DriveRecord {
    /// Drive letter (e.g. `"C"`).
    pub drive: String,
    /// `true` when this drive hosts the running OS.
    #[serde(default)]
    pub boot: bool,
    /// Volume label.
    #[serde(default)]
    pub label: String,
    /// Storage kind: `"NVMe"`, `"SSD"`, `"HDD"`, or `"???"` (undetected).
    pub drive_type: String,
    /// Total volume capacity in bytes.
    pub total_bytes: u64,
    /// Used capacity percentage in `[0, 100]`.
    pub used_pct: f64,
    /// Allocated MFT record count (the per-drive scan workload).
    pub mft_records: u64,
}

/// Capture `uffs-mft drives --format json`, persist it into the bundle, and
/// return the parsed records (for the at-a-glance summary).
///
/// Best-effort: a missing `uffs-mft` binary or a probe that yields no JSON is
/// logged and yields an empty vec — the report simply omits the storage
/// section and the summary's drive lines.
pub fn capture_and_write(host: &dyn Host, bundle_dir: &Path) -> Vec<DriveRecord> {
    let exe = resolve::uffs_mft_exe(host);
    match host.run(&exe, &["drives", "--format", "json"]) {
        Ok(out) if out.success() && out.stdout.trim_start().starts_with('[') => {
            let path = bundle_dir.join(DRIVES_JSON);
            if let Err(err) = host.write_file(&path, out.stdout.as_bytes()) {
                host.out(&format!("[storage] could not write {DRIVES_JSON}: {err}"));
            }
            parse(&out.stdout)
        }
        Ok(_) => {
            host.out("[storage] uffs-mft produced no drive JSON — storage section skipped");
            Vec::new()
        }
        Err(err) => {
            host.out(&format!(
                "[storage] uffs-mft not available ({err}) — storage section skipped"
            ));
            Vec::new()
        }
    }
}

/// Parse the captured `drives.json`; returns an empty vec on any error.
#[must_use]
pub fn parse(json: &str) -> Vec<DriveRecord> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Format a byte count as a one-decimal `TB`/`GB`/`MB` string using
/// integer-only math (the crate avoids floating-point arithmetic).
pub(crate) fn fmt_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;
    if bytes >= TIB {
        format!("{}.{} TB", bytes / TIB, (bytes % TIB) * 10 / TIB)
    } else if bytes >= GIB {
        format!("{}.{} GB", bytes / GIB, (bytes % GIB) * 10 / GIB)
    } else {
        format!("{} MB", bytes / MIB)
    }
}

/// Insert ASCII thousands separators into a `u64` (e.g. `4656384` →
/// `"4,656,384"`).
pub(crate) fn commas(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let len = digits.len();
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Render the `## Storage devices` markdown section.
///
/// `benched` is the set of drive letters the matrix selected; those rows are
/// flagged ✓ (and a note explains the rest were excluded by the ES RAM budget).
/// Returns `None` when there are no drives to show.
#[must_use]
pub fn render_md(drives: &[DriveRecord], benched: &[char]) -> Option<String> {
    if drives.is_empty() {
        return None;
    }
    let is_benched = |rec: &DriveRecord| -> bool {
        rec.drive
            .chars()
            .next()
            .is_some_and(|letter| benched.contains(&letter.to_ascii_uppercase()))
    };

    let note = "_Detected via `uffs-mft drives` (the same `detect_drive_type` UFFS uses to \
                choose its I/O mode). **Benched** drives were included in the cross-tool matrix; \
                the rest were excluded by the Everything RAM budget (see Negotiated matrix). \
                Drive **kind** is the dominant latency factor — NVMe ≫ SSD ≫ HDD — and \
                fullness / MFT-record count explain per-drive cost._";

    let mut lines = vec![
        "## Storage devices".to_owned(),
        String::new(),
        note.to_owned(),
        String::new(),
        "| Drive | Kind | Label | Size | Used | MFT records | Benched |".to_owned(),
        "|-------|------|-------|------|------|-------------|:-------:|".to_owned(),
    ];
    for rec in drives {
        let drive_cell = if rec.boot {
            format!("{}: (boot)", rec.drive)
        } else {
            format!("{}:", rec.drive)
        };
        let label = if rec.label.is_empty() {
            "—"
        } else {
            rec.label.as_str()
        };
        let benched_cell = if is_benched(rec) { "✓" } else { "" };
        lines.push(format!(
            "| {} | {} | {} | {} | {:.1}% | {} | {} |",
            drive_cell,
            rec.drive_type,
            label,
            fmt_bytes(rec.total_bytes),
            rec.used_pct,
            commas(rec.mft_records),
            benched_cell,
        ));
    }
    Some(format!("{}\n", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operator's real 7-drive inventory (trimmed to the fields we keep).
    const SAMPLE: &str = r#"[
      {"drive":"C","boot":true,"label":"BOOT 990","drive_type":"NVMe","total_bytes":1676799836160,"used_pct":85.5,"mft_records":4656384},
      {"drive":"D","boot":false,"label":"DATA","drive_type":"HDD","total_bytes":8001427599360,"used_pct":64.0,"mft_records":4917248},
      {"drive":"G","boot":false,"label":"NTFS_16_GB","drive_type":"???","total_bytes":15804112896,"used_pct":3.9,"mft_records":20224}
    ]"#;

    #[test]
    fn parses_records() {
        let drives = parse(SAMPLE);
        assert_eq!(drives.len(), 3);
        let first = drives.first().expect("first record");
        assert_eq!(first.drive, "C");
        assert!(first.boot);
        assert_eq!(first.drive_type, "NVMe");
        assert_eq!(drives.get(1).expect("second record").mft_records, 4_917_248);
    }

    #[test]
    fn parse_bad_json_is_empty() {
        assert!(parse("not json").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn render_flags_benched_and_lists_all() {
        let drives = parse(SAMPLE);
        let md = render_md(&drives, &['C', 'D']).expect("non-empty");
        assert!(md.starts_with("## Storage devices"));
        // All three drives present.
        assert!(md.contains("| C: (boot) | NVMe | BOOT 990 | 1.5 TB | 85.5% | 4,656,384 | ✓ |"));
        assert!(md.contains("| D: | HDD | DATA | 7.2 TB | 64.0% | 4,917,248 | ✓ |"));
        // G is NOT benched → empty marker.
        assert!(md.contains("| G: | ??? | NTFS_16_GB | 14.7 GB | 3.9% | 20,224 |  |"));
    }

    #[test]
    fn render_empty_is_none() {
        assert!(render_md(&[], &['C']).is_none());
    }

    #[test]
    fn fmt_bytes_scales() {
        assert_eq!(fmt_bytes(1_676_799_836_160), "1.5 TB");
        assert_eq!(fmt_bytes(15_804_112_896), "14.7 GB");
        assert_eq!(fmt_bytes(20_709_376), "19 MB");
    }
}
