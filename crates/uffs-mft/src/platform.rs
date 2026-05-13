// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Platform-specific implementations for Windows.
//!
//! This module provides Windows API wrappers for:
//! - Volume handle management
//! - NTFS volume data retrieval
//! - Privilege checking
//!
//! Some types (`MftExtent`, `MftBitmap`, `DriveType`) are available on all
//! platforms for testing and offline MFT processing.
//!
//! # Safety
//!
//! This module uses Windows FFI and requires careful handling of raw handles.

mod bitmap;
mod extents;
mod system;
/// `$UpCase` table reading from live NTFS volume.
pub mod upcase;
#[cfg(windows)]
mod volume;

pub use bitmap::MftBitmap;
pub use extents::MftExtent;
// Export DriveType unconditionally (needed for tests), but Windows-specific functions only on
// Windows
pub use system::DriveType;
// is_volume_read_only — Windows-only (non-Windows stub was removed because
// every caller in this crate is #[cfg(windows)]-gated).  Consumed by the
// uffs_mft bin (commands/windows/incremental.rs) via the external-style
// `uffs_mft::platform::is_volume_read_only` path, so must be pub.
#[cfg(windows)]
pub use system::is_volume_read_only;
#[cfg(windows)]
pub(crate) use system::u32_size_of;
// System memory query — available on all platforms
pub use system::{SystemMemory, query_system_memory};
// Public API surface — consumed cross-crate (uffs-daemon), by the
// uffs_mft bin (commands/) via `uffs_mft::platform::*` external-style
// paths, and as platform utility helpers (infer_drive_from_path,
// volume_root_path are stable public API restored from the Phase 2.5
// demotion in commit 1529cb162).
#[cfg(windows)]
pub use system::{
    detect_boot_drive, detect_drive_type, detect_ntfs_drives, infer_drive_from_path, is_boot_drive,
    is_elevated, volume_root_path,
};
#[cfg(windows)]
pub(crate) use volume::{
    IOCP_WAIT_COMPLETION_DEADLINE, IOCP_WAIT_POLL_INTERVAL_MS, WAIT_TIMEOUT_ERROR_CODE,
    classify_wait_error_code, wait_deadline_exceeded,
};
// Both VolumeHandle and NtfsVolumeData are part of the public API —
// `VolumeHandle::volume_data()` returns `&NtfsVolumeData` so the latter
// must be at least as public as the former.
#[cfg(windows)]
pub use volume::{NtfsVolumeData, VolumeHandle};

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn volume_root_path_works() {
        assert_eq!(volume_root_path('c'), PathBuf::from("C:\\"));
        assert_eq!(volume_root_path('D'), PathBuf::from("D:\\"));
    }

    #[test]
    fn is_elevated_works() {
        // Just verify it doesn't panic.  Bind with `_unused` so the must_use
        // result is consumed without us losing the annotation; the prefix
        // signals "intentionally unused" without triggering
        // `let_underscore_must_use`.
        let _unused = is_elevated();
    }

    #[test]
    fn nvme_optimal_settings() {
        let drive_type = DriveType::Nvme;

        assert_eq!(drive_type.optimal_concurrency(), 32);
        assert_eq!(drive_type.optimal_io_size(), 4 * 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 4 * 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 8);
        assert!(drive_type.is_high_performance());
        assert!(drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn ssd_optimal_settings() {
        let drive_type = DriveType::Ssd;

        assert_eq!(drive_type.optimal_concurrency(), 8);
        assert_eq!(drive_type.optimal_io_size(), 2 * 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 2 * 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 4);
        assert!(drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn hdd_optimal_settings() {
        let drive_type = DriveType::Hdd;

        assert_eq!(drive_type.optimal_concurrency(), 4);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn hdd_extent_aware_concurrency() {
        assert_eq!(DriveType::optimal_concurrency_for_hdd(62), 2);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(100), 2);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(51), 2);

        assert_eq!(DriveType::optimal_concurrency_for_hdd(50), 4);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(28), 4);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(21), 4);

        assert_eq!(DriveType::optimal_concurrency_for_hdd(20), 6);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(19), 6);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(17), 6);
        assert_eq!(DriveType::optimal_concurrency_for_hdd(1), 6);
    }

    #[test]
    fn unknown_optimal_settings() {
        let drive_type = DriveType::Unknown;

        assert_eq!(drive_type.optimal_concurrency(), 4);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn optimal_settings_are_reasonable() {
        let nvme = DriveType::Nvme;
        let ssd = DriveType::Ssd;
        let hdd = DriveType::Hdd;

        assert!(nvme.optimal_concurrency() > ssd.optimal_concurrency());
        assert!(ssd.optimal_concurrency() > hdd.optimal_concurrency());

        assert!(nvme.optimal_io_size() > ssd.optimal_io_size());
        assert!(ssd.optimal_io_size() >= hdd.optimal_io_size());

        assert!(nvme.prefetch_buffers() > ssd.prefetch_buffers());
        assert!(ssd.prefetch_buffers() >= hdd.prefetch_buffers());
    }
}
