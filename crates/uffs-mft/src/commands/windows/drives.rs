// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `drives` command handler — NTFS drive discovery and per-drive summary.
//!
//! Prints every NTFS volume with its label, capacity, free space, and `$MFT`
//! statistics, as a human table or machine JSON (consumed by the benchmark
//! report).  The lint exemptions below capture the CLI-specific display
//! patterns; library code never inherits them.
#![expect(
    clippy::print_stdout,
    reason = "intentional user-facing CLI drive listing output"
)]
#![expect(
    clippy::float_arithmetic,
    clippy::default_numeric_fallback,
    reason = "byte/percent calculations convert integer counters into f64 for human-readable display"
)]
#![expect(
    clippy::min_ident_chars,
    reason = "short closure identifiers aid readability in CLI driver code"
)]

use anyhow::{Context as _, Result};
use uffs_mft::u64_to_f64;

use super::shared::drive_type_label;
use crate::cli::OutputFormat;
use crate::display::{format_bytes, format_number_commas, truncate_string};

/// Per-drive summary used by [`cmd_drives`] to lay out the per-drive table.
#[cfg(windows)]
struct DriveInfo {
    /// Drive letter (e.g. `C`, `D`).
    letter: uffs_mft::platform::DriveLetter,
    /// `true` when this drive hosts the running OS.
    is_boot: bool,
    /// Volume label as reported by `GetVolumeInformationW`.
    label: String,
    /// Human-readable drive type (`SSD`, `HDD`, `NVMe`, ...).
    drive_type: String,
    /// Total volume capacity in bytes.
    total_size: u64,
    /// Free space in bytes (per Win32 disk-free-space query).
    free_space: u64,
    /// Used space in bytes (`total_size - free_space`).
    used_space: u64,
    /// Used capacity percentage in `[0.0, 100.0]`.
    used_pct: f64,
    /// Size of the `$MFT` file in bytes.
    mft_size: u64,
    /// Number of allocated MFT records on this volume.
    mft_records: u64,
}

/// `drives` CLI command — list every NTFS drive on this system with its
/// label, size, free space, and `$MFT` statistics.
#[cfg(windows)]
pub(crate) async fn cmd_drives(format: OutputFormat) -> Result<()> {
    use tracing::debug;
    use uffs_mft::platform::detect_ntfs_drives;

    debug!("🔍 Detecting NTFS drives...");

    let drives = detect_ntfs_drives();

    if drives.is_empty() {
        debug!("❌ No NTFS drives found");
        if matches!(format, OutputFormat::Json) {
            println!("[]");
        } else {
            println!("No NTFS drives found.");
        }
        return Ok(());
    }

    debug!(
        count = drives.len(),
        "✅ Found {} NTFS drive(s)",
        drives.len()
    );

    let drive_infos = collect_drive_infos(&drives);

    // JSON consumers (e.g. the benchmark report) get the machine form and
    // skip the human table entirely.
    if matches!(format, OutputFormat::Json) {
        return print_drives_json(&drive_infos);
    }

    print_drives_table(&drive_infos);
    Ok(())
}

/// Query type, label, capacity, and `$MFT` statistics for each drive.
///
/// Drives whose volume handle cannot be opened (insufficient privileges,
/// dismounted volume) are silently skipped — the listing is best-effort.
#[cfg(windows)]
fn collect_drive_infos(drives: &[uffs_mft::platform::DriveLetter]) -> Vec<DriveInfo> {
    use tracing::debug;
    use uffs_mft::platform::{VolumeHandle, detect_drive_type, is_boot_drive};

    let mut drive_infos: Vec<DriveInfo> = Vec::new();

    for drive in drives {
        // Detect drive type
        let drive_type = detect_drive_type(*drive);
        let drive_type_str = drive_type_label(drive_type, "???");

        // Get volume label
        let label = get_volume_label(*drive).unwrap_or_default();

        // Try to get volume info for each drive
        if let Ok(handle) = VolumeHandle::open(*drive) {
            let vol_data = handle.volume_data();
            let total_size = vol_data.total_clusters * u64::from(vol_data.bytes_per_cluster);
            let free_space = vol_data.free_clusters * u64::from(vol_data.bytes_per_cluster);
            let used_space = total_size.saturating_sub(free_space);
            let used_pct = if total_size > 0 {
                (u64_to_f64(used_space) / u64_to_f64(total_size)) * 100.0
            } else {
                0.0
            };
            let mft_size = vol_data.mft_valid_data_length;
            let mft_records = mft_size / u64::from(vol_data.bytes_per_file_record_segment);

            debug!(
                drive = %drive,
                label = %label,
                drive_type = drive_type_str,
                total_size,
                free_space,
                mft_records,
                "📁 Drive details"
            );

            drive_infos.push(DriveInfo {
                letter: *drive,
                is_boot: is_boot_drive(*drive),
                label,
                drive_type: drive_type_str.to_owned(),
                total_size,
                free_space,
                used_space,
                used_pct,
                mft_size,
                mft_records,
            });
        }
    }

    drive_infos
}

