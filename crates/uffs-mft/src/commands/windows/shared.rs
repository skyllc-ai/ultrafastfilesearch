//! Shared helpers for split Windows command modules.

use std::time::Duration;

/// Returns the display label for a detected drive type.
pub(super) fn drive_type_label(
    drive_type: uffs_mft::DriveType,
    unknown_label: &'static str,
) -> &'static str {
    match drive_type {
        uffs_mft::DriveType::Nvme => "NVMe",
        uffs_mft::DriveType::Ssd => "SSD",
        uffs_mft::DriveType::Hdd => "HDD",
        uffs_mft::DriveType::Unknown => unknown_label,
    }
}

/// Sleeps briefly between benchmark runs so the system can settle.
pub(super) async fn pause_between_benchmark_runs(run: u32, runs: u32) {
    if run < runs {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
