//! Platform-specific implementations for Windows.
//!
//! This module provides Windows API wrappers for:
//! - Volume handle management
//! - NTFS volume data retrieval
//! - Privilege checking
//!
//! Some types (MftExtent, MftBitmap, DriveType) are available on all platforms
//! for testing and offline MFT processing.
//!
//! # Safety
//!
//! This module uses Windows FFI and requires careful handling of raw handles.

// Platform module is mostly Windows-specific with cross-platform stubs
#![allow(clippy::all, clippy::nursery, clippy::pedantic)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

mod bitmap;
mod extents;
mod system;
#[cfg(windows)]
mod volume;

pub use bitmap::MftBitmap;
pub use extents::MftExtent;
// Export DriveType unconditionally (needed for tests), but Windows-specific functions only on
// Windows
pub use system::DriveType;
// is_volume_read_only is available on all platforms (stub on non-Windows)
pub use system::is_volume_read_only;
#[cfg(windows)]
pub use system::{
    detect_drive_type, detect_ntfs_drives, infer_drive_from_path, is_elevated, volume_root_path,
};
#[cfg(windows)]
pub(crate) use volume::{
    IOCP_WAIT_COMPLETION_DEADLINE, IOCP_WAIT_POLL_INTERVAL_MS, WAIT_TIMEOUT_ERROR_CODE,
    classify_wait_error_code, wait_deadline_exceeded,
};
#[cfg(windows)]
pub use volume::{NtfsVolumeData, VolumeHandle};

#[cfg(all(test, windows))]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_volume_root_path() {
        assert_eq!(volume_root_path('c'), PathBuf::from("C:\\"));
        assert_eq!(volume_root_path('D'), PathBuf::from("D:\\"));
    }

    #[test]
    fn test_is_elevated() {
        // Just verify it doesn't panic
        let _ = is_elevated();
    }

    #[test]
    fn test_nvme_optimal_settings() {
        let drive_type = DriveType::Nvme;

        assert_eq!(drive_type.optimal_concurrency(), 32);
        assert_eq!(drive_type.optimal_io_size(), 4 * 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 4 * 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 8);
        assert!(drive_type.is_high_performance());
        assert!(drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_ssd_optimal_settings() {
        let drive_type = DriveType::Ssd;

        assert_eq!(drive_type.optimal_concurrency(), 8);
        assert_eq!(drive_type.optimal_io_size(), 2 * 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 2 * 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 4);
        assert!(drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_hdd_optimal_settings() {
        let drive_type = DriveType::Hdd;

        assert_eq!(drive_type.optimal_concurrency(), 4);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_hdd_extent_aware_concurrency() {
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
    fn test_unknown_optimal_settings() {
        let drive_type = DriveType::Unknown;

        assert_eq!(drive_type.optimal_concurrency(), 4);
        assert_eq!(drive_type.optimal_io_size(), 1024 * 1024);
        assert_eq!(drive_type.optimal_chunk_size(), 1024 * 1024);
        assert_eq!(drive_type.prefetch_buffers(), 2);
        assert!(!drive_type.is_high_performance());
        assert!(!drive_type.benefits_from_parallel_parsing());
    }

    #[test]
    fn test_optimal_settings_are_reasonable() {
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