/// Print the human-readable drives summary table with a totals row.
#[cfg(windows)]
fn print_drives_table(drive_infos: &[DriveInfo]) {
    // Print table header
    println!();
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("                                    NTFS DRIVES SUMMARY");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!();
    println!(
        "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>7} {:>10} {:>12}",
        "Drive", "Label", "Type", "Size", "Used", "Free", "Used%", "MFT Size", "MFT Records"
    );
    println!(
        "{:-<6} {:-<16} {:-<5} {:->10} {:->10} {:->10} {:->7} {:->10} {:->12}",
        "", "", "", "", "", "", "", "", ""
    );

    // Print each drive (* = boot/system drive)
    for info in drive_infos {
        let drive_col = if info.is_boot {
            format!("{}:*", info.letter)
        } else {
            format!("{}:", info.letter)
        };
        println!(
            "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>6.1}% {:>10} {:>12}",
            drive_col,
            truncate_string(&info.label, 16),
            info.drive_type,
            format_bytes(info.total_size),
            format_bytes(info.used_space),
            format_bytes(info.free_space),
            info.used_pct,
            format_bytes(info.mft_size),
            format_number_commas(info.mft_records),
        );
    }

    // Print totals
    let total_size: u64 = drive_infos.iter().map(|d| d.total_size).sum();
    let total_used: u64 = drive_infos.iter().map(|d| d.used_space).sum();
    let total_free: u64 = drive_infos.iter().map(|d| d.free_space).sum();
    let total_mft: u64 = drive_infos.iter().map(|d| d.mft_size).sum();
    let total_records: u64 = drive_infos.iter().map(|d| d.mft_records).sum();
    let total_used_pct = if total_size > 0 {
        (u64_to_f64(total_used) / u64_to_f64(total_size)) * 100.0
    } else {
        0.0
    };

    println!(
        "{:-<6} {:-<16} {:-<5} {:->10} {:->10} {:->10} {:->7} {:->10} {:->12}",
        "", "", "", "", "", "", "", "", ""
    );
    println!(
        "{:<6} {:<16} {:<5} {:>10} {:>10} {:>10} {:>6.1}% {:>10} {:>12}",
        "TOTAL",
        format!("({} drives)", drive_infos.len()),
        "",
        format_bytes(total_size),
        format_bytes(total_used),
        format_bytes(total_free),
        total_used_pct,
        format_bytes(total_mft),
        format_number_commas(total_records),
    );
    println!();
    println!("  * = boot/system drive");
    println!();
}

/// Machine-readable per-drive record for `drives --format json`.
///
/// Field names are stable JSON keys consumed by the benchmark report; byte
/// counters keep their `_bytes` suffix so units are unambiguous.
#[cfg(windows)]
#[derive(serde::Serialize)]
struct DriveRecord {
    /// Drive letter (e.g. `"C"`).
    drive: String,
    /// `true` when this drive hosts the running OS.
    boot: bool,
    /// Volume label.
    label: String,
    /// Storage kind: `"NVMe"`, `"SSD"`, `"HDD"`, or `"???"` (undetected).
    drive_type: String,
    /// Total volume capacity in bytes.
    total_bytes: u64,
    /// Used capacity in bytes.
    used_bytes: u64,
    /// Free capacity in bytes.
    free_bytes: u64,
    /// Used capacity percentage in `[0, 100]`.
    used_pct: f64,
    /// `$MFT` size in bytes.
    mft_size_bytes: u64,
    /// Allocated MFT record count.
    mft_records: u64,
}

/// Emit the drive list as a pretty-printed JSON array on stdout.
///
/// # Errors
/// Returns an error only if JSON serialisation fails (effectively never, given
/// the plain scalar fields).
#[cfg(windows)]
fn print_drives_json(drive_infos: &[DriveInfo]) -> Result<()> {
    let records: Vec<DriveRecord> = drive_infos
        .iter()
        .map(|info| DriveRecord {
            drive: info.letter.to_string(),
            boot: info.is_boot,
            label: info.label.clone(),
            drive_type: info.drive_type.clone(),
            total_bytes: info.total_size,
            used_bytes: info.used_space,
            free_bytes: info.free_space,
            used_pct: info.used_pct,
            mft_size_bytes: info.mft_size,
            mft_records: info.mft_records,
        })
        .collect();
    let json = serde_json::to_string_pretty(&records).context("serialising drives to JSON")?;
    println!("{json}");
    Ok(())
}

/// Look up the volume label for `drive` via `GetVolumeInformationW`.
///
/// Returns `None` if the call fails or the volume has no label set.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "required for windows ffi call to GetVolumeInformationW"
)]
fn get_volume_label(drive: uffs_mft::platform::DriveLetter) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;

    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    use windows::core::PCWSTR;

    let root_path: Vec<u16> = format!("{drive}:\\")
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();

    let mut volume_name_buf = [0_u16; 261];

    // SAFETY: `root_path` is a NUL-terminated UTF-16 buffer kept alive for the
    // call duration; `volume_name_buf` is a writable 261-element stack buffer
    // matching the Win32 maximum volume-name length; the remaining four
    // optional out-parameters are documented as accepting `None`.
    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR::from_raw(root_path.as_ptr()),
            Some(&mut volume_name_buf),
            None,
            None,
            None,
            None,
        )
    };

    result.is_ok().then(|| {
        let len = volume_name_buf.iter().position(|&c| c == 0).unwrap_or(0);
        let name_slice = volume_name_buf.get(..len).unwrap_or(&[]);
        let label = OsString::from_wide(name_slice);
        label.to_string_lossy().to_string()
    })
}
