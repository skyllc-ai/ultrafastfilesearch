// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Tests for parallel reader configuration helpers.
//!
//! Windows-only: `ParallelMftReader` requires Windows types and methods.

#![cfg(windows)]

use super::*;

#[test]
fn test_parallel_mft_reader_uses_optimal_chunk_size() {
    use crate::platform::DriveType;

    let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

    let nvme_reader = ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Nvme);
    assert_eq!(
        nvme_reader.chunk_size,
        4 * 1024 * 1024,
        "NVMe should use 4MB chunk size"
    );
    assert_eq!(nvme_reader.drive_type, DriveType::Nvme);

    let ssd_reader = ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Ssd);
    assert_eq!(
        ssd_reader.chunk_size,
        2 * 1024 * 1024,
        "SSD should use 2MB chunk size"
    );
    assert_eq!(ssd_reader.drive_type, DriveType::Ssd);

    let hdd_reader = ParallelMftReader::new_optimized(extent_map.clone(), None, DriveType::Hdd);
    assert_eq!(
        hdd_reader.chunk_size,
        1024 * 1024,
        "HDD should use 1MB chunk size"
    );
    assert_eq!(hdd_reader.drive_type, DriveType::Hdd);

    let unknown_reader = ParallelMftReader::new_optimized(extent_map, None, DriveType::Unknown);
    assert_eq!(
        unknown_reader.chunk_size,
        1024 * 1024,
        "Unknown should use 1MB chunk size"
    );
    assert_eq!(unknown_reader.drive_type, DriveType::Unknown);
}

#[test]
fn test_drive_type_stored_in_reader() {
    use crate::platform::DriveType;

    let extent_map = MftExtentMap::contiguous(100, 1024 * 1024, 4096, 1024);

    for drive_type in [
        DriveType::Nvme,
        DriveType::Ssd,
        DriveType::Hdd,
        DriveType::Unknown,
    ] {
        let reader = ParallelMftReader::new_optimized(extent_map.clone(), None, drive_type);
        assert_eq!(
            reader.drive_type, drive_type,
            "Drive type should be stored in reader"
        );
    }
}

#[test]
fn test_optimal_defaults_when_none_passed() {
    use crate::platform::DriveType;

    fn resolve_concurrency(user_value: Option<usize>, drive_type: DriveType) -> usize {
        user_value.unwrap_or_else(|| drive_type.optimal_concurrency())
    }

    fn resolve_io_size(user_value: Option<usize>, drive_type: DriveType) -> usize {
        user_value.unwrap_or_else(|| drive_type.optimal_io_size())
    }

    assert_eq!(resolve_concurrency(None, DriveType::Nvme), 32);
    assert_eq!(resolve_io_size(None, DriveType::Nvme), 4 * 1024 * 1024);

    assert_eq!(resolve_concurrency(None, DriveType::Ssd), 8);
    assert_eq!(resolve_io_size(None, DriveType::Ssd), 2 * 1024 * 1024);

    assert_eq!(resolve_concurrency(None, DriveType::Hdd), 4);
    assert_eq!(resolve_io_size(None, DriveType::Hdd), 1024 * 1024);

    assert_eq!(resolve_concurrency(None, DriveType::Unknown), 4);
    assert_eq!(resolve_io_size(None, DriveType::Unknown), 1024 * 1024);

    assert_eq!(resolve_concurrency(Some(16), DriveType::Nvme), 16);
    assert_eq!(
        resolve_io_size(Some(8 * 1024 * 1024), DriveType::Hdd),
        8 * 1024 * 1024
    );
}

#[test]
fn test_parallel_parsing_auto_detection() {
    use crate::platform::DriveType;

    fn resolve_parallel_parse(user_value: Option<bool>, drive_type: DriveType) -> bool {
        user_value.unwrap_or_else(|| drive_type.benefits_from_parallel_parsing())
    }

    assert!(
        resolve_parallel_parse(None, DriveType::Nvme),
        "NVMe should auto-enable parallel parsing"
    );
    assert!(
        !resolve_parallel_parse(None, DriveType::Ssd),
        "SSD should NOT auto-enable parallel parsing"
    );
    assert!(
        !resolve_parallel_parse(None, DriveType::Hdd),
        "HDD should NOT auto-enable parallel parsing"
    );
    assert!(
        !resolve_parallel_parse(None, DriveType::Unknown),
        "Unknown should NOT auto-enable parallel parsing"
    );

    assert!(resolve_parallel_parse(Some(true), DriveType::Hdd));
    assert!(!resolve_parallel_parse(Some(false), DriveType::Nvme));
}
